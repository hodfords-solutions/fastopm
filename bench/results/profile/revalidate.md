## revalidate

Load: `GET /express` c=32, {"accept":"application/vnd.npm.install-v1+json; q=1.0, application/json; q=0.8, */*","accept-encoding":"gzip,deflate"}, maxage 1ms: 1332 req/s, statuses {"200":14653}.

67 threads, 329841 thread-samples over 8s (1 ms interval): 26398 on CPU (≈3.3 cores), 1026 blocked on a mutex (≈0.1 threads), rest idle.

Self time (top of stack, on-CPU only), top 25:

| samples | % of on-CPU | function |
|---:|---:|---|
| 16463 | 62.4 | `miniz_oxide::deflate::core::compress_inner [fastopm]` |
| 2330 | 8.8 | `md5::compress::compress [fastopm]` |
| 1501 | 5.7 | `fastopm_render::json::write_string [fastopm]` |
| 508 | 1.9 | `_platform_memmove [libsystem_platform.dylib]` |
| 445 | 1.7 | `_xzm_xzone_malloc_tiny [libsystem_malloc.dylib]` |
| 417 | 1.6 | `_xzm_free [libsystem_malloc.dylib]` |
| 344 | 1.3 | `miniz_oxide::deflate::core::flush_block [fastopm]` |
| 196 | 0.7 | `<fastopm_render::json::ObjectWriter>::key [fastopm]` |
| 193 | 0.7 | `<indexmap::map::IndexMap<alloc::string::String, serde_json::value::Value>>::get::<str> [fastopm]` |
| 186 | 0.7 | `core::str::converts::from_utf8 [fastopm]` |
| 178 | 0.7 | `_platform_memset [libsystem_platform.dylib]` |
| 159 | 0.6 | `writev [libsystem_kernel.dylib]` |
| 131 | 0.5 | `<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>> as serde_core::ser::Serializer>::serialize_str [fastopm]` |
| 129 | 0.5 | `fastopm_render::json::write_value [fastopm]` |
| 122 | 0.5 | `fastopm_render::json::js_entries [fastopm]` |
| 116 | 0.4 | `_free [libsystem_malloc.dylib]` |
| 111 | 0.4 | `_malloc_zone_malloc [libsystem_malloc.dylib]` |
| 102 | 0.4 | `mach_absolute_time [libsystem_kernel.dylib]` |
| 100 | 0.4 | `__psynch_cvsignal [libsystem_kernel.dylib]` |
| 95 | 0.4 | `<deduplicated_symbol> [libsystem_malloc.dylib]` |
| 94 | 0.4 | `<url::parser::Parser>::after_double_slash [fastopm]` |
| 89 | 0.3 | `_xzm_xzone_malloc [libsystem_malloc.dylib]` |
| 79 | 0.3 | `<core::marker::PhantomData<serde_json::value::Value> as serde_core::de::DeserializeSeed>::deserialize::<&mut serde_json::de::Deserializer<se` |
| 78 | 0.3 | `<core::hash::sip::Hasher<core::hash::sip::Sip13Rounds> as core::hash::Hasher>::write [fastopm]` |
| 73 | 0.3 | `<std::hash::random::RandomState as core::hash::BuildHasher>::hash_one::<&str> [fastopm]` |

Inclusive samples (on CPU + blocked on a mutex) of selected frames, top 40:

