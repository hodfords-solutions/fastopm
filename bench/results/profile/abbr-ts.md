## abbr-ts

Load: `GET /typescript` c=16, {"accept":"application/vnd.npm.install-v1+json; q=1.0, application/json; q=0.8, */*","accept-encoding":"gzip,deflate"}: 4982 req/s, statuses {"200":54812}.

35 threads, 115084 thread-samples over 8s (1 ms interval): 21704 on CPU (≈2.7 cores), 3713 blocked on a mutex (≈0.5 threads), rest idle.

Self time (top of stack, on-CPU only), top 25:

| samples | % of on-CPU | function |
|---:|---:|---|
| 20104 | 92.6 | `writev [libsystem_kernel.dylib]` |
| 229 | 1.1 | `__psynch_cvsignal [libsystem_kernel.dylib]` |
| 165 | 0.8 | `__recvfrom [libsystem_kernel.dylib]` |
| 160 | 0.7 | `__psynch_mutexdrop [libsystem_kernel.dylib]` |
| 129 | 0.6 | `stat [libsystem_kernel.dylib]` |
| 116 | 0.5 | `fastopm_store::merge::would_change_self [fastopm]` |
| 102 | 0.5 | `write [libsystem_kernel.dylib]` |
| 75 | 0.3 | `_xzm_xzone_malloc_tiny [libsystem_malloc.dylib]` |
| 56 | 0.3 | `_xzm_free [libsystem_malloc.dylib]` |
| 52 | 0.2 | `_platform_memmove [libsystem_platform.dylib]` |
| 42 | 0.2 | `mach_absolute_time [libsystem_kernel.dylib]` |
| 26 | 0.1 | `<fastopm_server::Server<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::handle::{closure#0} [fastopm]` |
| 19 | 0.1 | `_free [libsystem_malloc.dylib]` |
| 19 | 0.1 | `_platform_memcmp [libsystem_platform.dylib]` |
| 18 | 0.1 | `<deduplicated_symbol> [libsystem_malloc.dylib]` |
| 18 | 0.1 | `core::fmt::write [fastopm]` |
| 17 | 0.1 | `<tokio::runtime::context::scoped::Scoped<tokio::runtime::scheduler::Context>>::set::<tokio::runtime::scheduler::multi_thread::worker::run::{` |
| 14 | 0.1 | `<str>::to_lowercase [fastopm]` |
| 14 | 0.1 | `<hyper::proto::h1::role::Server as hyper::proto::h1::Http1Transaction>::encode [fastopm]` |
| 13 | 0.1 | `<http::header::name::StandardHeader>::from_bytes [fastopm]` |
| 13 | 0.1 | `pthread_mutex_unlock [libsystem_pthread.dylib]` |
| 12 | 0.1 | `<std::hash::random::RandomState as core::hash::BuildHasher>::hash_one::<&str> [fastopm]` |
| 12 | 0.1 | `_platform_memset [libsystem_platform.dylib]` |
| 12 | 0.1 | `_xzm_xzone_malloc [libsystem_malloc.dylib]` |
| 11 | 0.1 | `<indexmap::map::IndexMap<alloc::string::String, serde_json::value::Value>>::get::<str> [fastopm]` |

Inclusive samples (on CPU + blocked on a mutex) of selected frames, top 40:

| samples | % of on-CPU+mutex | function |
|---:|---:|---|
| 24111 | 94.9 | `<tokio::runtime::task::harness::Harness<axum::serve::handle_connection<tokio::net::tcp::listener::TcpListener, axum::extract::connect_info::` |
| 24101 | 94.8 | `axum::serve::handle_connection::<tokio::net::tcp::listener::TcpListener, axum::extract::connect_info::IntoMakeServiceWithConnectInfo<axum::r` |
| 24097 | 94.8 | `<core::future::poll_fn::PollFn<axum::serve::handle_connection<tokio::net::tcp::listener::TcpListener, axum::extract::connect_info::IntoMakeS` |
| 24082 | 94.7 | `<hyper_util::server::conn::auto::UpgradeableConnection<hyper_util::rt::tokio::TokioIo<tokio::net::tcp::stream::TcpStream>, hyper_util::servi` |
| 24080 | 94.7 | `<hyper::server::conn::http1::UpgradeableConnection<hyper_util::common::rewind::Rewind<hyper_util::rt::tokio::TokioIo<tokio::net::tcp::stream` |
| 23740 | 93.4 | `<hyper::proto::h1::dispatch::Dispatcher<hyper::proto::h1::dispatch::Server<hyper_util::service::glue::TowerToHyperService<tower::util::map_r` |
| 20138 | 79.2 | `<hyper::proto::h1::dispatch::Dispatcher<hyper::proto::h1::dispatch::Server<hyper_util::service::glue::TowerToHyperService<tower::util::map_r` |
| 20104 | 79.1 | `writev` |
| 3648 | 14.4 | `<hyper_util::service::oneshot::Oneshot<tower::util::map_request::MapRequest<axum::extension::AddExtension<axum::routing::Router, axum::extra` |
| 3578 | 14.1 | `<fastopm_server::router<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>::{closure#0} as axum::handler::Handler<(axum_core::extract::` |
| 3572 | 14.1 | `<fastopm_server::Server<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::handle::{closure#0}` |
| 1553 | 6.1 | `<fastopm_server::Server<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::manifest::{closure#0}` |
| 1122 | 4.4 | `<fastopm_store::Inner<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::load_local::{closure#0}` |
| 1049 | 4.1 | `tokio::runtime::blocking::pool::spawn_blocking::<<fastopm_server::Server<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::finish_js` |
| 959 | 3.8 | `tokio::runtime::blocking::pool::spawn_blocking::<tokio::fs::metadata::metadata<std::path::PathBuf>::{closure#0}::{closure#0}, core::result::` |
| 672 | 2.6 | `<tracing_core::event::Event>::dispatch` |
| 667 | 2.6 | `<tracing_subscriber::fmt::Subscriber<tracing_subscriber::fmt::format::DefaultFields, tracing_subscriber::fmt::format::Format<tracing_subscri` |
| 551 | 2.2 | `<std::io::stdio::Stdout as core::io::write::Write>::write_all` |
| 435 | 1.7 | `<std::io::stdio::Stdout>::lock` |
| 211 | 0.8 | `<fastopm_store::Inner<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::served_without_network` |
| 194 | 0.8 | `fastopm_store::merge::would_change_self` |
| 165 | 0.6 | `__recvfrom` |
| 141 | 0.6 | `<tokio::runtime::task::harness::Harness<tokio::runtime::blocking::task::BlockingTask<tokio::fs::metadata::metadata<std::path::PathBuf>::{clo` |
| 132 | 0.5 | `<tokio::runtime::task::core::Core<tokio::runtime::blocking::task::BlockingTask<tokio::fs::metadata::metadata<std::path::PathBuf>::{closure#0` |
| 130 | 0.5 | `<tokio::runtime::blocking::task::BlockingTask<tokio::fs::metadata::metadata<std::path::PathBuf>::{closure#0}::{closure#0}> as core::future::` |
| 129 | 0.5 | `stat` |
| 106 | 0.4 | `<std::io::stdio::StdoutLock as core::io::write::Write>::write_all` |
| 103 | 0.4 | `<std::sys::stdio::unix::Stdout as core::io::write::Write>::write_all` |
| 89 | 0.4 | `<tracing_core::field::ValueSet>::record` |
| 81 | 0.3 | `<tracing_subscriber::fmt::format::DefaultVisitor as tracing_core::field::Visit>::record_debug` |
| 79 | 0.3 | `fastopm_render::public_base_url` |
| 70 | 0.3 | `<fastopm_server::cache::RenderCache>::get` |
| 65 | 0.3 | `fastopm_server::negotiate::negotiate_coding` |
| 62 | 0.2 | `fastopm_store::merge::get_latest_readme` |
| 58 | 0.2 | `fastopm_store::merge::get_latest_readme::{closure#0}` |
| 52 | 0.2 | `_platform_memmove` |
| 50 | 0.2 | `fastopm_render::public_url::resolve` |
| 47 | 0.2 | `fastopm_storage::package_dir` |
| 42 | 0.2 | `<fastopm_config::Config>::package_rule` |
| 42 | 0.2 | `fastopm_config::pattern::pattern_matches` |

Mutex waits (`__psynch_mutexwait`) by nearest caller outside std/pthread, top 10:

| samples | caller |
|---:|---|
| 1685 | `<tokio::runtime::blocking::pool::Spawner>::spawn_task` |
| 1397 | `<tokio::runtime::blocking::pool::Inner>::run` |
| 434 | `<std::io::stdio::Stdout as core::io::write::Write>::write_all` |
| 59 | `<fastopm_server::cache::RenderCache>::get` |
| 54 | `<fastopm_store::Inner<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::load_local::{closure#0}` |
| 22 | `<deduplicated_symbol>` |
| 22 | `<tokio::runtime::scheduler::inject::Inject<alloc::sync::Arc<tokio::runtime::scheduler::multi_thread::handle::Handle>>>::push` |
| 20 | `<tokio::runtime::scheduler::inject::Inject<alloc::sync::Arc<tokio::runtime::scheduler::multi_thread::handle::Handle>>>::pop_n::<core::option` |
| 9 | `<tokio::runtime::context::scoped::Scoped<tokio::runtime::scheduler::Context>>::set::<tokio::runtime::scheduler::multi_thread::worker::run::{` |
| 6 | `<tokio::runtime::scheduler::multi_thread::idle::Idle>::transition_worker_to_parked` |
