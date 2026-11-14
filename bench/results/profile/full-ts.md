## full-ts

Load: `GET /typescript` c=16, {"accept":"application/json","accept-encoding":"gzip,deflate"}: 4269 req/s, statuses {"200":46974}.

35 threads, 114166 thread-samples over 8s (1 ms interval): 20446 on CPU (≈2.6 cores), 3104 blocked on a mutex (≈0.4 threads), rest idle.

Self time (top of stack, on-CPU only), top 25:

| samples | % of on-CPU | function |
|---:|---:|---|
| 18597 | 91.0 | `writev [libsystem_kernel.dylib]` |
| 293 | 1.4 | `__psynch_cvsignal [libsystem_kernel.dylib]` |
| 203 | 1.0 | `__recvfrom [libsystem_kernel.dylib]` |
| 156 | 0.8 | `stat [libsystem_kernel.dylib]` |
| 151 | 0.7 | `fastopm_store::merge::would_change_self [fastopm]` |
| 122 | 0.6 | `__psynch_mutexdrop [libsystem_kernel.dylib]` |
| 101 | 0.5 | `write [libsystem_kernel.dylib]` |
| 72 | 0.4 | `_xzm_xzone_malloc_tiny [libsystem_malloc.dylib]` |
| 63 | 0.3 | `_xzm_free [libsystem_malloc.dylib]` |
| 52 | 0.3 | `mach_absolute_time [libsystem_kernel.dylib]` |
| 36 | 0.2 | `_platform_memmove [libsystem_platform.dylib]` |
| 24 | 0.1 | `<fastopm_server::Server<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::handle::{closure#0} [fastopm]` |
| 22 | 0.1 | `pthread_mutex_lock [libsystem_pthread.dylib]` |
| 21 | 0.1 | `_free [libsystem_malloc.dylib]` |
| 20 | 0.1 | `<hyper::proto::h1::role::Server as hyper::proto::h1::Http1Transaction>::encode [fastopm]` |
| 20 | 0.1 | `_xzm_xzone_malloc [libsystem_malloc.dylib]` |
| 19 | 0.1 | `_platform_memcmp [libsystem_platform.dylib]` |
| 18 | 0.1 | `<tokio::runtime::context::scoped::Scoped<tokio::runtime::scheduler::Context>>::set::<tokio::runtime::scheduler::multi_thread::worker::run::{` |
| 16 | 0.1 | `<std::hash::random::RandomState as core::hash::BuildHasher>::hash_one::<&str> [fastopm]` |
| 16 | 0.1 | `<std::path::Components as core::iter::traits::iterator::Iterator>::next [fastopm]` |
| 15 | 0.1 | `<deduplicated_symbol> [libsystem_malloc.dylib]` |
| 15 | 0.1 | `<str>::to_lowercase [fastopm]` |
| 13 | 0.1 | `<tokio::runtime::task::state::State>::transition_to_join_handle_dropped [fastopm]` |
| 12 | 0.1 | `<idna::uts46::Uts46>::to_ascii_from_cow [fastopm]` |
| 12 | 0.1 | `core::fmt::write [fastopm]` |

Inclusive samples (on CPU + blocked on a mutex) of selected frames, top 40:

| samples | % of on-CPU+mutex | function |
|---:|---:|---|
| 22185 | 94.2 | `<tokio::runtime::task::harness::Harness<axum::serve::handle_connection<tokio::net::tcp::listener::TcpListener, axum::extract::connect_info::` |
| 22177 | 94.2 | `axum::serve::handle_connection::<tokio::net::tcp::listener::TcpListener, axum::extract::connect_info::IntoMakeServiceWithConnectInfo<axum::r` |
| 22172 | 94.1 | `<core::future::poll_fn::PollFn<axum::serve::handle_connection<tokio::net::tcp::listener::TcpListener, axum::extract::connect_info::IntoMakeS` |
| 22149 | 94.1 | `<hyper_util::server::conn::auto::UpgradeableConnection<hyper_util::rt::tokio::TokioIo<tokio::net::tcp::stream::TcpStream>, hyper_util::servi` |
| 22140 | 94.0 | `<hyper::server::conn::http1::UpgradeableConnection<hyper_util::common::rewind::Rewind<hyper_util::rt::tokio::TokioIo<tokio::net::tcp::stream` |
| 21789 | 92.5 | `<hyper::proto::h1::dispatch::Dispatcher<hyper::proto::h1::dispatch::Server<hyper_util::service::glue::TowerToHyperService<tower::util::map_r` |
| 18616 | 79.0 | `<hyper::proto::h1::dispatch::Dispatcher<hyper::proto::h1::dispatch::Server<hyper_util::service::glue::TowerToHyperService<tower::util::map_r` |
| 18597 | 79.0 | `writev` |
| 3165 | 13.4 | `<hyper_util::service::oneshot::Oneshot<tower::util::map_request::MapRequest<axum::extension::AddExtension<axum::routing::Router, axum::extra` |
| 3079 | 13.1 | `<fastopm_server::router<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>::{closure#0} as axum::handler::Handler<(axum_core::extract::` |
| 3070 | 13.0 | `<fastopm_server::Server<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::handle::{closure#0}` |
| 1397 | 5.9 | `<fastopm_server::Server<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::manifest::{closure#0}` |
| 985 | 4.2 | `<fastopm_store::Inner<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::load_local::{closure#0}` |
| 824 | 3.5 | `tokio::runtime::blocking::pool::spawn_blocking::<<fastopm_server::Server<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::finish_js` |
| 801 | 3.4 | `tokio::runtime::blocking::pool::spawn_blocking::<tokio::fs::metadata::metadata<std::path::PathBuf>::{closure#0}::{closure#0}, core::result::` |
| 508 | 2.2 | `<tracing_core::event::Event>::dispatch` |
| 502 | 2.1 | `<tracing_subscriber::fmt::Subscriber<tracing_subscriber::fmt::format::DefaultFields, tracing_subscriber::fmt::format::Format<tracing_subscri` |
| 416 | 1.8 | `<std::io::stdio::Stdout as core::io::write::Write>::write_all` |
| 306 | 1.3 | `<std::io::stdio::Stdout>::lock` |
| 236 | 1.0 | `<fastopm_store::Inner<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::served_without_network` |
| 222 | 0.9 | `fastopm_store::merge::would_change_self` |
| 203 | 0.9 | `__recvfrom` |
| 194 | 0.8 | `<tokio::runtime::task::harness::Harness<tokio::runtime::blocking::task::BlockingTask<tokio::fs::metadata::metadata<std::path::PathBuf>::{clo` |
| 163 | 0.7 | `<tokio::runtime::task::core::Core<tokio::runtime::blocking::task::BlockingTask<tokio::fs::metadata::metadata<std::path::PathBuf>::{closure#0` |
| 161 | 0.7 | `<tokio::runtime::blocking::task::BlockingTask<tokio::fs::metadata::metadata<std::path::PathBuf>::{closure#0}::{closure#0}> as core::future::` |
| 156 | 0.7 | `stat` |
| 102 | 0.4 | `<std::io::stdio::StdoutLock as core::io::write::Write>::write_all` |
| 101 | 0.4 | `<std::sys::stdio::unix::Stdout as core::io::write::Write>::write_all` |
| 76 | 0.3 | `fastopm_storage::package_dir` |
| 69 | 0.3 | `fastopm_render::public_base_url` |
| 64 | 0.3 | `<tracing_core::field::ValueSet>::record` |
| 62 | 0.3 | `fastopm_server::negotiate::negotiate_coding` |
| 61 | 0.3 | `<tracing_subscriber::fmt::format::DefaultVisitor as tracing_core::field::Visit>::record_debug` |
| 56 | 0.2 | `fastopm_server::route::make_relative` |
| 55 | 0.2 | `fastopm_store::merge::get_latest_readme` |
| 50 | 0.2 | `fastopm_store::merge::get_latest_readme::{closure#0}` |
| 48 | 0.2 | `<fastopm_server::cache::RenderCache>::get` |
| 43 | 0.2 | `<fastopm_config::Config>::package_rule` |
| 41 | 0.2 | `<indexmap::map::IndexMap<alloc::string::String, serde_json::value::Value>>::get::<str>` |
| 40 | 0.2 | `fastopm_render::public_url::resolve` |

Mutex waits (`__psynch_mutexwait`) by nearest caller outside std/pthread, top 10:

| samples | caller |
|---:|---|
| 1362 | `<tokio::runtime::blocking::pool::Inner>::run` |
| 1285 | `<tokio::runtime::blocking::pool::Spawner>::spawn_task` |
| 306 | `<std::io::stdio::Stdout as core::io::write::Write>::write_all` |
| 37 | `<fastopm_server::cache::RenderCache>::get` |
| 35 | `<fastopm_store::Inner<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::load_local::{closure#0}` |
| 22 | `<tokio::runtime::scheduler::inject::Inject<alloc::sync::Arc<tokio::runtime::scheduler::multi_thread::handle::Handle>>>::push` |
| 20 | `<tokio::runtime::scheduler::inject::Inject<alloc::sync::Arc<tokio::runtime::scheduler::multi_thread::handle::Handle>>>::pop_n::<core::option` |
| 14 | `<deduplicated_symbol>` |
| 9 | `<tokio::runtime::scheduler::multi_thread::idle::Idle>::transition_worker_to_parked` |
| 7 | `<tokio::runtime::context::scoped::Scoped<tokio::runtime::scheduler::Context>>::set::<tokio::runtime::scheduler::multi_thread::worker::run::{` |
