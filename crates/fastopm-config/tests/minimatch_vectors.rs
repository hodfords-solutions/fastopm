//! `pattern_matches` against vectors generated from the real minimatch package.
//!
//! Regenerate with `node tests/fixtures/gen-minimatch.mjs <minimatch dir> tests/fixtures/minimatch-vectors.json`
//! using the minimatch that `verdaccio@7.1.0` installs (11.0.3).

use fastopm_config::pattern_matches;

#[test]
fn agrees_with_minimatch() {
    let raw = include_str!("fixtures/minimatch-vectors.json");
    let doc: serde_json::Value = serde_json::from_str(raw).expect("fixture is JSON");
    let strings = |key: &str| -> Vec<String> {
        doc[key]
            .as_array()
            .expect("array")
            .iter()
            .map(|v| v.as_str().expect("string").to_owned())
            .collect()
    };
    let patterns = strings("patterns");
    let names = strings("names");
    let results = strings("results");
    assert_eq!(patterns.len(), results.len());

    let mut failures = Vec::new();
    for (pattern, row) in patterns.iter().zip(&results) {
        for (name, expected) in names.iter().zip(row.chars()) {
            let expected = expected == '1';
            if pattern_matches(pattern, name) != expected {
                failures.push(format!("minimatch({name:?}, {pattern:?}) should be {expected}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} vectors disagree:\n{}",
        failures.len(),
        patterns.len() * names.len(),
        failures.join("\n")
    );
}
