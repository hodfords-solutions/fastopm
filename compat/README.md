# compat — harness so sánh vi sai fastopm ↔ Verdaccio

Chạy cùng một chuỗi request vào **baseline** (`verdaccio@7.1.0`, devDependency ghim đúng version)
và **candidate** (binary bất kỳ có CLI `--config <path> --listen <addr>`, mặc định cũng là
Verdaccio để tự kiểm tra harness), rồi so từng bước:

- **response**: status, body từng byte (gzip thì so sau khi giải nén), mọi header trừ danh sách bỏ qua;
- **request mỗi mock uplink nhận được** trong bước đó: method, path thô, header (`accept`,
  `user-agent`, `via`, `if-none-match`, `x-forwarded-for`, `authorization`, …), số lần thử lại;
- **cây storage** cuối kịch bản: danh sách file, byte tarball, `package.json` (nội dung đã chuẩn hóa
  và định dạng: compact hay indent).

Mỗi kịch bản có môi trường riêng cho mỗi bên: mock uplink (Node thuần), thư mục storage tạm,
`config.yaml`, cổng tự chọn. Hai bên chạy song song nên thời gian (`maxage`, `fail_timeout`) như nhau.
Tiến trình con và thư mục tạm được dọn khi xong, khi lỗi và khi Ctrl-C. Harness không ra Internet.

## Chạy

```bash
cd compat
npm install
node run.mjs                                   # Verdaccio vs Verdaccio: phải 0 lệch
node run.mjs --candidate-bin ../target/release/fastopm
node run.mjs --candidate-bin ../target/release/fastopm --filter tarball/ --verbose
```

| Tùy chọn | |
|---|---|
| `--candidate-bin <path>` | binary candidate; được gọi `<path> --config <dir>/config.yaml --listen 127.0.0.1:<port>`, cwd là thư mục chứa config, sẵn sàng khi `GET /-/ping` trả 200 (chờ tối đa 30 s) |
| `--candidate-config <json>` | deep-merge vào config của riêng candidate (vd. `'{"url_prefix":"/other/"}'` để thấy harness bắt lệch) |
| `--filter <text>` / `--filter /regex/` | chỉ chạy kịch bản khớp tên (lặp được) |
| `--jobs <n>` | số kịch bản chạy song song (mặc định 4) |
| `--verbose` | in mọi bước: status, body rút gọn, request lên uplink của từng bên |
| `--keep` | giữ thư mục tạm (config, storage, `server.log` của từng bên) |
| `--json <file>` | ghi kết quả đầy đủ dạng JSON |
| `--list` | liệt kê kịch bản |

Exit code: `0` không có lệch ngoài danh sách cho phép, `1` có lệch, `2` lỗi harness (server không lên…).

Config sinh ra cho cả hai bên (`lib/config.mjs`): `server_id: compat-srv` cố định, `web.enable: false`,
`auth.htpasswd.max_users: -1` (tắt đăng ký user), `middlewares.audit.enabled: false` (không thì
Verdaccio chuyển tiếp audit tới registry.npmjs.org), `packages:` mặc định proxy mọi package qua
mọi uplink, trừ `local-*`. Request mặc định gửi `Host: localhost:4873` để `dist.tarball` giống nhau
giữa hai bên; kịch bản nào cần thì ghi đè.

## Cấu trúc

```
run.mjs                 điểm vào: tham số, chạy song song, báo cáo, exit code
lib/rules.mjs           MỌI ngoại lệ so sánh: header bỏ qua, file storage bỏ qua, chuẩn hóa, lệch cho phép
lib/runner.mjs          vòng đời một kịch bản (mock → seed → config → server → các bước → storage)
lib/compare.mjs         so response / request uplink / storage, chuẩn hóa
lib/mock-uplink.mjs     registry giả: packument + tarball, ETag/304, chế độ down/status/delay/reset, ghi log request
lib/registry.mjs        khởi động/dừng baseline và candidate, lọc biến môi trường
lib/config.mjs          sinh config.yaml
fixtures/packages.mjs   định nghĩa package (tarball sinh tất định, không có file nhị phân)
fixtures/storage.mjs    storage dựng sẵn theo layout Verdaccio (package cache từ uplink + package publish local)
scenarios/*.mjs         kịch bản, liệt kê trong scenarios/index.mjs
```

## Thêm kịch bản

