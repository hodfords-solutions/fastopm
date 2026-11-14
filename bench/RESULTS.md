# fastopm vs verdaccio@7.1.0 — kết quả benchmark

Lần đo: 2026-11-14, một máy, hai server chạy lần lượt (không cùng lúc). Số thô:
[`results/latest.json`](results/latest.json), bảng sinh tự động: [`results/latest.md`](results/latest.md),
log: [`results/full-run.log`](results/full-run.log), profile: [`results/profile/`](results/profile/).
Chạy lại: `node bench/run.mjs` (≈75 phút trên máy này), `node bench/run.mjs --quick` (≈4 phút),
`node bench/profile.mjs`; chuẩn bị xem [README.md](README.md).

## Tóm tắt

- **Metadata cache nóng: fastopm nhanh hơn 20× đến ~1000× về RPS**, tăng theo cỡ packument, vì
  Verdaccio đọc + parse `package.json`, dựng lại body, MD5 và gzip ở mỗi request trên một luồng JS
  còn fastopm trả bytes đã cache. Package nhỏ (`ms`): 56–59k RPS so với ~840 (66–71×); vừa
  (`express`): ~34k so với ~95 (~360×); lớn (`typescript`, 16 MB): ~5k so với 5 (~1000×). Ngay cả
  khi khóa fastopm ở 1 worker thread vẫn nhanh hơn 38–880×. Tính theo CPU: 10.7k so với 574 request
  mỗi giây-CPU với `ms` (19×), 6.2k so với 67 với `express` (92×).
- **304 (`If-None-Match` khớp): fastopm ~55.7k RPS bất kể cỡ package; Verdaccio vẫn dựng nguyên body**
  (304 của `typescript` tốn 201 ms, gần bằng 200 OK 213 ms) → 57× / 554× / ~10 000×.
- **Tarball từ đĩa: nhanh hơn 2.8–4.4×** (7.9 GB/s so với 1.8 GB/s với 1 MiB, c=16), tốn ít CPU
  mỗi byte hơn ~2×. Không phải zero-copy.
- **Gộp request trùng hoạt động**: 100 client cùng hỏi một package chưa cache → 1 request lên uplink
  (Verdaccio: 100), xong trong 84 ms so với 1 437 ms; tarball 1 vs 100 request, 101 vs 250 ms.
  Khi hết `maxage`, 32 client đồng thời → 0.031 request uplink cho mỗi request client (Verdaccio ~1.0),
  RPS 19–23× (uplink trả ngay) hoặc 2–10× (uplink trễ 20 ms).
- **Chỗ không nhanh hơn**: cache lạnh, *một* client, lần lượt 100 package khác nhau (cỡ `express`):
  **fastopm chậm hơn ~24%** (24.1 ms so với 19.4 ms mỗi package). Revalidate tuần tự (c=1) với
  `express`: ngang Verdaccio (16.6 so với 15.8 ms). Đây là đường merge/ghi/render, xem điểm nóng #3.
- **Tài nguyên**: khởi động 16 ms so với 411 ms (25×); RSS lúc nghỉ 9.4 MiB so với 139 MiB (15×);
  giữ `typescript` trong RAM ~160 MB so với ~240 MB của Verdaccio (đo bằng `footprint`); dưới tải
  Verdaccio lên tới 3.8 GB RSS. Binary 12.3 MB so với 57 MB `node_modules` + 122 MB `node`.
- **Điểm nóng lớn nhất của fastopm**: mỗi request metadata gọi `tokio::fs::metadata` (qua
  `spawn_blocking`) để kiểm tra file có đổi không → tranh chấp mutex của blocking pool của Tokio
  chiếm phần lớn thời gian ở request nhỏ. Xem phần "Điểm nóng và đề xuất".

## Máy và cách đo

| item | value |
|---|---|
| CPU | Apple M1 Pro, 10 cores (8 performance + 2 efficiency) |
| RAM | 32 GiB |
| OS | macOS 26.3 (25.3.0) |
| Node | v24.20.0 |
| Rust | rustc 1.100.0 (0ef1bc386 2026-11-09) |
| load generator | oha 1.16.0 |
| Verdaccio | 7.1.0 |
| fastopm binary | bench/target/release/fastopm (built 2026-11-14T04:57:32.623Z) |
| date | 2026-11-14T05:15:38.663Z |
| options | warm-up 3s, 3 × 10s per cell |

Nền máy: macOS desktop đang dùng (WindowServer, trình duyệt…) và **các session khác cùng chạy trên
máy** (một `npm install` ~300% CPU lúc bắt đầu). Script chờ tới khi CPU của tiến trình ngoài
< 150% (hai mẫu liên tiếp, tối đa 3 phút) trước mỗi lần lặp và ghi CPU ngoài trong lúc đo; ô có
lúc vượt 150% được đánh dấu ⚠ (gần như mọi ô của fastopm — thực tế nền máy hiếm khi xuống dưới
~120%). Xem "Độ tin cậy" bên dưới.

