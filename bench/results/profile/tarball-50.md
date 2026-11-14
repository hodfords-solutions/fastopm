## tarball-50

Load: `GET /bench-tarball/-/bench-tarball-2.0.0.tgz` c=8, {}: 94 req/s, statuses {"200":1033}.

63 threads, 200340 thread-samples over 8s (1 ms interval): 18082 on CPU (≈2.3 cores), 12120 blocked on a mutex (≈1.5 threads), rest idle.

Self time (top of stack, on-CPU only), top 25:

| samples | % of on-CPU | function |
|---:|---:|---|
| 9525 | 52.7 | `writev [libsystem_kernel.dylib]` |
| 4825 | 26.7 | `read [libsystem_kernel.dylib]` |
| 1431 | 7.9 | `_platform_memmove [libsystem_platform.dylib]` |
| 644 | 3.6 | `__psynch_cvsignal [libsystem_kernel.dylib]` |
| 462 | 2.6 | `__psynch_mutexdrop [libsystem_kernel.dylib]` |
| 259 | 1.4 | `__ulock_wait2 [libsystem_kernel.dylib]` |
| 159 | 0.9 | `mach_vm_reclaim_update_kernel_accounting_trap [libsystem_kernel.dylib]` |
| 129 | 0.7 | `mach_absolute_time [libsystem_kernel.dylib]` |
| 45 | 0.2 | `__open [libsystem_kernel.dylib]` |
| 39 | 0.2 | `_pthread_mutex_droplock [libsystem_pthread.dylib]` |
| 25 | 0.1 | `<tokio::runtime::context::scoped::Scoped<tokio::runtime::scheduler::Context>>::set::<tokio::runtime::scheduler::multi_thread::worker::run::{` |
| 22 | 0.1 | `<tokio::runtime::task::harness::Harness<tokio::runtime::blocking::task::BlockingTask<<tokio::fs::file::Inner>::spawn_blocking_read::{closure` |
| 19 | 0.1 | `<alloc::collections::vec_deque::iter::Iter<hyper::proto::h1::encode::EncodedBuf<bytes::bytes::Bytes>> as core::iter::traits::iterator::Itera` |
| 19 | 0.1 | `<hyper::proto::h1::io::Buffered<hyper_util::common::rewind::Rewind<hyper_util::rt::tokio::TokioIo<tokio::net::tcp::stream::TcpStream>>, hype` |
| 19 | 0.1 | `_xzm_segment_group_segment_span_free_coalesce [libsystem_malloc.dylib]` |
| 19 | 0.1 | `pthread_mutex_lock [libsystem_pthread.dylib]` |
| 17 | 0.1 | `<tokio::runtime::io::driver::Driver>::turn [fastopm]` |
| 17 | 0.1 | `<std::sys::sync::condvar::pthread::Condvar>::wait_timeout [fastopm]` |
| 17 | 0.1 | `__ulock_wake [libsystem_kernel.dylib]` |
| 16 | 0.1 | `_pthread_cond_wait [libsystem_pthread.dylib]` |
| 16 | 0.1 | `_xzm_free [libsystem_malloc.dylib]` |
| 15 | 0.1 | `<tokio::runtime::blocking::pool::Inner>::run [fastopm]` |
| 14 | 0.1 | `<tokio::runtime::scheduler::multi_thread::park::Inner>::park_driver [fastopm]` |
| 13 | 0.1 | `<deduplicated_symbol> [fastopm]` |
| 12 | 0.1 | `<tokio::runtime::scheduler::multi_thread::park::Inner>::park_condvar [fastopm]` |

Inclusive samples (on CPU + blocked on a mutex) of selected frames, top 40:

