# fastopm — proxy cache npm viết bằng Rust

Một việc duy nhất: đứng giữa client (npm / pnpm / yarn / bun) và registry gốc, cache lại
metadata và tarball để lần cài sau nhanh hơn và vẫn cài được khi registry gốc sập.

Thay thế được cho Verdaccio trong vai trò proxy cache: đọc cùng `config.yaml`, dùng cùng thư mục
storage. Baseline để so sánh hành vi: `../verdaccio`, commit `e2ab94b10`.

## 1. Phạm vi

**Có:**

| Route | Hành vi |
|---|---|
| `GET /:package` | Metadata. Bản rút gọn khi `Accept: application/vnd.npm.install-v1+json`, ngược lại bản đầy đủ |
| `GET /:package/:version` | Một version hoặc một dist-tag |
| `GET /:package/-/:filename` | Tarball |
| `GET /-/ping` | `{}` |

Package có scope nhận cả ba dạng `/@s/p`, `/@s%2Fp`, `/%40s/p`.

**Không có:** publish, unpublish, dist-tag, deprecate, user / login / token, web UI, plugin,
search, 2FA, stage, notify, admin CLI. Các route ghi trả `405 {"error": "..."}`.

**Giả định (báo nếu sai):**

- Không xác thực client. Ai gọi được tới server thì đọc được; header `Authorization` bị bỏ qua.
  Rule `access` trong `packages:` không được áp dụng (cảnh báo khi khởi động).
- Package đã publish sẵn trong storage Verdaccio cũ vẫn được phục vụ (chỉ đọc).

## 2. Giữ tương thích với Verdaccio ở đâu

| Mặt | Giữ gì |
|---|---|
| HTTP | Status, header, body của 4 route trên giống Verdaccio: body JSON pretty 2 space + `\n`, `ETag` = MD5 body, lỗi dạng `{"error": "..."}`, `dist.tarball` viết lại theo URL của proxy |
| Config | Đọc `config.yaml` của Verdaccio. Dùng: `storage`, `listen`, `uplinks.*`, `packages.*.proxy`, `packages.*.storage`, `url_prefix`, `log`, `http_proxy` / `https_proxy` / `no_proxy`, `user_agent`, `server_id`. Key khác: bỏ qua kèm cảnh báo, không lỗi |
| Biến môi trường | `VERDACCIO_PUBLIC_URL`, `VERDACCIO_FORWARDED_PROTO`, `VERDACCIO_STORAGE_PATH`, `NPM_TOKEN` |
| Storage | Cùng layout: `<storage>/<name>/package.json` (JSON gọn, có `_uplinks`, `_distfiles`, `_rev`) và `<storage>/<name>/<file>.tgz`. Trỏ vào thư mục cache Verdaccio sẵn có là chạy; quay lại Verdaccio cũng được |
| CLI | `fastopm --config <path> --listen <addr>`, `--version` |

Tùy chọn uplink được hỗ trợ, cùng mặc định với Verdaccio:

| Tùy chọn | Mặc định | Ý nghĩa |
|---|---|---|
| `url` | bắt buộc | Registry gốc |
| `maxage` | `2m` | Trong khoảng này dùng metadata cache, không hỏi uplink |
| `timeout` | `30s` | Timeout mỗi request |
| `max_fails` / `fail_timeout` | `2` / `5m` | Ngắt mạch: sau N lần lỗi thì coi uplink là offline trong khoảng thời gian này |
| `cache` | `true` | `false` thì chỉ stream tarball, không ghi đĩa |
| `headers`, `auth` | — | Header thêm và token gửi lên uplink (cho registry gốc riêng tư) |

Hành vi lõi sao chép từ Verdaccio:

- Thử các uplink trong `proxy:` lần lượt, cái đầu thành công thì dùng.
- Gửi `If-None-Match` với etag đã lưu; 304 thì chỉ cập nhật thời điểm `fetched`.
- Merge: chỉ thêm version chưa có, dist-tags và `time` lấy theo uplink.
- Uplink sập mà có bản cache → phục vụ bản cache. Không có bản cache → 404 (503 nếu uplink trả 503).
- Tarball: có file local thì gửi; không thì lấy URL từ `_distfiles`, vừa stream về client vừa ghi
  file tạm rồi rename. Chỉ tải từ host của uplink đã cấu hình.
- Chống vòng lặp: thêm `Via: 1.1 <server_id> (Verdaccio)` khi gọi uplink; request đến có `Via`
  chứa `server_id` của mình → 508.

## 3. Thiết kế

Một Cargo workspace, một binary. Mỗi crate do một session riêng viết nên không session nào sửa
file của session khác.