- **Uplink**: mock HTTP chạy cục bộ, tiến trình Node riêng ([`lib/uplink.mjs`](lib/uplink.mjs)).
  Phục vụ packument thật lấy một lần từ registry.npmjs.org và lưu nén trong `fixtures/`:
  `ms` (92 KB đầy đủ), `express` (824 KB), `typescript` (15.9 MB, ~3 500 version); `dist.tarball`
  viết lại về mock. Tarball 1 MiB và 50 MiB là byte ngẫu nhiên (không nén được) sinh lúc chạy, trong
  package giả `bench-tarball`. Package cache lạnh `bench-cold-*` là bản sao packument `express` đổi
  tên. ETag = MD5 body, `If-None-Match` khớp → 304. Độ trễ uplink chỉnh được (0 ms mặc định; 50 ms ở
  S5 “cùng lúc”; 0/20 ms ở S6).
- **Storage**: mỗi lần chạy, Verdaccio nạp `ms`, `express`, `typescript` và hai tarball từ mock vào
  một storage mẫu; mỗi server chạy trên bản sao riêng của storage mẫu đó, với
  `_uplinks.npmjs.fetched` đặt lại thành thời điểm copy. fastopm đọc thẳng storage do Verdaccio ghi.
- **Tải**: [`oha`](https://github.com/hatoo/oha) 1.16 (Rust, đa luồng, cài bằng `cargo install` vào
  `bench/.tools`), keep-alive, HTTP/1.1, cùng máy với server. Mỗi ô: warm-up 3 s, chờ server hết bận
  (CPU < 10% một core), rồi 3 lần × 10 s; báo **trung vị**, `±x%` = nửa khoảng (max−min)/trung vị
  của RPS qua 3 lần. Ô quá chậm để cửa sổ 10 s có ý nghĩa (chỉ Verdaccio với `express` c=128 và
  `typescript`) chạy theo số request cố định, ghi `(n=…)`, mỗi lần tối đa ~2 phút. CPU = thời gian
  CPU (user+sys, `ps`) của tiến trình server chia thời gian đo; 100% = một core.
- **Header** như npm gửi: rút gọn `Accept: application/vnd.npm.install-v1+json; q=1.0,
  application/json; q=0.8, */*`; đầy đủ `Accept: application/json`; `Accept-Encoding: gzip,deflate`
  (trừ S1i).
- **Config** ([`lib/servers.mjs`](lib/servers.mjs) `configYaml`): cùng một file cho hai server, dựa
  trên `conf/default.yaml` của Verdaccio 7.1.0. Khác mặc định:

  | key | mặc định Verdaccio | dùng ở đây | lý do |
  |---|---|---|---|
  | `uplinks.npmjs.url` | `https://registry.npmjs.org/` | mock cục bộ | không đo qua Internet |
  | `uplinks.npmjs.maxage` | `2m` | `30m` (S1–S5, S7), `1ms` (S6) | một ô đo dài hơn 2 phút; S6 cố ý làm mọi request phải revalidate |
  | `web.enable` | `true` | `false` | fastopm không có UI |
  | `middlewares.audit.enabled` | `true` | `false` | fastopm không có audit |
  | `server.rateLimit` | `{windowMs: 1000, max: 10000}` | `max: 1000000000` | mặc định chặn >10k req/s |
  | `filters` (`@verdaccio/package-filter`), `i18n` | có | bỏ | fastopm không hỗ trợ; bỏ chỉ có thể làm Verdaccio nhanh hơn |
  | `storage` | `./storage` | đường dẫn tuyệt đối tới bản sao | — |

  Giữ nguyên: `log: {type: stdout, format: pretty, level: http}` (cả hai log mọi request; stdout
  của server trỏ vào `/dev/null`), `server.keepAliveTimeout: 60`, `packages`, `auth`. Verdaccio chạy
  với `NODE_ENV=production`. fastopm chạy với số worker mặc định của Tokio (10); `fastopm 1T` là cùng
  binary với `TOKIO_WORKER_THREADS=1` (vẫn có blocking pool), để so “cùng một luồng sự kiện” với Node.
- **Binary**: `CARGO_TARGET_DIR=bench/target cargo build --release --locked -p fastopm` (profile
  release mặc định), từ cây `crates/` lúc 11:57 (+07). Thư mục không phải git repo và một session
  khác đang sửa `crates/`, nên số này ứng với mã nguồn tại thời điểm build đó.

## Kết quả

`f / V` = fastopm / Verdaccio. “ratio” = RPS fastopm ÷ RPS Verdaccio.

### S1 — abbreviated metadata (`Accept: install-v1`, `Accept-Encoding: gzip,deflate`), warm cache

