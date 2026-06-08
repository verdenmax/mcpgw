use catalog::Catalog;
use retrieval::{Bm25Strategy, RetrievalStrategy};

fn load_catalog() -> Catalog {
    // `cargo test` sets CWD to the crate's manifest dir (crates/retrieval) for a
    // workspace member, so resolve the workspace-root fixture via CARGO_MANIFEST_DIR.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/tools.json");
    let json =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    Catalog::from_json_str(&json).expect("fixture must be valid")
}

/// (query, expected top-1 qualified_name)
const GOLDEN: &[(&str, &str)] = &[
    ("merge a pull request", "github__merge_pull_request"),
    ("send slack chat message", "slack__post_message"),
    ("weather forecast", "weather__get_forecast"),
    ("write file to disk", "filesystem__write_file"),
];

#[test]
fn golden_top_one_matches_expected() {
    let mut s = Bm25Strategy::new();
    s.index(&load_catalog());

    for (query, expected) in GOLDEN {
        let hits = s.search(query, 5);
        assert!(!hits.is_empty(), "query {query:?} returned no hits");
        assert_eq!(
            &hits[0].qualified_name, expected,
            "query {query:?} -> got {:?}, want {expected:?}",
            hits[0].qualified_name
        );
    }
}
