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
node bench/run.mjs                  # đầy đủ, ~1 giờ
node bench/run.mjs --quick          # thử nhanh, ~4 phút (1 lần lặp, cửa sổ 2 s)
node bench/run.mjs --only s1,s5 --servers fastopm,verdaccio
node bench/profile.mjs abbr-ms abbr-ts tarball-50   # macOS `sample` trên fastopm dưới tải
node bench/lib/report.mjs bench/results/latest.json # in lại bảng từ JSON
```

Kịch bản: `s1` metadata rút gọn (gzip), `s1i` rút gọn không nén, `s2` metadata đầy đủ, `s3` 304,
`s4` tarball, `s5` cache lạnh, `s6` hết `maxage`, `s7` tài nguyên, `checks` kiểm tra thiết kế.
Server: `fastopm`, `verdaccio`, `fastopm-1t` (fastopm với `TOKIO_WORKER_THREADS=1`, chỉ chạy `s1`).

Mỗi lần chạy: một mock uplink (`lib/uplink.mjs`, tiến trình riêng) phục vụ packument thật
(`fixtures/*.json.gz`, lấy từ registry.npmjs.org) và tarball sinh ngẫu nhiên; Verdaccio nạp storage
mẫu từ mock uplink; mỗi server chạy lần lượt trên một bản sao của storage đó trong `bench/.work/`
(xóa khi xong, trừ khi `--keep`). Lấy lại fixture: xóa `fixtures/*.json.gz` rồi chạy
`node bench/fetch-fixtures.mjs`.