| package | c | RPS fastopm | RPS Verdaccio | ratio | p50 ms f / V | p99 ms f / V | CPU f / V | req per CPU-s f / V | RPS fastopm 1T | 1T / V |
|---|---|---|---|---|---|---|---|---|---|---|
| ms | 1 | 9,530 ±3% ⚠ | 476 ±4% ⚠ | 20× | 0.090 / 2.00 | 0.220 / 4.15 | 47% / 120% | 20,170 / 397 | 10,761 ±3% | 23× |
| ms | 16 | 55,776 ±20% ⚠ | 844 ±1% ⚠ | 66× | 0.260 / 18.6 | 0.690 / 25.0 | 523% / 147% | 10,672 / 574 | 32,516 ±0% | 39× |
| ms | 128 | 58,781 ±2% | 824 ±0% ⚠ | 71× | 2.04 / 152 | 5.52 / 192 | 529% / 154% | 10,983 / 536 | 31,663 ±1% | 38× |
| express | 1 | 7,187 ±18% ⚠ | 70 ±1% | 102× | 0.125 / 13.9 | 0.263 / 18.1 | 54% / 104% | 13,213 / 68 | 8,301 ±1% | 118× |
| express | 16 | 34,817 ±1% | 95 ±1% | 367× | 0.398 / 166 | 1.32 / 200 | 562% / 141% | 6,197 / 67 | 23,092 ±1% | 243× |
| express | 128 | 33,508 ±8% ⚠ | 94 ±1% (n=922) ⚠ | 357× | 3.53 / 1310 | 9.70 / 5567 | 569% / 155% | 6,062 / 61 | 22,232 ±1% | 237× |
| typescript | 1 | 2,775 ±2% ⚠ | 4 ±1% (n=44) | 647× | 0.346 / 233 | 0.626 / 253 | 83% / 112% | 3,357 / 4 | 3,771 ±3% | 880× |
| typescript | 16 | 5,212 ±2% ⚠ | 5 ±0% (n=55) | 975× | 3.01 / 2993 | 4.77 / 3028 | 312% / 156% | 1,668 / 3 | 4,691 ±1% | 877× |
| typescript | 128 | 4,337 ±3% ⚠ | 5 ±1% (n=384) ⚠ | 823× | 29.6 / 24174 | 53.2 / 35635 | 244% / 188% | 1,779 / 3 | 3,628 ±13% | 688× |

Bytes per response (as sent): ms: fastopm 12,001 B, Verdaccio 12,032 B; express: fastopm 74,303 B, Verdaccio 75,186 B; typescript: fastopm 1,782,751 B, Verdaccio 1,787,532 B

### S1i — abbreviated metadata without `Accept-Encoding` (no compression), warm cache, c=16

| package | c | RPS fastopm | RPS Verdaccio | ratio | p50 ms f / V | p99 ms f / V | CPU f / V | req per CPU-s f / V |
|---|---|---|---|---|---|---|---|---|
| ms | 16 | 53,634 ±6% ⚠ | 879 ±2% ⚠ | 61× | 0.253 / 17.8 | 0.895 / 23.9 | 439% / 117% | 12,071 / 757 |
| express | 16 | 18,613 ±1% ⚠ | 99 ±1% ⚠ | 188× | 0.827 / 160 | 1.63 / 195 | 298% / 108% | 6,235 / 92 |
| typescript | 16 | 591 ±3% ⚠ | 6 ±1% (n=55) | 106× | 26.6 / 2724 | 37.6 / 3137 | 173% / 126% | 349 / 4 |

Bytes per response (as sent): ms: fastopm 40,731 B, Verdaccio 40,731 B; express: fastopm 506,585 B, Verdaccio 506,585 B; typescript: fastopm 11,988,194 B, Verdaccio 11,988,194 B

### S2 — full metadata (`Accept: application/json`, gzip), warm cache

| package | c | RPS fastopm | RPS Verdaccio | ratio | p50 ms f / V | p99 ms f / V | CPU f / V | req per CPU-s f / V |
|---|---|---|---|---|---|---|---|---|
| ms | 1 | 9,979 ±0% ⚠ | 310 ±0% ⚠ | 32× | 0.090 / 3.09 | 0.189 / 6.47 | 48% / 110% | 20,795 / 282 |
| ms | 16 | 51,837 ±3% ⚠ | 607 ±1% | 85× | 0.280 / 25.6 | 0.753 / 37.1 | 474% / 168% | 10,946 / 361 |
| express | 1 | 7,423 ±0% ⚠ | 57 ±1% | 131× | 0.124 / 17.1 | 0.243 / 22.4 | 53% / 111% | 13,997 / 52 |
| express | 16 | 32,771 ±0% ⚠ | 82 ±1% ⚠ | 397× | 0.427 / 189 | 1.38 / 289 | 509% / 147% | 6,420 / 56 |
| typescript | 1 | 2,439 ±1% ⚠ | 3 ±0% (n=33) | 711× | 0.397 / 290 | 0.666 / 314 | 85% / 107% | 2,857 / 3 |
| typescript | 16 | 4,440 ±0% ⚠ | 5 ±2% (n=48) | 978× | 3.61 / 3476 | 5.00 / 3682 | 311% / 165% | 1,426 / 3 |

Bytes per response (as sent): ms: fastopm 18,437 B, Verdaccio 18,343 B; express: fastopm 96,670 B, Verdaccio 97,101 B; typescript: fastopm 2,127,839 B, Verdaccio 2,133,399 B

### S3 — abbreviated metadata with matching `If-None-Match` (304), c=16