```js
// scenarios/<nhóm>.mjs
import { act, get, req, wait, ownEtag, NPM_INSTALL_ACCEPT } from './helpers.mjs';

export default [
  {
    name: 'nhom/ten-kich-ban',             // duy nhất; dùng cho --filter và ALLOWED_DIFFS
    description: 'một dòng mô tả',
    uplinks: { upstream: { config: { maxage: '2s' } } },   // mặc định { upstream: {} }; `packages: [...]` giới hạn package mock phục vụ
    packages: { /* mục packages: của config, nếu cần khác mặc định */ },
    config: { url_prefix: '/npm/' },       // merge vào gốc config
    env: { VERDACCIO_PUBLIC_URL: '...' },  // biến môi trường cho cả hai server
    seed: (storageDir, side) => {},        // ghi storage trước khi server chạy
    setup: (side) => side.uplink().setMode('down'),   // trước khi server chạy
    steps: [
      get('/simple-pkg', { accept: NPM_INSTALL_ACCEPT }, { saveAs: 'first' }),
      act('uplink trả 503', (s) => s.uplink().setMode({ type: 'status', status: 503 })),
      wait(2500),
      get('/simple-pkg', { 'if-none-match': ownEtag('first') }),  // giá trị header có thể là hàm theo từng bên
      req('PUT', '/simple-pkg', { headers: { 'content-type': 'application/json' }, body: {} }),
    ],
  },
];
```

Rồi thêm file vào `scenarios/index.mjs`. Điều khiển mock: `setMode('down' | 'up' | {type:'status',status} |
{type:'delay',ms} | {type:'reset'})`, `override(path, behaviour)`, `clearOverrides()`,
`setPackage(name, def => newDef)` (vd. `withNewVersion` trong `fixtures/packages.mjs`).
Chạy `node run.mjs --filter <tên>` vài lần để chắc kịch bản ổn định với Verdaccio vs Verdaccio.

## Bỏ qua, chuẩn hóa và lệch cho phép

Chi tiết và lý do nằm trong `lib/rules.mjs`; tóm tắt:

- **Header response bỏ qua**: `date`, `connection`, `keep-alive`, `transfer-encoding`,
  `x-ratelimit-reset|limit|remaining` (bộ đếm/thời điểm của rate limiter Verdaccio).
- **Header suy ra từ body** (`etag`, `content-length`): nếu hai body giống hệt thì so nguyên văn;
  nếu body chỉ khác ở phần đã chuẩn hóa thì mỗi bên phải tính từ body của chính nó giống baseline
  (`"md5(body)"` cho JSON 2xx, ETag yếu của Express `W/"<len>-<sha1>"` cho lỗi). Với 304/HEAD
  (không có body), ETag mỗi bên được đối chiếu với body mà bên đó đã trả kèm ETag ấy trước đó.
- **Header request lên uplink bỏ qua**: `connection`, `keep-alive`.
- **File storage bỏ qua**: `.verdaccio-db.json` (danh sách package publish local + secret ngẫu nhiên),
  `.token-db.json`, `.sinopia-db.json`.
- **Chuẩn hóa**: `127.0.0.1:<port>` của mock/server → `{uplink:<tên>}` / `{registry}`; phần hex
  ngẫu nhiên của `_rev` (giữ bộ đếm và độ dài); `_uplinks.*.fetched` → `<timestamp>`. Mock tính ETag
  packument trên bản có origin chuẩn nên `_uplinks.*.etag` và `If-None-Match` giống nhau giữa hai bên.
- **Lệch cho phép** (`ALLOWED_DIFFS`, báo cáo ghi `≈ allowed` kèm lý do, không làm fail):
  - `write-routes/publish-and-unpublish`, `write-routes/user-and-token`, `write-routes/dist-tags-and-audit`:
    fastopm chỉ đọc, route ghi trả 405.
  - bước `GET /.hidden` trong `metadata/package-name-edge-cases`: Verdaccio 7.1.0 không bao giờ trả lời
    đường dẫn có đoạn bắt đầu bằng `.` (bug); fastopm trả 404 là được.
  - `misc/client-authorization-ignored`: fastopm không xác thực client và bỏ qua `Authorization`
    (PLAN §1); Verdaccio trả 401 `bad username/password, access denied` khi token không hợp lệ.

## Kiểm thử với client thật