| samples | % of on-CPU+mutex | function |
|---:|---:|---|
| 18616 | 61.6 | `<tokio::runtime::task::harness::Harness<axum::serve::handle_connection<tokio::net::tcp::listener::TcpListener, axum::extract::connect_info::` |
| 18562 | 61.5 | `axum::serve::handle_connection::<tokio::net::tcp::listener::TcpListener, axum::extract::connect_info::IntoMakeServiceWithConnectInfo<axum::r` |
| 18554 | 61.4 | `<core::future::poll_fn::PollFn<axum::serve::handle_connection<tokio::net::tcp::listener::TcpListener, axum::extract::connect_info::IntoMakeS` |
| 18528 | 61.3 | `<hyper_util::server::conn::auto::UpgradeableConnection<hyper_util::rt::tokio::TokioIo<tokio::net::tcp::stream::TcpStream>, hyper_util::servi` |
| 18522 | 61.3 | `<hyper::server::conn::http1::UpgradeableConnection<hyper_util::common::rewind::Rewind<hyper_util::rt::tokio::TokioIo<tokio::net::tcp::stream` |
| 9965 | 33.0 | `<hyper::proto::h1::dispatch::Dispatcher<hyper::proto::h1::dispatch::Server<hyper_util::service::glue::TowerToHyperService<tower::util::map_r` |
| 9525 | 31.5 | `writev` |
| 8817 | 29.2 | `<hyper::proto::h1::dispatch::Dispatcher<hyper::proto::h1::dispatch::Server<hyper_util::service::glue::TowerToHyperService<tower::util::map_r` |
| 8377 | 27.7 | `<tokio_util::io::reader_stream::ReaderStream<tokio::fs::file::File> as futures_core::stream::Stream>::poll_next` |
| 8370 | 27.7 | `<http_body_util::combinators::map_err::MapErr<axum_core::body::StreamBody<futures_util::stream::stream::Inspect<futures_util::stream::stream` |
| 8367 | 27.7 | `<axum_core::body::StreamBody<futures_util::stream::stream::Inspect<futures_util::stream::stream::chain::Chain<futures_util::stream::iter::It` |
| 8128 | 26.9 | `tokio_util::util::poll_buf::poll_read_buf::<tokio::fs::file::File, &mut bytes::bytes_mut::BytesMut>` |
| 8120 | 26.9 | `<tokio::fs::file::File as tokio::io::async_read::AsyncRead>::poll_read` |
| 6616 | 21.9 | `tokio::runtime::blocking::pool::spawn_blocking::<<tokio::fs::file::Inner>::spawn_blocking_read::{closure#0}, (tokio::fs::file::Operation, to` |
| 4909 | 16.3 | `<tokio::runtime::task::harness::Harness<tokio::runtime::blocking::task::BlockingTask<<tokio::fs::file::Inner>::spawn_blocking_read::{closure` |
| 4849 | 16.1 | `<tokio::runtime::task::core::Core<tokio::runtime::blocking::task::BlockingTask<<tokio::fs::file::Inner>::spawn_blocking_read::{closure#0}>, ` |
| 1431 | 4.7 | `_platform_memmove` |
| 88 | 0.3 | `<hyper_util::service::oneshot::Oneshot<tower::util::map_request::MapRequest<axum::extension::AddExtension<axum::routing::Router, axum::extra` |
| 77 | 0.3 | `<fastopm_server::router<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>::{closure#0} as axum::handler::Handler<(axum_core::extract::` |
| 76 | 0.3 | `<fastopm_server::Server<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::handle::{closure#0}` |
| 55 | 0.2 | `<tokio::runtime::task::harness::Harness<tokio::runtime::blocking::task::BlockingTask<<tokio::fs::file::Inner>::spawn_blocking_read::{closure` |
| 50 | 0.2 | `<tokio::runtime::task::harness::Harness<tokio::runtime::blocking::task::BlockingTask<fastopm_storage::open_tarball_in::{closure#0}::{closure` |
| 47 | 0.2 | `<tokio::runtime::task::core::Core<tokio::runtime::blocking::task::BlockingTask<fastopm_storage::open_tarball_in::{closure#0}::{closure#0}>, ` |
| 47 | 0.2 | `<tokio::runtime::blocking::task::BlockingTask<fastopm_storage::open_tarball_in::{closure#0}::{closure#0}> as core::future::future::Future>::` |
| 35 | 0.1 | `<tokio::runtime::task::harness::Harness<tokio::runtime::blocking::task::BlockingTask<<tokio::fs::file::Inner>::spawn_blocking_read::{closure` |
| 35 | 0.1 | `<tokio::runtime::task::core::Cell<tokio::runtime::blocking::task::BlockingTask<<tokio::fs::file::Inner>::spawn_blocking_read::{closure#0}>, ` |
| 29 | 0.1 | `<tokio::runtime::task::join::JoinHandle<(tokio::fs::file::Operation, tokio::io::blocking::Buf)> as core::future::future::Future>::poll` |
| 21 | 0.1 | `<tokio::runtime::task::harness::Harness<tokio::runtime::blocking::task::BlockingTask<<tokio::fs::file::Inner>::spawn_blocking_read::{closure` |
| 16 | 0.1 | `tokio::runtime::blocking::pool::spawn_blocking::<fastopm_storage::open_tarball_in::{closure#0}::{closure#0}, core::result::Result<core::opti` |
| 7 | 0.0 | `fastopm_server::route::make_relative` |
| 7 | 0.0 | `__recvfrom` |
| 7 | 0.0 | `<tracing_core::event::Event>::dispatch` |
| 6 | 0.0 | `<tracing_subscriber::fmt::Subscriber<tracing_subscriber::fmt::format::DefaultFields, tracing_subscriber::fmt::format::Format<tracing_subscri` |
| 5 | 0.0 | `<fastopm_storage::FsStorage>::package_dir` |
| 5 | 0.0 | `<fastopm_config::Config>::package_rule` |
| 5 | 0.0 | `fastopm_config::pattern::pattern_matches` |
| 5 | 0.0 | `core::ptr::drop_glue::<http_body_util::combinators::map_err::MapErr<axum_core::body::StreamBody<futures_util::stream::stream::Inspect<future` |
| 5 | 0.0 | `core::ptr::drop_glue::<tokio_util::io::reader_stream::ReaderStream<tokio::fs::file::File>>` |
| 4 | 0.0 | `<std::io::stdio::Stdout as core::io::write::Write>::write_all` |
| 3 | 0.0 | `<tokio::runtime::task::harness::Harness<tokio::runtime::blocking::task::BlockingTask<fastopm_storage::open_tarball_in::{closure#0}::{closure` |