| package | c | RPS fastopm | RPS Verdaccio | ratio | p50 ms f / V | p99 ms f / V | CPU f / V | req per CPU-s f / V |
|---|---|---|---|---|---|---|---|---|
| ms | 16 | 55,738 ±1% ⚠ | 979 ±0% ⚠ | 57× | 0.251 / 16.1 | 0.806 / 21.5 | 509% / 113% | 10,943 / 862 |
| express | 16 | 55,759 ±1% ⚠ | 101 ±4% ⚠ | 554× | 0.253 / 154 | 0.798 / 250 | 514% / 104% | 10,804 / 98 |
| typescript | 16 | 55,585 ±1% ⚠ | 5 ±3% (n=54) ⚠ | 10279× | 0.256 / 2859 | 0.789 / 3081 | 523% / 120% | 10,664 / 5 |

Bytes per response (as sent): ms: fastopm 0 B, Verdaccio 0 B; express: fastopm 0 B, Verdaccio 0 B; typescript: fastopm 0 B, Verdaccio 0 B

### S4 — tarball from disk

| package | c | RPS fastopm | RPS Verdaccio | ratio | MB/s fastopm | MB/s Verdaccio | p50 ms f / V | p99 ms f / V | CPU f / V | req per CPU-s f / V |
|---|---|---|---|---|---|---|---|---|---|---|
| 1MiB | 1 | 2,698 ±2% ⚠ | 734 ±6% ⚠ | 3.7× | 2,829 | 769 | 0.361 / 1.24 | 0.576 / 3.48 | 100% / 138% | 2,701 / 532 |
| 1MiB | 16 | 7,520 ±1% ⚠ | 1,698 ±2% | 4.4× | 7,885 | 1,780 | 1.85 / 8.67 | 6.58 / 14.4 | 466% / 228% | 1,613 / 744 |
| 50MiB | 1 | 80 ±1% ⚠ | 21 ±3% ⚠ | 3.9× | 4,177 | 1,080 | 12.5 / 46.6 | 15.3 / 84.6 | 153% / 130% | 52 / 16 |
| 50MiB | 8 | 128 ±1% ⚠ | 46 ±4% ⚠ | 2.8× | 6,693 | 2,416 | 67.4 / 170 | 78.2 / 221 | 366% / 250% | 35 / 18 |

### S5 — cold cache

| server | case | wall ms (median) | client latency ms | uplink requests (per rep) | statuses |
|---|---|---|---|---|---|
| fastopm | same package × 100 at once (metadata) | 83.7 | 83.3 | 1, 1, 1 | {"200":300} |
| fastopm | same tarball × 100 at once (1 MiB) | 101 | 99.4 | 1, 1, 1 | {"200":300} |
| fastopm | 100 distinct packages, 1 client | 2412 | p50 23.6 / p99 30.4 | 100 | {"200":100} |
| fastopm | 100 distinct packages, 16 clients | 717 | p50 101 / p99 239 | 100 | {"200":100} |
| Verdaccio | same package × 100 at once (metadata) | 1437 | 1436 | 100, 100, 100 | {"200":300} |
| Verdaccio | same tarball × 100 at once (1 MiB) | 250 | 247 | 100, 100, 100 | {"200":300} |
| Verdaccio | 100 distinct packages, 1 client | 1945 | p50 18.7 / p99 25.4 | 100 | {"200":100} |
| Verdaccio | 100 distinct packages, 16 clients | 1272 | p50 194 / p99 258 | 100 | {"200":100} |

S5 “cùng lúc”: uplink trễ 50 ms, 100 kết nối mở sẵn, 3 lần với tên package khác nhau, cột uplink là
số request lên uplink cho đúng package / tarball đó ở từng lần. Với tarball, manifest đã được hỏi
trước (một request) nên chỉ đo phần tarball. “distinct”: uplink không trễ, 100 package `express`
khác nhau.

### S6 — `maxage` expired (every request revalidates, uplink answers 304)

| package | c | uplink delay ms | RPS fastopm | RPS Verdaccio | ratio | p50 ms f / V | p99 ms f / V | uplink req per client req f / V | CPU f / V |
|---|---|---|---|---|---|---|---|---|---|
| ms | 1 | 0 | 448 ±3% | 284 ±2% | 1.6× | 2.17 / 3.43 | 2.76 / 6.21 | 1.000 / 1.000 | 92% / 125% |
| ms | 32 | 0 | 10,879 ±1% | 564 ±4% | 19× | 2.94 / 55.1 | 3.78 / 78.2 | 0.031 / 1.006 | 347% / 161% |
| ms | 32 | 20 | 1,232 ±1% | 600 ±4% | 2.1× | 25.9 / 52.8 | 30.1 / 65.7 | 0.031 / 1.005 | 66% / 161% |
| express | 1 | 0 | 60 ±2% | 62 ±1% | 1.0× | 16.6 / 15.8 | 20.3 / 22.2 | 1.002 / 1.002 | 97% / 102% |
| express | 32 | 0 | 1,775 ±26% | 79 ±0% | 23× | 17.7 / 398 | 25.4 / 503 | 0.031 / 1.041 | 487% / 151% |
| express | 32 | 20 | 809 ±1% | 80 ±0% | 10× | 39.1 / 395 | 50.9 / 471 | 0.031 / 1.040 | 220% / 151% |

