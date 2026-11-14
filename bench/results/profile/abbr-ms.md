## abbr-ms

Load: `GET /ms` c=128, {"accept":"application/vnd.npm.install-v1+json; q=1.0, application/json; q=0.8, */*","accept-encoding":"gzip,deflate"}: 34759 req/s, statuses {"200":382424}.

123 threads, 281762 thread-samples over 8s (1 ms interval): 10195 on CPU (≈1.3 cores), 28931 blocked on a mutex (≈3.6 threads), rest idle.

Self time (top of stack, on-CPU only), top 25:

| samples | % of on-CPU | function |
|---:|---:|---|
| 1134 | 11.1 | `__psynch_mutexdrop [libsystem_kernel.dylib]` |
| 1118 | 11.0 | `writev [libsystem_kernel.dylib]` |
| 922 | 9.0 | `__psynch_cvsignal [libsystem_kernel.dylib]` |
| 859 | 8.4 | `__recvfrom [libsystem_kernel.dylib]` |
| 630 | 6.2 | `_xzm_xzone_malloc_tiny [libsystem_malloc.dylib]` |
| 511 | 5.0 | `stat [libsystem_kernel.dylib]` |
| 444 | 4.4 | `_xzm_free [libsystem_malloc.dylib]` |
| 367 | 3.6 | `write [libsystem_kernel.dylib]` |
| 278 | 2.7 | `mach_absolute_time [libsystem_kernel.dylib]` |
| 197 | 1.9 | `_platform_memmove [libsystem_platform.dylib]` |
| 147 | 1.4 | `<std::path::Components as core::iter::traits::iterator::Iterator>::next [fastopm]` |
| 106 | 1.0 | `<fastopm_server::Server<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::handle::{closure#0} [fastopm]` |
| 104 | 1.0 | `_free [libsystem_malloc.dylib]` |
| 88 | 0.9 | `<deduplicated_symbol> [libsystem_malloc.dylib]` |
| 87 | 0.9 | `_malloc_zone_malloc [libsystem_malloc.dylib]` |
| 83 | 0.8 | `_xzm_xzone_malloc [libsystem_malloc.dylib]` |
| 70 | 0.7 | `_platform_memcmp [libsystem_platform.dylib]` |
| 66 | 0.6 | `<alloc::string::String as core::iter::traits::collect::FromIterator<char>>::from_iter::<core::iter::adapters::map::Map<core::slice::iter::It` |
| 66 | 0.6 | `core::fmt::write [fastopm]` |
| 61 | 0.6 | `_pthread_mutex_droplock [libsystem_pthread.dylib]` |
| 53 | 0.5 | `<str>::to_lowercase [fastopm]` |
| 51 | 0.5 | `<idna::uts46::Uts46>::process_innermost [fastopm]` |
| 49 | 0.5 | `<fastopm_server::router<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>::{closure#0} as axum::handler::Handler<(axum_core::extract::` |
| 49 | 0.5 | `xzm_malloc_zone_size [libsystem_malloc.dylib]` |
| 46 | 0.5 | `pthread_mutex_lock [libsystem_pthread.dylib]` |

Inclusive samples (on CPU + blocked on a mutex) of selected frames, top 40:

| samples | % of on-CPU+mutex | function |
|---:|---:|---|
| 24134 | 61.7 | `<tokio::runtime::task::harness::Harness<axum::serve::handle_connection<tokio::net::tcp::listener::TcpListener, axum::extract::connect_info::` |
| 24110 | 61.6 | `axum::serve::handle_connection::<tokio::net::tcp::listener::TcpListener, axum::extract::connect_info::IntoMakeServiceWithConnectInfo<axum::r` |
| 24101 | 61.6 | `<core::future::poll_fn::PollFn<axum::serve::handle_connection<tokio::net::tcp::listener::TcpListener, axum::extract::connect_info::IntoMakeS` |
| 24037 | 61.4 | `<hyper_util::server::conn::auto::UpgradeableConnection<hyper_util::rt::tokio::TokioIo<tokio::net::tcp::stream::TcpStream>, hyper_util::servi` |
| 24018 | 61.4 | `<hyper::server::conn::http1::UpgradeableConnection<hyper_util::common::rewind::Rewind<hyper_util::rt::tokio::TokioIo<tokio::net::tcp::stream` |
| 21640 | 55.3 | `<hyper::proto::h1::dispatch::Dispatcher<hyper::proto::h1::dispatch::Server<hyper_util::service::glue::TowerToHyperService<tower::util::map_r` |
| 21481 | 54.9 | `<hyper_util::service::oneshot::Oneshot<tower::util::map_request::MapRequest<axum::extension::AddExtension<axum::routing::Router, axum::extra` |
| 21086 | 53.9 | `<fastopm_server::router<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>::{closure#0} as axum::handler::Handler<(axum_core::extract::` |
| 21027 | 53.7 | `<fastopm_server::Server<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::handle::{closure#0}` |
| 16681 | 42.6 | `<fastopm_server::Server<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::manifest::{closure#0}` |
| 15159 | 38.7 | `<fastopm_store::Inner<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::load_local::{closure#0}` |
| 13888 | 35.5 | `tokio::runtime::blocking::pool::spawn_blocking::<tokio::fs::metadata::metadata<std::path::PathBuf>::{closure#0}::{closure#0}, core::result::` |
| 2723 | 7.0 | `<tracing_core::event::Event>::dispatch` |
| 2712 | 6.9 | `<tracing_subscriber::fmt::Subscriber<tracing_subscriber::fmt::format::DefaultFields, tracing_subscriber::fmt::format::Format<tracing_subscri` |
| 2471 | 6.3 | `<std::io::stdio::Stdout as core::io::write::Write>::write_all` |
| 1989 | 5.1 | `<std::io::stdio::Stdout>::lock` |
| 1157 | 3.0 | `<hyper::proto::h1::dispatch::Dispatcher<hyper::proto::h1::dispatch::Server<hyper_util::service::glue::TowerToHyperService<tower::util::map_r` |
| 1118 | 2.9 | `writev` |
| 859 | 2.2 | `__recvfrom` |
| 710 | 1.8 | `<tokio::runtime::task::harness::Harness<tokio::runtime::blocking::task::BlockingTask<tokio::fs::metadata::metadata<std::path::PathBuf>::{clo` |
| 616 | 1.6 | `fastopm_storage::package_dir` |
| 541 | 1.4 | `<tokio::runtime::task::core::Core<tokio::runtime::blocking::task::BlockingTask<tokio::fs::metadata::metadata<std::path::PathBuf>::{closure#0` |
| 535 | 1.4 | `<tokio::runtime::blocking::task::BlockingTask<tokio::fs::metadata::metadata<std::path::PathBuf>::{closure#0}::{closure#0}> as core::future::` |
| 511 | 1.3 | `stat` |
| 418 | 1.1 | `fastopm_server::negotiate::negotiate_coding` |
| 401 | 1.0 | `fastopm_render::public_base_url` |
| 370 | 0.9 | `<std::io::stdio::StdoutLock as core::io::write::Write>::write_all` |
| 366 | 0.9 | `<std::sys::stdio::unix::Stdout as core::io::write::Write>::write_all` |
| 319 | 0.8 | `<fastopm_config::Config>::package_rule` |
| 317 | 0.8 | `<fastopm_server::cache::RenderCache>::get` |
| 316 | 0.8 | `fastopm_storage::path::resolve_safe_path` |
| 314 | 0.8 | `fastopm_config::pattern::pattern_matches` |
| 308 | 0.8 | `fastopm_server::route::make_relative` |
| 305 | 0.8 | `<fastopm_server::Ctx>::header` |
| 277 | 0.7 | `fastopm_storage::path::normalize` |
| 259 | 0.7 | `<fastopm_store::Inner<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::served_without_network` |
| 236 | 0.6 | `fastopm_render::public_url::resolve` |
| 205 | 0.5 | `fastopm_store::merge::would_change_self` |
| 197 | 0.5 | `_platform_memmove` |
| 186 | 0.5 | `<tracing_core::field::ValueSet>::record` |

Mutex waits (`__psynch_mutexwait`) by nearest caller outside std/pthread, top 10:

| samples | caller |
|---:|---|
| 13969 | `<tokio::runtime::blocking::pool::Inner>::run` |
| 12189 | `<tokio::runtime::blocking::pool::Spawner>::spawn_task` |
| 1975 | `<std::io::stdio::Stdout as core::io::write::Write>::write_all` |
| 274 | `<fastopm_store::Inner<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::load_local::{closure#0}` |
| 250 | `<fastopm_server::cache::RenderCache>::get` |
| 122 | `<tokio::runtime::scheduler::inject::Inject<alloc::sync::Arc<tokio::runtime::scheduler::multi_thread::handle::Handle>>>::push` |
| 101 | `<deduplicated_symbol>` |
| 48 | `<tokio::runtime::scheduler::inject::Inject<alloc::sync::Arc<tokio::runtime::scheduler::multi_thread::handle::Handle>>>::pop_n::<core::option` |
| 4 | `<tokio::sync::notify::NotifiedProject>::poll_notified` |
| 1 | `<tokio::runtime::scheduler::multi_thread::idle::Idle>::worker_to_notify` |
