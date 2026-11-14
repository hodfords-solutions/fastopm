## cold-distinct

Load: distinct uncached packages, 16 clients.

35 threads, 154140 thread-samples over 8s (1 ms interval): 52815 on CPU (≈6.6 cores), 8379 blocked on a mutex (≈1.0 threads), rest idle.

Self time (top of stack, on-CPU only), top 25:

| samples | % of on-CPU | function |
|---:|---:|---|
| 8802 | 16.7 | `miniz_oxide::deflate::core::compress_inner [fastopm]` |
| 6020 | 11.4 | `_xzm_xzone_malloc_tiny [libsystem_malloc.dylib]` |
| 5710 | 10.8 | `_xzm_free [libsystem_malloc.dylib]` |
| 2870 | 5.4 | `_platform_memmove [libsystem_platform.dylib]` |
| 1972 | 3.7 | `<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>> as serde_core::ser::Serializer>::serialize_str [fastopm]` |
| 1471 | 2.8 | `_xzm_xzone_malloc [libsystem_malloc.dylib]` |
| 1445 | 2.7 | `core::str::converts::from_utf8 [fastopm]` |
| 1421 | 2.7 | `<deduplicated_symbol> [libsystem_malloc.dylib]` |
| 1330 | 2.5 | `_malloc_zone_malloc [libsystem_malloc.dylib]` |
| 1081 | 2.0 | `_free [libsystem_malloc.dylib]` |
| 963 | 1.8 | `fastopm_render::json::write_string [fastopm]` |
| 963 | 1.8 | `md5::compress::compress [fastopm]` |
| 803 | 1.5 | `core::ptr::drop_glue::<serde_json::value::Value> [fastopm]` |
| 715 | 1.4 | `<core::marker::PhantomData<serde_json::value::Value> as serde_core::de::DeserializeSeed>::deserialize::<&mut serde_json::de::Deserializer<se` |
| 658 | 1.2 | `<indexmap::map::IndexMap<alloc::string::String, serde_json::value::Value>>::insert_full [fastopm]` |
| 657 | 1.2 | `<hashbrown::raw::RawTable<usize>>::reserve_rehash::<indexmap::inner::get_hash<alloc::string::String, serde_json::value::Value>::{closure#0}>` |
| 657 | 1.2 | `<serde_json::read::SliceRead>::skip_to_escape [fastopm]` |
| 648 | 1.2 | `<core::iter::adapters::cloned::Cloned<core::slice::iter::Iter<indexmap::Bucket<alloc::string::String, serde_json::value::Value>>> as core::i` |
| 645 | 1.2 | `<indexmap::inner::Core<alloc::string::String, serde_json::value::Value>>::push_entry [fastopm]` |
| 614 | 1.2 | `<alloc::string::String as core::clone::Clone>::clone [fastopm]` |
| 577 | 1.1 | `_platform_memset [libsystem_platform.dylib]` |
| 565 | 1.1 | `<core::hash::sip::Hasher<core::hash::sip::Sip13Rounds> as core::hash::Hasher>::write [fastopm]` |
| 555 | 1.1 | `<serde_json::read::SliceRead as serde_json::read::Read>::parse_str [fastopm]` |
| 517 | 1.0 | `__ulock_wait [libsystem_kernel.dylib]` |
| 488 | 0.9 | `<serde_json::value::Value as serde_core::ser::Serialize>::serialize::<&mut serde_json::ser::Serializer<&mut alloc::vec::Vec<u8>>> [fastopm]` |

Inclusive samples (on CPU + blocked on a mutex) of selected frames, top 40:

| samples | % of on-CPU+mutex | function |
|---:|---:|---|
| 16341 | 26.7 | `<tokio::runtime::task::harness::Harness<tokio::runtime::blocking::task::BlockingTask<fastopm_storage::update_manifest_in<<fastopm_store::Inn` |
| 16325 | 26.7 | `<tokio::runtime::task::core::Core<tokio::runtime::blocking::task::BlockingTask<fastopm_storage::update_manifest_in<<fastopm_store::Inner<fas` |
| 16315 | 26.7 | `<tokio::runtime::blocking::task::BlockingTask<fastopm_storage::update_manifest_in<<fastopm_store::Inner<fastopm_storage::FsStorage, fastopm_` |
| 14217 | 23.2 | `<tokio::runtime::task::harness::Harness<tokio::runtime::blocking::task::BlockingTask<fastopm_uplink::parse_manifest::{closure#0}::{closure#0` |
| 14211 | 23.2 | `<tokio::runtime::task::core::Core<tokio::runtime::blocking::task::BlockingTask<fastopm_uplink::parse_manifest::{closure#0}::{closure#0}>, to` |
| 14211 | 23.2 | `<tokio::runtime::blocking::task::BlockingTask<fastopm_uplink::parse_manifest::{closure#0}::{closure#0}> as core::future::future::Future>::po` |
| 14208 | 23.2 | `<fastopm_core::manifest::Manifest>::from_slice` |
| 14208 | 23.2 | `serde_json::de::from_trait::<serde_json::read::SliceRead, fastopm_core::manifest::Manifest>` |
| 14208 | 23.2 | `<&mut serde_json::de::Deserializer<serde_json::read::SliceRead> as serde_core::de::Deserializer>::deserialize_map::<<serde_json::map::Map<al` |
| 14189 | 23.2 | `<core::marker::PhantomData<serde_json::value::Value> as serde_core::de::DeserializeSeed>::deserialize::<&mut serde_json::de::Deserializer<se` |
| 14116 | 23.1 | `<<serde_json::value::Value as serde_core::de::Deserialize>::deserialize::ValueVisitor as serde_core::de::Visitor>::visit_map::<serde_json::d` |
| 13913 | 22.7 | `<tokio::runtime::task::harness::Harness<<fastopm_store::Inner<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::refresh::{closure#0}` |
| 13907 | 22.7 | `<fastopm_store::Inner<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::refresh::{closure#0}` |
| 13903 | 22.7 | `<fastopm_store::Inner<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::sync::{closure#0}` |
| 10131 | 16.6 | `<indexmap::inner::Core<alloc::string::String, serde_json::value::Value> as core::clone::Clone>::clone` |
| 9568 | 15.6 | `<[indexmap::Bucket<alloc::string::String, serde_json::value::Value>] as alloc::slice::SpecCloneIntoVec<indexmap::Bucket<alloc::string::Strin` |
| 9524 | 15.6 | `<core::iter::adapters::cloned::Cloned<core::slice::iter::Iter<indexmap::Bucket<alloc::string::String, serde_json::value::Value>>> as core::i` |
| 9197 | 15.0 | `<tokio::runtime::task::harness::Harness<tokio::runtime::blocking::task::BlockingTask<<fastopm_server::Server<fastopm_storage::FsStorage, fas` |
| 9196 | 15.0 | `<tokio::runtime::task::core::Core<tokio::runtime::blocking::task::BlockingTask<<fastopm_server::Server<fastopm_storage::FsStorage, fastopm_u` |
| 9195 | 15.0 | `<tokio::runtime::blocking::task::BlockingTask<<fastopm_server::Server<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::finish_json:` |
| 9195 | 15.0 | `<fastopm_server::cache::CachedBody>::encoded` |
| 9194 | 15.0 | `<std::sync::once_lock::OnceLock<bytes::bytes::Bytes>>::initialize::<<std::sync::once_lock::OnceLock<bytes::bytes::Bytes>>::get_or_init<<fast` |
| 9193 | 15.0 | `<std::sync::once::Once>::call_once_force::<<std::sync::once_lock::OnceLock<bytes::bytes::Bytes>>::initialize<<std::sync::once_lock::OnceLock` |
| 9191 | 15.0 | `fastopm_server::compress::encode` |
| 9080 | 14.8 | `<flate2::mem::Compress as flate2::zio::Ops>::run_vec` |
| 9050 | 14.8 | `miniz_oxide::deflate::stream::deflate` |
| 9049 | 14.8 | `miniz_oxide::deflate::core::compress_inner` |
| 8971 | 14.7 | `<flate2::zio::Writer<alloc::vec::Vec<u8>, flate2::mem::Compress>>::write_with_status` |
| 8789 | 14.4 | `fastopm_store::merge::update_versions_next` |
| 8437 | 13.8 | `core::ptr::drop_glue::<serde_json::value::Value>` |
| 6132 | 10.0 | `<tokio::runtime::task::harness::Harness<axum::serve::handle_connection<tokio::net::tcp::listener::TcpListener, axum::extract::connect_info::` |
| 6128 | 10.0 | `axum::serve::handle_connection::<tokio::net::tcp::listener::TcpListener, axum::extract::connect_info::IntoMakeServiceWithConnectInfo<axum::r` |
| 6127 | 10.0 | `<core::future::poll_fn::PollFn<axum::serve::handle_connection<tokio::net::tcp::listener::TcpListener, axum::extract::connect_info::IntoMakeS` |
| 6108 | 10.0 | `<hyper_util::server::conn::auto::UpgradeableConnection<hyper_util::rt::tokio::TokioIo<tokio::net::tcp::stream::TcpStream>, hyper_util::servi` |
| 6106 | 10.0 | `<hyper::server::conn::http1::UpgradeableConnection<hyper_util::common::rewind::Rewind<hyper_util::rt::tokio::TokioIo<tokio::net::tcp::stream` |
| 6025 | 9.8 | `<hyper::proto::h1::dispatch::Dispatcher<hyper::proto::h1::dispatch::Server<hyper_util::service::glue::TowerToHyperService<tower::util::map_r` |
| 6019 | 9.8 | `<hyper_util::service::oneshot::Oneshot<tower::util::map_request::MapRequest<axum::extension::AddExtension<axum::routing::Router, axum::extra` |
| 6009 | 9.8 | `<fastopm_server::router<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>::{closure#0} as axum::handler::Handler<(axum_core::extract::` |
| 6008 | 9.8 | `<fastopm_server::Server<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::handle::{closure#0}` |
| 5927 | 9.7 | `<fastopm_server::Server<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::manifest::{closure#0}` |

Mutex waits (`__psynch_mutexwait`) by nearest caller outside std/pthread, top 10:

| samples | caller |
|---:|---|
| 3296 | `<fastopm_store::Inner<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::remember` |
| 2801 | `<fastopm_store::Inner<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::load_local::{closure#0}` |
| 1656 | `<fastopm_store::Inner<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::sync::{closure#0}` |
| 481 | `<tokio::runtime::blocking::pool::Inner>::run` |
| 100 | `<tokio::runtime::blocking::pool::Spawner>::spawn_task` |
| 11 | `<fastopm_store::Inner<fastopm_storage::FsStorage, fastopm_uplink::HttpUplink>>::refresh` |
| 8 | `<fastopm_storage::lock::PackageLock as core::ops::drop::Drop>::drop` |
| 5 | `<hyper_util::client::legacy::pool::Checkout<hyper_util::client::legacy::client::PoolClient<reqwest::async_impl::body::Body>, (http::uri::sch` |
| 4 | `<std::io::stdio::Stdout as core::io::write::Write>::write_all` |
| 4 | `<tokio::runtime::time::handle::Handle>::process_at_time` |