S6: `maxage: 1ms` nên request nào cũng phải hỏi lại uplink (`If-None-Match`, uplink trả 304), rồi
server ghi lại `package.json` để cập nhật `fetched` (cả hai server đều ghi). Ở c=32, fastopm gộp các
lần revalidate đang chạy dở nên chỉ ~3% request client đi lên uplink.

### S7 — resources

| metric | fastopm | Verdaccio |  |
|---|---|---|---|
| start-up to first `/-/ping` 200 (ms, median of runs) | 16.3 | 411 | 25× faster |
| RSS idle, empty storage (MiB) | 9.4 | 138.8 | 15× smaller |
| RSS idle after start, seeded storage (MiB) | 9.4 | 132.2 |  |
| RSS peak under load, typescript abbreviated (S1) (MiB) | 21.4 | 3810.7 |  |
| RSS peak under load, typescript full (S2) (MiB) | 226.3 | 1627.8 |  |
| RSS peak under load, 50 MiB tarball (S4) (MiB) | 162.6 | 678.1 |  |
| RSS idle after the warm scenarios (MiB) | 162.6 | 401.5 |  |
| uplink requests during all warm scenarios | 0 | 0 |  |
| on-disk size | 12.3 MB binary | 57 MB node_modules + 122 MB node |  |

Bộ nhớ đo lại bằng `footprint` (macOS, "physical footprint", tin cậy hơn RSS của `ps`), một client,
storage mẫu:

| trạng thái | fastopm | Verdaccio |
|---|---|---|
| nghỉ, sau khởi động | 2.5 MB (RSS 9 MiB) | 93 MB (RSS 139 MiB) |
| sau một GET rút gọn `typescript` | 159 MB (RSS 165 MiB) | 239 MB (RSS 272 MiB) |
| thêm một GET đầy đủ `typescript` | 182 MB (RSS 189 MiB) | 388 MB (RSS 418 MiB) |

### Design checks — single client, `typescript` full manifest, no compression (ms, median)

| server | same request again | new Host each time | package.json changed before each | 304 | body bytes |
|---|---|---|---|---|---|
| fastopm | 10.1 | 76.9 | 194 (change seen 10/10) | 0.198 | 21,992,682 |
| Verdaccio | 213 | 215 | 209 (change seen 10/10) | 201 | 21,992,682 |

## Nhận xét

### Chỗ fastopm nhanh hơn, và bao nhiêu

- **Metadata cache nóng (S1, S2)**: ở c=1 20× (`ms`), ~100× (`express`), ~650× (`typescript`); ở
  c=16–128 66–400×, ~1000× với packument 16 MB.
  Verdaccio bão hòa ở một core (CPU 110–190%, phần trên 100% là GC và thread pool của libuv/zlib)
  và tốn ~3 ms (`ms`), ~10 ms (`express`), ~200–290 ms (`typescript`) CPU mỗi request; ở c=128 với
  `typescript`, p50 của Verdaccio là 24 s — client npm thật sẽ timeout. fastopm ở c=1 trả `ms` trong
  0.09 ms. Với `typescript` fastopm bị chặn bởi băng thông loopback (≈1.8 MB gzip × 5 200 = 9 GB/s;
  profile: 91% thời gian on-CPU là `writev`).
- **1 worker thread** (`fastopm 1T`): 31–32k RPS với `ms`, 22–23k với `express` — 38× và ~240×
  Verdaccio, nên lợi thế chủ yếu đến từ việc không làm lại việc, không phải từ đa luồng. Đa luồng
  (10 worker) chỉ thêm ~1.8× với package nhỏ (55–59k), xem điểm nóng #1 vì sao không hơn.
- **Không nén (S1i)**: lợi thế giảm với body lớn (`typescript` 106× thay vì ~1000×) vì cả hai bị
  chặn bởi việc chép 12 MB mỗi response; Verdaccio không còn tốn gzip nhưng vẫn parse + stringify.
- **304 (S3)**: fastopm trả 304 mà không đụng tới body (0.2 ms ở một client, ~55.7k RPS mọi cỡ);
  Verdaccio dựng body, tính ETag rồi Express mới so `If-None-Match`, nên 304 gần như không rẻ hơn 200.
- **Tarball (S4)**: 2.8–4.4× throughput, 2× ít CPU mỗi GB (0.55–0.6 core-giây/GB so với 1.0–1.3).
- **Gộp request (S5, S6)**: đúng như thiết kế — 1 request uplink cho 100 client, kể cả tarball.
- **Khởi động và RAM lúc nghỉ**: 25× và 15× (RSS), ~37× theo footprint.

### Chỗ fastopm không nhanh hơn hoặc chậm hơn

- **Cache lạnh, một client, nhiều package khác nhau (S5 distinct, c=1)**: fastopm 2.41 s so với
  1.94 s cho 100 package (chậm hơn ~24%). Với 16 client fastopm nhanh hơn 1.8× (0.72 s so với
  1.27 s) nhưng dùng nhiều core; p50 mỗi request vẫn ~100 ms. Profile (`cold-distinct`): parse
  packument 23%, `update_manifest` (merge + serialize compact + ghi) 27%, clone sâu `IndexMap`
  17%, gzip 15%, malloc/free ~25% self-time, cộng tranh chấp mutex ở cache manifest.
