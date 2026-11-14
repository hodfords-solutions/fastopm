# bench — fastopm vs verdaccio@7.1.0

Kết quả và nhận xét: [RESULTS.md](RESULTS.md). Số thô của lần chạy gần nhất:
`results/latest.json` (bảng sinh tự động: `results/latest.md`), profile: `results/profile/`.

## Chuẩn bị (một lần)

```bash
# fastopm bản release, target riêng của bench
CARGO_TARGET_DIR=bench/target cargo build --release --locked -p fastopm
# công cụ tạo tải và demangle symbol Rust, cài vào bench/.tools (không cần quyền hệ thống)
CARGO_TARGET_DIR=bench/target/tools cargo install oha rustfilt --locked --root bench/.tools
# Verdaccio: dùng bản 7.1.0 đã có trong compat/node_modules (cd compat && npm ci)
```

## Chạy

```bash
node bench/run.mjs                  # đầy đủ, ~75 phút
node bench/run.mjs --quick          # thử nhanh, ~4 phút (1 lần lặp, cửa sổ 2 s)
node bench/run.mjs --only s1,s5 --servers fastopm,verdaccio
node bench/profile.mjs abbr-ms abbr-ts tarball-50   # macOS `sample` trên fastopm dưới tải
node bench/lib/report.mjs bench/results/latest.json # in lại bảng từ JSON
```

Kịch bản: `s1` metadata rút gọn (gzip), `s1i` rút gọn không nén, `s2` metadata đầy đủ, `s3` 304,
`s4` tarball, `s5` cache lạnh, `s6` hết `maxage`, `s7` tài nguyên, `checks` kiểm tra thiết kế.
Server: `fastopm`, `verdaccio`, `fastopm-1t` (fastopm với `TOKIO_WORKER_THREADS=1`, chỉ chạy `s1`).

## Bài test bộ nhớ (`memtest.mjs`)

Tái hiện sự cố OOM: fastopm chạy trong container Docker với giới hạn bộ nhớ cứng
(`--memory X --memory-swap X --cpus 6`), cache lạnh, mock uplink (`lib/uplink.mjs`, container
`node:26.8`) phục vụ bản sao của các packument thật cỡ lớn dưới tên mới mỗi vòng
(`memtest-r<vòng>-<i>--<fixture>`), client (`lib/memload.mjs`, cùng mạng Docker) bắn mọi request của
một vòng cùng lúc (256 song song, cả dạng rút gọn lẫn đầy đủ).

```bash
docker build -t fastopm:memtest .                       # image cần đo (Dockerfile của repo)
node bench/memtest.mjs --image fastopm:memtest --memory 512m,2g --label after
node bench/memtest.mjs --image fastopm:memtest --memory 512m --env RUST_LOG=warn,fastopm::memory=debug --logs /tmp/memlogs
```

Mỗi vòng: 24 packument lớn (3 bản sao mỗi loại: `typescript`, `vite`, `@firebase/firestore`,
`@types/node`, `rollup`, `webpack`, `@aws-sdk/client-s3`, `zod`; 4–39 MB), 60 bản sao `express`,
200 bản sao `ms` → 568 request; 3 vòng lạnh rồi 1 vòng nóng (lặp tên của vòng cuối). Ghi: bị OOM
kill hay không (`State.OOMKilled`, `memory.events`), RSS đỉnh (`VmHWM`), RSS sau khi tải dừng 15 s,
`memory.stat` của cgroup lúc đỉnh, trạng thái và độ trễ từng vòng → `results/memtest-<label>.json`.
Mọi container, mạng và thư mục tạm bị xoá khi xong (cả khi Ctrl-C). Mỗi lần chạy ghi vài GB vào
đĩa của VM Docker; thỉnh thoảng `docker system df` / `docker builder prune` (build Rust để lại
nhiều cache).

Fixture thêm cho bài này: `fixtures/*.json.br` (brotli, ~5.8 MB tổng), lấy bằng
`node bench/fetch-fixtures.mjs`.

## Ghi chú

Mỗi lần chạy: một mock uplink (`lib/uplink.mjs`, tiến trình riêng) phục vụ packument thật
(`fixtures/*.json.gz`, lấy từ registry.npmjs.org) và tarball sinh ngẫu nhiên; Verdaccio nạp storage
mẫu từ mock uplink; mỗi server chạy lần lượt trên một bản sao của storage đó trong `bench/.work/`
(xóa khi xong, trừ khi `--keep`). Lấy lại fixture: xóa `fixtures/*.json.gz` rồi chạy
`node bench/fetch-fixtures.mjs`.