```
crates/
  fastopm-core/     kiểu dùng chung: lỗi, manifest, tên package, interval (`2m`, `30s`), hằng số
  fastopm-config/   parse YAML, default, khớp pattern `packages:`, địa chỉ listen
  fastopm-storage/  đọc/ghi package.json và tarball (file tạm + rename), lock theo package
  fastopm-uplink/   HTTP client, header, auth, retry, ngắt mạch, etag
  fastopm-render/   dựng response: full / rút gọn, viết lại dist.tarball, pretty JSON, ETag
  fastopm-store/    điều phối: cache → uplink → merge → ghi lại; gộp request trùng
  fastopm-server/   route, chuẩn hóa scope, format lỗi, nén, log request
  fastopm/          binary: CLI, khởi động, signal
compat/             harness Node so sánh với Verdaccio (ngoài Cargo workspace)
bench/              kịch bản benchmark
```

Baseline chạy thật trong harness: `verdaccio@7.1.0` từ npm (bản stable hiện tại, cùng internals
với source ở `../verdaccio`).

Thư viện: `tokio`, `axum`, `reqwest` (rustls), `serde_json` bật `preserve_order` (giữ thứ tự key
để body khớp Verdaccio), một parser YAML, `md-5`, `clap`. Khớp pattern `packages:` tự viết theo
ngữ nghĩa `minimatch` (không phân biệt hoa thường), kiểm bằng test vector sinh từ thư viện gốc.

Hiệu năng — các điểm Verdaccio làm chậm và cách xử lý:

| Verdaccio | fastopm |
|---|---|
| Mỗi GET đọc và parse `package.json` ít nhất 2 lần | Cache manifest đã parse trong RAM (LRU theo dung lượng); kiểm mtime + size của file để tự làm mới khi có tiến trình khác ghi |
| Mỗi GET serialize lại body và MD5 | Cache sẵn bytes + ETag + bản gzip cho từng biến thể (full / rút gọn) và từng base URL |
| `If-None-Match` vẫn dựng cả body | Khớp ETag trong cache → trả 304 ngay |
| N client hỏi cùng package chưa cache → N request lên uplink | Gộp thành một request (áp dụng cho cả metadata và tarball) |
| Buffer cả packument uplink trên một event loop | Parse trên thread pool; nhiều core |
| Tarball đi qua stream JS | Gửi file local bằng streaming zero-copy |

Giữ nguyên số lần ghi đĩa và cách tăng `_rev` như Verdaccio để storage dùng chung được. Ghi vào
`package.json` luôn có lock theo package (Verdaccio ghi cache không lock).

## 4. Lộ trình

**Bước 1 — MVP.** Config, storage, uplink, 4 route, cache RAM và gộp request ngay từ đầu.
Xong khi: `npm`, `pnpm`, `yarn`, `bun` install một project thật qua fastopm với config mặc định
của Verdaccio; lần cài thứ hai không còn request nào lên uplink ngoài kiểm tra `maxage`; tắt mạng
vẫn cài được từ cache.

**Bước 2 — Khớp hành vi.** Viết harness nhỏ: cùng một request vào Verdaccio và fastopm (chung mock
uplink), so status / header / body và so thư mục storage sau đó. Sửa các chỗ lệch: bảng ánh xạ lỗi,
ngắt mạch, 304, `url_prefix` / public URL sau reverse proxy, `packages.*.storage`.
Xong khi: harness sạch; Verdaccio đọc được storage do fastopm ghi và ngược lại;
`scripts/proxy-install-popular.sh` của Verdaccio (cài 20 cây dependency lớn) chạy qua.

**Bước 3 — Đo và tối ưu.** Benchmark so với Verdaccio trên cùng máy:

| Kịch bản | Chỉ số |
|---|---|
| GET metadata rút gọn, cache nóng (package nhỏ và `@types/node`) | RPS, p99 |
| GET metadata có `If-None-Match` khớp | RPS |
| GET tarball local 1 MB / 50 MB | throughput, CPU |
| Cài 20 cây dependency lớn, cache lạnh và cache nóng | tổng thời gian |
| Nghỉ và sau tải | RSS, thời gian khởi động |

Xong khi: có bảng số so sánh; Dockerfile và hướng dẫn chuyển từ Verdaccio.

## 5. Rủi ro

| Rủi ro | Giảm thiểu |
|---|---|
| Body JSON lệch byte so với Verdaccio (thứ tự key, số, escape) | Harness ở bước 2 so từng byte; lệch thì chỉ làm ETag khác, client tải lại một lần |
| Cache RAM lệch với đĩa khi chạy chung storage với Verdaccio | Kiểm mtime + size trước khi dùng cache |
| `npm install` gọi endpoint audit (`POST /-/npm/v1/security/*`) | Trả 405; nếu client báo lỗi gây phiền thì thêm chuyển tiếp thẳng lên uplink |
| Packument rất lớn (vài chục MB) chiếm RAM cache | Giới hạn cache theo dung lượng, không theo số entry |