`e2e/run.mjs` (cần Internet, không nằm trong bộ kịch bản offline): npm / pnpm / yarn / bun cài một
project thật qua fastopm và Verdaccio với registry gốc là npmjs — cache lạnh, cache nóng, lockfile sinh
từ npmjs, uplink sập, storage dùng chéo. Xem `e2e/README.md`.

## Hành vi Verdaccio 7.1.0 quan sát được (khác hoặc chi tiết hơn PLAN.md)

Chạy `node run.mjs --verbose` để xem lại. Những điểm fastopm cần biết:

1. **Uplink trả HTTP 503, chưa có cache → client nhận 404** `{"error":"no such package available"}`,
   không phải 503 như PLAN §2. Client chỉ nhận **503** `looks like the server is taking to long to respond`
   khi uplink **timeout** hoặc **reset kết nối**. Uplink từ chối kết nối (sập hẳn) → 404.
   Uplink 401/403/418/500 → 404.
2. **Thử lại** (metadata): lỗi mạng, timeout và 5xx (đã thấy với 500, 503) được thử lại 2 lần
   (3 request, backoff ~1 s rồi ~2 s), kể cả khi có `If-None-Match`. 4xx không thử lại.
   Tarball gặp HTTP 503 chỉ có 1 request (không thử lại).
3. **Ngắt mạch**: `failed_requests` = số lần thử lại của request gần nhất, nên với mặc định
   `max_fails: 2` **một** request metadata thất bại hoàn toàn là đủ mở mạch. Trong `fail_timeout`,
   metadata không lên uplink (có cache → trả cache; không có → 404); tarball không đi qua ngắt mạch.
4. **Tarball khi uplink lỗi**: uplink 404 → 404 `file doesn't exist on uplink`; uplink 503 → 500
   `bad uplink status code: 503`; uplink sập → 500 `internal server error`. Tên file không có trong
   metadata → 404 `no such file available` (không lên uplink).
5. **Header gửi lên uplink**: `Accept: application/json;` (có dấu `;` thừa), `Accept-Encoding: gzip`,
   `User-Agent: npm (<user_agent>)` — mặc định `npm (hidden)`, `Via: 1.1 <server_id> (Verdaccio)`,
   `X-Forwarded-For: <ip client>` cho metadata nhưng **chuỗi `undefined`** cho tarball;
   có etag thì `If-None-Match` và `Accept` bị ép lại `application/json;`. Tên scoped gửi dạng `/@s%2Fp`.
6. **Header response**: `X-Powered-By: <user_agent>` (mặc định `hidden`), `Access-Control-Allow-Origin: *`,
   `Vary: Accept-Encoding`, nén gzip khi client xin. Tarball không có ETag. **Lỗi vẫn có ETag**
   (ETag yếu Express `W/"len-sha1"`), dù `final.ts` ghi "don't send etags with errors". Client
   `If-None-Match` khớp (kể cả `W/"…"` và `*`) → 304 cho metadata, version, ping.
7. **Uplink trả 404 cho package đã cache**: vẫn phục vụ cache, nhưng `fetched` không được cập nhật nên
   **mọi** request sau đó đều hỏi lại uplink.
8. **Upstream gỡ version / dist-tag**: Verdaccio giữ version và dist-tag cũ trong cache (merge chỉ thêm
   và ghi đè từng tag, không xóa). `cache: false` vẫn ghi `package.json`, chỉ không ghi tarball.
9. **Route lạ**: `GET /-/<x>` bị hiểu là package `-` (gửi `GET /-` lên uplink, trả 404);
   `/npm/simple-pkg` là package `npm` version `simple-pkg`. `url_prefix` chỉ đổi URL sinh ra, route vẫn
   ở gốc. `GET /` → 404 `Web interface is disabled…`. `POST`/`PATCH /:package` → 404 HTML của Express;
   `PUT` không auth → 401 JSON. `OPTIONS` → 204. `HEAD` tarball tải và cache tarball từ uplink.
   `Host` không hợp lệ hoặc rỗng → 400 `Invalid URL: …`. Version `v1.0.0` được chuẩn hóa thành `1.0.0`;
   `^1.0.0` → 400 `invalid version`; `1.x` → 404.
10. `GET /.hidden` (đoạn path bắt đầu bằng `.`) treo vô hạn.
11. Storage: Verdaccio tạo `.verdaccio-db.json` khi khởi động; nếu file đã có thì `secret` phải đúng
    32 ký tự, không thì Verdaccio thoát.
