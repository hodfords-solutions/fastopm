## not-modified

Load: `GET /express` c=128, {"accept":"application/vnd.npm.install-v1+json; q=1.0, application/json; q=0.8, */*","accept-encoding":"gzip,deflate"} + If-None-Match: 33756 req/s, statuses {"304":371444}.

129 threads, 274784 thread-samples over 8s (1 ms interval): 9792 on CPU (≈1.2 cores), 27130 blocked on a mutex (≈3.4 threads), rest idle.

Self time (top of stack, on-CPU only), top 25:

| samples | % of on-CPU | function |
|---:|---:|---|
| 979 | 10.0 | `writev [libsystem_kernel.dylib]` |
| 953 | 9.7 | `__psynch_mutexdrop [libsystem_kernel.dylib]` |
| 930 | 9.5 | `__recvfrom [libsystem_kernel.dylib]` |
| 751 | 7.7 | `__psynch_cvsignal [libsystem_kernel.dylib]` |
| 615 | 6.3 | `stat [libsystem_kernel.dylib]` |
| 457 | 4.7 | `_xzm_xzone_malloc_tiny [libsystem_malloc.dylib]` |
| 446 | 4.6 | `_platform_memmove [libsystem_platform.dylib]` |
| 352 | 3.6 | `_xzm_free [libsystem_malloc.dylib]` |
| 350 | 3.6 | `write [libsystem_kernel.dylib]` |
| 296 | 3.0 | `mach_absolute_time [libsystem_kernel.dylib]` |
| 184 | 1.9 | `thread_start [libsystem_pthread.dylib]` |
| 123 | 1.3 | `_platform_memcmp [libsystem_platform.dylib]` |
| 119 | 1.2 | `<std::path::Components as core::iter::traits::iterator::Iterator>::next [fastopm]` |
| 93 | 0.9 | `<alloc::string::String as core::iter::traits::collect::FromIterator<char>>::from_iter::<core::iter::adapters::map::Map<core::slice::iter::It` |
| 86 | 0.9 | `<fastopm_server::Server<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::handle::{closure#0} [fastopm]` |
| 68 | 0.7 | `_malloc_zone_malloc [libsystem_malloc.dylib]` |
| 68 | 0.7 | `_pthread_mutex_droplock [libsystem_pthread.dylib]` |
| 66 | 0.7 | `xzm_malloc_zone_size [libsystem_malloc.dylib]` |
| 60 | 0.6 | `_free [libsystem_malloc.dylib]` |
| 57 | 0.6 | `core::fmt::write [fastopm]` |
| 55 | 0.6 | `<fastopm_server::router<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>::{closure#0} as axum::handler::Handler<(axum_core::extract::` |
| 55 | 0.6 | `<http::header::name::StandardHeader>::from_bytes [fastopm]` |
| 52 | 0.5 | `<deduplicated_symbol> [libsystem_malloc.dylib]` |
| 52 | 0.5 | `pthread_mutex_lock [libsystem_pthread.dylib]` |
| 51 | 0.5 | `_xzm_xzone_malloc [libsystem_malloc.dylib]` |

Inclusive samples (on CPU + blocked on a mutex) of selected frames, top 40:

| samples | % of on-CPU+mutex | function |
|---:|---:|---|
| 23954 | 64.9 | `<tokio::runtime::task::harness::Harness<axum::serve::handle_connection<tokio::net::tcp::listener::TcpListener, axum::extract::connect_info::` |
| 23927 | 64.8 | `axum::serve::handle_connection::<tokio::net::tcp::listener::TcpListener, axum::extract::connect_info::IntoMakeServiceWithConnectInfo<axum::r` |
| 23911 | 64.8 | `<core::future::poll_fn::PollFn<axum::serve::handle_connection<tokio::net::tcp::listener::TcpListener, axum::extract::connect_info::IntoMakeS` |
| 23838 | 64.6 | `<hyper_util::server::conn::auto::UpgradeableConnection<hyper_util::rt::tokio::TokioIo<tokio::net::tcp::stream::TcpStream>, hyper_util::servi` |
| 23828 | 64.5 | `<hyper::server::conn::http1::UpgradeableConnection<hyper_util::common::rewind::Rewind<hyper_util::rt::tokio::TokioIo<tokio::net::tcp::stream` |
| 21516 | 58.3 | `<hyper::proto::h1::dispatch::Dispatcher<hyper::proto::h1::dispatch::Server<hyper_util::service::glue::TowerToHyperService<tower::util::map_r` |
| 21390 | 57.9 | `<hyper_util::service::oneshot::Oneshot<tower::util::map_request::MapRequest<axum::extension::AddExtension<axum::routing::Router, axum::extra` |
| 20938 | 56.7 | `<fastopm_server::router<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>::{closure#0} as axum::handler::Handler<(axum_core::extract::` |
| 20872 | 56.5 | `<fastopm_server::Server<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::handle::{closure#0}` |
| 16384 | 44.4 | `<fastopm_server::Server<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::manifest::{closure#0}` |
| 14685 | 39.8 | `<fastopm_store::Inner<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::load_local::{closure#0}` |
| 13203 | 35.8 | `tokio::runtime::blocking::pool::spawn_blocking::<tokio::fs::metadata::metadata<std::path::PathBuf>::{closure#0}::{closure#0}, core::result::` |
| 3378 | 9.1 | `<tracing_core::event::Event>::dispatch` |
| 3371 | 9.1 | `<tracing_subscriber::fmt::Subscriber<tracing_subscriber::fmt::format::DefaultFields, tracing_subscriber::fmt::format::Format<tracing_subscri` |
| 3129 | 8.5 | `<std::io::stdio::Stdout as core::io::write::Write>::write_all` |
| 2663 | 7.2 | `<std::io::stdio::Stdout>::lock` |
| 1005 | 2.7 | `<hyper::proto::h1::dispatch::Dispatcher<hyper::proto::h1::dispatch::Server<hyper_util::service::glue::TowerToHyperService<tower::util::map_r` |
| 979 | 2.7 | `writev` |
| 930 | 2.5 | `__recvfrom` |
| 808 | 2.2 | `<tokio::runtime::task::harness::Harness<tokio::runtime::blocking::task::BlockingTask<tokio::fs::metadata::metadata<std::path::PathBuf>::{clo` |
| 654 | 1.8 | `<tokio::runtime::task::core::Core<tokio::runtime::blocking::task::BlockingTask<tokio::fs::metadata::metadata<std::path::PathBuf>::{closure#0` |
| 651 | 1.8 | `fastopm_storage::package_dir` |
| 647 | 1.8 | `<tokio::runtime::blocking::task::BlockingTask<tokio::fs::metadata::metadata<std::path::PathBuf>::{closure#0}::{closure#0}> as core::future::` |
| 615 | 1.7 | `stat` |
| 524 | 1.4 | `<fastopm_store::Inner<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::served_without_network` |
| 476 | 1.3 | `fastopm_store::merge::would_change_self` |
| 446 | 1.2 | `_platform_memmove` |
| 398 | 1.1 | `fastopm_render::public_base_url` |
| 376 | 1.0 | `fastopm_storage::path::normalize` |
| 360 | 1.0 | `<std::io::stdio::StdoutLock as core::io::write::Write>::write_all` |
| 354 | 1.0 | `<std::sys::stdio::unix::Stdout as core::io::write::Write>::write_all` |
| 324 | 0.9 | `<fastopm_server::cache::RenderCache>::get` |
| 315 | 0.9 | `fastopm_storage::path::resolve_safe_path` |
| 311 | 0.8 | `fastopm_store::merge::get_latest_readme` |
| 270 | 0.7 | `fastopm_server::route::make_relative` |
| 259 | 0.7 | `<fastopm_server::Ctx>::header` |
| 250 | 0.7 | `fastopm_store::merge::get_latest_readme::{closure#1}::{closure#0}` |
| 230 | 0.6 | `fastopm_render::public_url::resolve` |
| 218 | 0.6 | `<fastopm_config::Config>::package_rule` |
| 214 | 0.6 | `fastopm_config::pattern::pattern_matches` |

Mutex waits (`__psynch_mutexwait`) by nearest caller outside std/pthread, top 10:

| samples | caller |
|---:|---|
| 11788 | `<tokio::runtime::blocking::pool::Inner>::run` |
| 11762 | `<tokio::runtime::blocking::pool::Spawner>::spawn_task` |
| 2648 | `<std::io::stdio::Stdout as core::io::write::Write>::write_all` |
| 374 | `<fastopm_store::Inner<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::load_local::{closure#0}` |
| 271 | `<fastopm_server::cache::RenderCache>::get` |
| 120 | `<tokio::runtime::scheduler::inject::Inject<alloc::sync::Arc<tokio::runtime::scheduler::multi_thread::handle::Handle>>>::push` |
| 102 | `<deduplicated_symbol>` |
| 53 | `<tokio::runtime::scheduler::inject::Inject<alloc::sync::Arc<tokio::runtime::scheduler::multi_thread::handle::Handle>>>::pop_n::<core::option` |
| 8 | `<tokio::sync::notify::NotifiedProject>::poll_notified` |
| 4 | `<tokio::runtime::context::scoped::Scoped<tokio::runtime::scheduler::Context>>::set::<tokio::runtime::scheduler::multi_thread::worker::run::{` |
