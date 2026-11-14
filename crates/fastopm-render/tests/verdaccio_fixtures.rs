//! Byte-exact comparison with the bodies the harness's `verdaccio@7.1.0` produces.
//! Expected files come from `tests/fixtures/gen.cjs`.

use std::path::Path;

use fastopm_core::Manifest;
use fastopm_render::{
    BaseUrlInput, Variant, public_base_url, render_error, render_manifest, render_version,
};
use serde_json::Value;

fn fixtures() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures"))
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn s(v: &Value) -> Option<&str> {
    v.as_str()
}

/// Checks every case of one manifest fixture; returns the number of cases checked.
fn check_manifest(file: &str) -> usize {
    let input = std::fs::read(fixtures().join("input").join(file)).unwrap();
    let manifest = Manifest::from_slice(&input).unwrap();
    let expected = read_json(&fixtures().join("expected").join(file));
    let requests = expected["requests"].as_array().unwrap();
    let mut checked = 0;
    for case in expected["cases"].as_array().unwrap() {
        let request = requests.iter().find(|r| r["id"] == case["request"]).unwrap();
        let base = public_base_url(&BaseUrlInput {
            host: s(&request["host"]),
            forwarded_proto: s(&request["proto"]),
            scheme: s(&request["scheme"]).unwrap(),
            url_prefix: s(&request["prefix"]),
            public_url: None,
        })
        .unwrap();
        let what = format!("{file} {} {} {}", case["request"], case["kind"], case["query"]);
        let want_body = case["body"].as_str().unwrap();
        let status = case["status"].as_u64().unwrap();
        let result = match case["kind"].as_str().unwrap() {
            "full" => Ok(render_manifest(&manifest, Variant::Full, base.as_deref())),
            "abbreviated" => Ok(render_manifest(&manifest, Variant::Abbreviated, base.as_deref())),
            "version" => render_version(
                &manifest,
                case["query"].as_str().unwrap(),
                Variant::Full,
                base.as_deref(),
            ),
            other => panic!("unknown kind {other}"),
        };
        match result {
            Ok(rendered) => {
                assert_eq!(status, 200, "{what}: expected an error");
                assert_eq!(std::str::from_utf8(&rendered.body).unwrap(), want_body, "{what}");
                assert_eq!(rendered.etag, case["etag"].as_str().unwrap(), "{what}");
            }
            Err(err) => {
                assert_eq!(u64::from(err.client_view().0), status, "{what}: {err:?}");
                assert_eq!(std::str::from_utf8(&render_error(&err)).unwrap(), want_body, "{what}");
            }
        }
        checked += 1;
    }
    checked
}

#[test]
fn real_packuments_match_verdaccio() {
    for file in ["left-pad.json", "ms.json", "_types_ms.json", "is-number.json"] {
        assert!(check_manifest(file) > 0);
    }
}

#[test]
fn odd_manifest_matches_verdaccio() {
    assert!(check_manifest("odd.json") > 0);
}

#[test]
fn version_lookup_errors_match_verdaccio() {
    assert!(check_manifest("invalid-key.json") > 0);
}

/// Long decimals and integers beyond u64: these parse to the same double as `JSON.parse` only
/// with serde_json's `float_roundtrip` feature (enabled workspace-wide).
#[test]
fn long_decimals_match_verdaccio() {
    assert!(check_manifest("precision.json") > 0);
}

#[test]
fn public_url_matches_verdaccio() {
    let cases = read_json(&fixtures().join("expected/public-url.json"));
    for case in cases.as_array().unwrap() {
        let input = BaseUrlInput {
            host: s(&case["host"]),
            forwarded_proto: s(&case["proto"]),
            scheme: s(&case["scheme"]).unwrap(),
            url_prefix: s(&case["prefix"]),
            public_url: s(&case["publicUrl"]),
        };
        let got = match public_base_url(&input) {
            Ok(v) => v,
            Err(err) => {
                assert_eq!(err.client_view(), (500, "internal server error"));
                Some("ERROR".to_owned())
            }
        };
        assert_eq!(got.as_deref(), s(&case["result"]), "{case}");
    }
}