| samples | % of on-CPU+mutex | function |
|---:|---:|---|
| 54995 | 200.5 | `<tokio::runtime::task::harness::Harness<tokio::runtime::blocking::task::BlockingTask<<fastopm_server::Server<fastopm_storage::FsStorage, fas` |
| 54945 | 200.4 | `<tokio::runtime::task::core::Core<tokio::runtime::blocking::task::BlockingTask<<fastopm_server::Server<fastopm_storage::FsStorage, fastopm_u` |
| 54942 | 200.3 | `<tokio::runtime::blocking::task::BlockingTask<<fastopm_server::Server<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::finish_json:` |
| 54939 | 200.3 | `<fastopm_server::cache::CachedBody>::encoded` |
| 54934 | 200.3 | `<std::sync::once_lock::OnceLock<bytes::bytes::Bytes>>::initialize::<<std::sync::once_lock::OnceLock<bytes::bytes::Bytes>>::get_or_init<<fast` |
| 16989 | 61.9 | `<std::sync::once::Once>::call_once_force::<<std::sync::once_lock::OnceLock<bytes::bytes::Bytes>>::initialize<<std::sync::once_lock::OnceLock` |
| 16987 | 61.9 | `fastopm_server::compress::encode` |
| 16883 | 61.6 | `<flate2::mem::Compress as flate2::zio::Ops>::run_vec` |
| 16844 | 61.4 | `miniz_oxide::deflate::stream::deflate` |
| 16843 | 61.4 | `miniz_oxide::deflate::core::compress_inner` |
| 16707 | 60.9 | `<flate2::zio::Writer<alloc::vec::Vec<u8>, flate2::mem::Compress>>::write_with_status` |
| 7788 | 28.4 | `<tokio::runtime::task::harness::Harness<axum::serve::handle_connection<tokio::net::tcp::listener::TcpListener, axum::extract::connect_info::` |
| 7786 | 28.4 | `axum::serve::handle_connection::<tokio::net::tcp::listener::TcpListener, axum::extract::connect_info::IntoMakeServiceWithConnectInfo<axum::r` |
| 7785 | 28.4 | `<core::future::poll_fn::PollFn<axum::serve::handle_connection<tokio::net::tcp::listener::TcpListener, axum::extract::connect_info::IntoMakeS` |
| 7777 | 28.4 | `<hyper_util::server::conn::auto::UpgradeableConnection<hyper_util::rt::tokio::TokioIo<tokio::net::tcp::stream::TcpStream>, hyper_util::servi` |
| 7774 | 28.3 | `<hyper::server::conn::http1::UpgradeableConnection<hyper_util::common::rewind::Rewind<hyper_util::rt::tokio::TokioIo<tokio::net::tcp::stream` |
| 7535 | 27.5 | `<hyper::proto::h1::dispatch::Dispatcher<hyper::proto::h1::dispatch::Server<hyper_util::service::glue::TowerToHyperService<tower::util::map_r` |
| 7523 | 27.4 | `<hyper_util::service::oneshot::Oneshot<tower::util::map_request::MapRequest<axum::extension::AddExtension<axum::routing::Router, axum::extra` |
| 7502 | 27.4 | `<fastopm_server::router<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>::{closure#0} as axum::handler::Handler<(axum_core::extract::` |
| 7500 | 27.3 | `<fastopm_server::Server<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::handle::{closure#0}` |
| 6769 | 24.7 | `<fastopm_server::Server<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::manifest::{closure#0}` |
| 6167 | 22.5 | `fastopm_render::render_manifest` |
| 3754 | 13.7 | `fastopm_render::json::write_object::<fastopm_render::render_manifest::{closure#2}>` |
| 3700 | 13.5 | `fastopm_render::write_abbreviated_version` |
| 2344 | 8.5 | `<tokio::runtime::task::harness::Harness<tokio::runtime::blocking::task::BlockingTask<fastopm_storage::update_manifest_in<<fastopm_store::Inn` |
| 2344 | 8.5 | `<tokio::runtime::task::core::Core<tokio::runtime::blocking::task::BlockingTask<fastopm_storage::update_manifest_in<<fastopm_store::Inner<fas` |
| 2344 | 8.5 | `<tokio::runtime::blocking::task::BlockingTask<fastopm_storage::update_manifest_in<<fastopm_store::Inner<fastopm_storage::FsStorage, fastopm_` |
| 2335 | 8.5 | `fastopm_render::rendered` |
| 2330 | 8.5 | `md5::compress::compress` |
| 1782 | 6.5 | `fastopm_render::json::write_string` |
| 1666 | 6.1 | `fastopm_render::json::write_value` |
| 1643 | 6.0 | `fastopm_render::write_dist` |
| 1245 | 4.5 | `fastopm_storage::read_manifest_sync` |
| 1198 | 4.4 | `<fastopm_core::manifest::Manifest>::from_slice` |
| 1198 | 4.4 | `serde_json::de::from_trait::<serde_json::read::SliceRead, fastopm_core::manifest::Manifest>` |
| 1197 | 4.4 | `<&mut serde_json::de::Deserializer<serde_json::read::SliceRead> as serde_core::de::Deserializer>::deserialize_map::<<serde_json::map::Map<al` |
| 1194 | 4.4 | `<core::marker::PhantomData<serde_json::value::Value> as serde_core::de::DeserializeSeed>::deserialize::<&mut serde_json::de::Deserializer<se` |
| 1189 | 4.3 | `<<serde_json::value::Value as serde_core::de::Deserialize>::deserialize::ValueVisitor as serde_core::de::Visitor>::visit_map::<serde_json::d` |
| 1056 | 3.9 | `<fastopm_render::json::ObjectWriter>::key` |
| 845 | 3.1 | `fastopm_render::local_tarball_url` |

Mutex waits (`__psynch_mutexwait`) by nearest caller outside std/pthread, top 10:

| samples | caller |
|---:|---|
| 569 | `<tokio::runtime::blocking::pool::Spawner>::spawn_task` |
| 305 | `<tokio::runtime::blocking::pool::Inner>::run` |
| 63 | `<std::io::stdio::Stdout as core::io::write::Write>::write_all` |
| 32 | `<fastopm_server::cache::RenderCache>::get` |
| 27 | `<fastopm_server::cache::RenderCache>::insert` |
| 7 | `<tokio::runtime::scheduler::inject::Inject<alloc::sync::Arc<tokio::runtime::scheduler::multi_thread::handle::Handle>>>::push` |
| 6 | `<fastopm_store::Inner<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::load_local::{closure#0}` |
| 3 | `<fastopm_store::Inner<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::refresh` |
| 3 | `<tokio::sync::notify::NotifiedProject>::poll_notified` |
| 3 | `<deduplicated_symbol>` |