- **Revalidate một client (S6 c=1)**: `ms` 1.6× nhanh hơn, `express` ngang (60 so với 62 RPS).
  Mỗi revalidate fastopm ghi file, đổi “generation” rồi render lại + MD5 + gzip lại body dù body rút
  gọn không đổi (profile `revalidate`: 62% gzip, 9% MD5, 22% render).
- **Thêm kết nối không thêm RPS**: `ms` 56k (c=16) → 59k (c=128); `express` 35k → 34k; p99 tăng
  0.7 → 5.5 ms. Một phần do oha chạy cùng máy (2–3 core), một phần do điểm nóng #1.
- **fastopm 1T nhanh hơn bản 10 worker ở c=1** (`typescript` 3.8k so với 2.8k; `ms` 10.8k so với
  9.5k): ở một client, chi phí chuyển việc giữa các thread (blocking pool cho `stat`, đánh thức
  worker khác) lớn hơn lợi ích song song.
- **Bộ nhớ cho packument lớn**: `typescript` (16 MB JSON) chiếm ~150 MB trong fastopm (≈9–10× kích
  thước file, do `serde_json::Value` với `preserve_order`), ít hơn Verdaccio nhưng không nhỏ — và
  giới hạn cache manifest (256 MiB) đang đếm theo *kích thước file*, xem đề xuất #6.

### Phép đo nào chưa đáng tin và vì sao

- **Máy không yên**: CPU của tiến trình ngoài 120–390% trong hầu hết các ô (⚠). Ảnh hưởng nhiều nhất
  tới fastopm (dùng 3–6 core): `ms` c=16 dao động ±20% (34k–56k giữa các lần), `express` c=1 ±18%,
  S6 `express` c=32 ±26%. Verdaccio (một luồng) ít bị ảnh hưởng hơn (±0–7%). Các tỉ lệ ở trên nên
  hiểu là đúng bậc độ lớn, chênh ±20% là trong nhiễu.
- **Công cụ tạo tải chung máy**: oha dùng 2–3 core. Với package nhỏ fastopm (≈5.5 core) + oha gần
  hết 10 core, nên RPS của fastopm ở S1/S2/S3 là cận dưới; Verdaccio không bị giới hạn bởi điều này.
- **Body lớn đo băng thông loopback** (S1/S2 `typescript`, S4): ~7–9 GB/s là trần memcpy của kernel,
  không phải giới hạn của server; trên mạng thật, NIC sẽ là trần trước.
- **RSS đỉnh trong bảng S7 (đo bằng `ps` mỗi 100 ms) không tin được với fastopm**: “21 MiB đỉnh khi
  phục vụ `typescript`” là sai — macOS nén/đẩy trang ra khi máy chịu tải từ session khác, `ps rss`
  không tính các trang đó. Bảng `footprint` ở trên là số đúng hơn (159 MB). Số của Verdaccio (GB) vẫn
  cho đúng bậc.
- **Ô Verdaccio chạy theo số request cố định** (`n=…`, ví dụ `typescript` c=1 chỉ 33–55 request mỗi
  lần): RPS đáng tin nhưng p99 dựa trên ít mẫu.
- **S6 với `maxage: 1ms` là nhân tạo**: thực tế (`maxage: 2m`) mỗi package chỉ revalidate một lần
  mỗi 2 phút; số c=1 cho biết chi phí *một* lần revalidate, số c=32 cho biết hiệu quả gộp.
- **S1i lần đầu bị sai** (oha tự gửi `Accept-Encoding`, cả hai server vẫn nén); đã chạy lại riêng S1i
  với `--disable-compression` lúc 06:34 UTC và ghép vào `latest.json` (có ghi chú trong file).
- p99 ở c=1 (< 1 ms với fastopm) chủ yếu là độ trễ phía client.
- Không đo kịch bản PLAN “cài 20 cây dependency lớn” (cần client npm thật và registry thật/hoặc
  fixture rất lớn); S5 distinct + S1/S4 là phần thay thế gần nhất.

## Kiểm tra các điểm thiết kế của PLAN.md §3