Mutex waits (`__psynch_mutexwait`) by nearest caller outside std/pthread, top 10:

| samples | caller |
|---:|---|
| 6394 | `<tokio::runtime::blocking::pool::Inner>::run` |
| 5639 | `<tokio::runtime::blocking::pool::Spawner>::spawn_task` |
| 20 | `<tokio::runtime::scheduler::inject::Inject<alloc::sync::Arc<tokio::runtime::scheduler::multi_thread::handle::Handle>>>::push` |
| 13 | `<tokio::runtime::scheduler::multi_thread::idle::Idle>::transition_worker_to_parked` |
| 13 | `<deduplicated_symbol>` |
| 11 | `<tokio::runtime::context::scoped::Scoped<tokio::runtime::scheduler::Context>>::set::<tokio::runtime::scheduler::multi_thread::worker::run::{` |
| 11 | `<tokio::runtime::scheduler::inject::Inject<alloc::sync::Arc<tokio::runtime::scheduler::multi_thread::handle::Handle>>>::pop_n::<core::option` |
| 6 | `<tokio::runtime::scheduler::multi_thread::idle::Idle>::worker_to_notify` |
| 5 | `<tokio::runtime::scheduler::multi_thread::idle::Idle>::is_parked` |
| 4 | `<tokio::sync::notify::NotifiedProject>::poll_notified` |