| Điểm thiết kế | Có cài đặt? | Có tác dụng? | Bằng chứng |
|---|---|---|---|
| Cache manifest đã parse trong RAM, kiểm mtime + size | Có (`fastopm-store/src/cache.rs`, `Inner::load_local`) | **Có**, nhưng việc kiểm `stat` mỗi request là điểm nóng #1 | Checks: Host mới mỗi lần (phải render lại, không parse) 77 ms; file bị sửa trước mỗi lần (parse + render) 194 ms → parse ~117 ms được tránh; thay đổi trên đĩa được thấy 10/10 lần. Profile hot path không có `serde_json` parse. Giới hạn 256 MiB đếm theo kích thước file, RAM thực ≈9–10× (đề xuất #6). |
| Cache bytes + ETag + gzip theo biến thể và base URL | Có (`fastopm-server/src/cache.rs`) | **Có** khi nội dung ổn định; **mất tác dụng ở mỗi lần ghi** (revalidate) dù body không đổi | Checks: cùng request 10 ms (chủ yếu là client đọc 22 MB) so với 77 ms khi phải render; profile `abbr-ts`/`full-ts`: 91–93% là `writev`, không có render/MD5/gzip. Profile `revalidate`: 62% gzip + 9% MD5 + 22% render (đề xuất #2). Body > 32 MB (pretty) không bao giờ vào cache (ngân sách 64 MiB, chi phí = 2× body). |
| 304 không dựng lại body | Có (`finish_json` so ETag trước khi nén/gửi) | **Có** (khi body đã ở cache) | S3: ~55.7k RPS với cả `ms` lẫn `typescript`; checks: 0.2 ms so với 201 ms của Verdaccio. Khi cache render miss vẫn phải render để biết ETag. |
| Gộp request trùng lên uplink (metadata và tarball) | Có (`Inner::refresh`, `start_download`) | **Có** | S5: 1 request uplink cho 100 client (metadata và tarball), Verdaccio 100; S6: 0.031 request uplink / request client. |
| Parse trên thread pool, nhiều core | Một phần | Một phần | Parse body uplink (`fastopm-uplink` `parse_manifest`) và đọc/ghi `package.json` chạy trên blocking pool. Nhưng merge (clone sâu manifest, `fastopm-store/src/lib.rs:438`) và render (`fastopm-server/src/lib.rs:364`, ~67 ms với `typescript`) chạy ngay trên worker Tokio, chặn worker đó. |
| Gửi tarball local zero-copy | **Không** | — | `fastopm-storage/src/lib.rs:361` dùng `ReaderStream` trên `tokio::fs::File` (khối 256 KiB, mỗi lần đọc là một `spawn_blocking`); profile `tarball-50`: `read` 27% + `memmove` 8% + `writev` 53%, 1.5 thread chờ mutex blocking pool. Vẫn nhanh hơn Verdaccio 2.8–4.4×. |

## Điểm nóng và đề xuất tối ưu

Profile: `node bench/profile.mjs` chạy `sample` (1 ms) trên fastopm dưới tải, 8 s mỗi trường hợp;
tóm tắt trong `results/profile/<case>.md` (self time, inclusive, ai đang chờ mutex). Lưu ý:
`sample` làm chậm tiến trình (~30–40% RPS) và macOS báo một thread đang chờ mutex là "không chạy",
nên tôi tách "on-CPU" và "chờ mutex". Xếp theo lợi ích ước tính:

1. **`stat` mỗi request qua blocking pool của Tokio** — lợi ích lớn nhất, áp dụng cho mọi request
   metadata (kể cả 304).
   - Ở đâu: `crates/fastopm-store/src/lib.rs:312` (`Inner::load_local`) gọi
     `PackageStorage::manifest_identity` → `crates/fastopm-storage/src/lib.rs:325`
     (`manifest_identity_in`, `tokio::fs::metadata` = một `spawn_blocking` mỗi request).
   - Bằng chứng (`abbr-ms`, `not-modified`, c=128): `load_local` chiếm ~39% mẫu (on-CPU + chờ
     mutex), trong đó `spawn_blocking(metadata)` 35%; ~26 000 mẫu chờ mutex trong
     `blocking::pool::{Spawner::spawn_task, Inner::run}` — trung bình ~1.5 worker Tokio bị chặn
     chờ khóa của pool, process có 120+ thread. Self time: `__psynch_mutexdrop` + `__psynch_cvsignal`
     ~20% CPU. Cùng lý do: bản 1 worker nhanh hơn bản 10 worker ở c=1, và RPS không tăng từ c=16 lên
     c=128.
   - Đề xuất: gọi `std::fs::metadata` trực tiếp (stat trên inode đã cache ~1–2 µs, rẻ hơn nhiều so với
     một vòng `spawn_blocking`), hoặc tốt hơn: chỉ `stat` lại nếu lần kiểm trước đã cũ hơn một
     khoảng ngắn (ví dụ 1 s; giữ ngữ nghĩa "tự làm mới khi tiến trình khác ghi" với độ trễ tối đa
     1 s), hoặc dùng file watcher. Áp dụng tương tự cho `open_tarball_in`.
   - Ước tính: +30–60% RPS cho metadata nhỏ/vừa và 304, p99 thấp hơn, bớt ~100 thread.

2. **Cache render bị bỏ ở mỗi lần ghi dù body không đổi, và gzip chậm** — lợi ích lớn ở revalidate
   (mỗi package phổ biến mỗi 2 phút) và cache lạnh.
   - Ở đâu: `crates/fastopm-store/src/lib.rs:553` (`write_back` → `remember(.., changed = true)`
     luôn cấp generation mới) và `crates/fastopm-server/src/cache.rs:113` (generation mới xóa mọi
     biến thể của package); nén ở `crates/fastopm-server/src/compress.rs` (flate2 backend
     `miniz_oxide`).
   - Bằng chứng (`revalidate`, `express`, maxage 1 ms): 62% on-CPU trong
     `miniz_oxide::deflate::compress_inner`, 9% MD5, 22% `render_manifest`; trong khi body rút gọn
     không chứa `_rev`/`_uplinks` nên không đổi. S6 c=1 `express`: 16.6 ms, ngang Verdaccio.
     `cold-distinct`: gzip 15–17%.
   - Đề xuất: (a) giữ generation (hoặc một generation riêng cho từng biến thể) khi lần ghi chỉ đổi
     khóa bookkeeping (`_uplinks`, `_distfiles`, `_rev` — `_rev` chỉ có trong body đầy đủ), hoặc so
     MD5 của body mới với body cũ trước khi bỏ bản gzip; (b) bật backend nhanh hơn cho flate2
     (`zlib-rs`) hoặc nén mức thấp hơn cho lần đầu — ETag tính trên body chưa nén nên không ảnh hưởng
     tương thích.
   - Ước tính: revalidate `express` từ ~17 ms xuống vài ms CPU; S6 c=32 tăng nhiều lần; cache lạnh
     −15%.

3. **Đường cache lạnh: clone sâu, giải phóng manifest lớn trong lúc giữ mutex** — chỗ duy nhất
   fastopm chậm hơn Verdaccio.
   - Ở đâu: `crates/fastopm-store/src/lib.rs:440` (`(*l.local).clone()` trước merge), `:347`
     (clone cả manifest chỉ để thay `dist-tags` đã chuẩn hóa), `:359` + `crates/fastopm-store/src/cache.rs:81`
     (`ManifestCache::insert` gỡ/evict entry cũ — tức là drop cả cây `Value` — trong khi giữ mutex),
     `update_manifest_in` (`crates/fastopm-storage/src/lib.rs:333`, đọc lại + parse file hiện có rồi
     serialize).
   - Bằng chứng (`cold-distinct`, 16 client): clone `IndexMap` 17%, `update_manifest` 27%, malloc/free
     ~25% self time; mutex chờ trong `remember`/`load_local` (~6 000 mẫu). S5 distinct c=1: 24.1 ms
     so với 19.4 ms của Verdaccio.
   - Đề xuất: merge trên manifest sở hữu (move từ kết quả parse uplink, không clone bản local), chỉ
     clone `dist-tags` khi cần, drop entry bị thay/evict *sau* khi nhả khóa (trả về từ `insert` rồi
     drop ngoài), tránh parse lại `package.json` trong `update_manifest` khi identity khớp bản trong
     RAM, chuyển merge + render của manifest lớn sang `spawn_blocking`.
   - Ước tính: −20–30% độ trễ cache lạnh một client (ngang hoặc hơn Verdaccio), ít chặn worker hơn.

Các điểm nhỏ hơn:

4. **Log truy cập ghi stdout có khóa**: `crates/fastopm/src/main.rs:169` (`init_logging`,
   `tracing_subscriber::fmt` ghi thẳng `Stdout`). `abbr-ms`: 7% inclusive trong
   `Event::dispatch`, ~2 000 mẫu chờ khóa `Stdout`. Đề xuất: writer không chặn có buffer (ví dụ
   `tracing-appender::non_blocking` — thư viện mới, cần session điều phối đồng ý). Ước tính +5–10%
   RPS ở request nhỏ.
5. **Tarball qua `tokio::fs`** (`crates/fastopm-storage/src/lib.rs:361`): mỗi khối 256 KiB là một
   `spawn_blocking`, 22% inclusive + 1.5 thread chờ mutex ở `tarball-50`. Đề xuất: một task blocking
   cho cả file đọc vào channel có giới hạn, khối 1–4 MiB, hoặc `sendfile` (đúng như PLAN ghi
   "zero-copy"). Ước tính +10–25% throughput, ít CPU hơn.
6. **Ngân sách bộ nhớ**: cache manifest đếm kích thước file (`crates/fastopm-store/src/lib.rs:359`,
   `identity.len`) trong khi `Value` tốn ~9–10× → ngân sách 256 MiB có thể thành ~2.5 GB RAM. Cache
   render (`crates/fastopm-server/src/lib.rs` `RENDER_CACHE_BYTES = 64 MiB`, chi phí = 2× body ở
   `cache.rs:101`) không bao giờ giữ body pretty > 32 MB, nên packument cỡ đó sẽ render lại ở mỗi
   request (~70 ms+ mỗi lần). Đề xuất: ước lượng kích thước thật (hoặc đếm khi parse) và tách ngân
   sách cho body lớn.
7. `would_change_self` (`crates/fastopm-store/src/merge.rs:435`) chạy ở mọi request và clone chuỗi
   readme; ~0.5–1% với `typescript`, không đáng ưu tiên.

## Cấu trúc thư mục

| file | vai trò |
|---|---|
| `run.mjs` | chạy mọi kịch bản, ghi `results/latest.{json,md}` |
| `profile.mjs` | `sample` trên fastopm dưới tải, ghi `results/profile/` |
| `lib/uplink.mjs` | mock uplink |
| `lib/servers.mjs` | config, khởi động/dừng server, `ps` (CPU, RSS, tải ngoài) |
| `lib/load.mjs` | wrapper `oha`, client Node, thống kê |
| `lib/storage.mjs` | storage mẫu do Verdaccio ghi |
| `lib/report.mjs` | bảng markdown từ JSON |
| `fetch-fixtures.mjs`, `fixtures/` | packument thật đã lấy về |
