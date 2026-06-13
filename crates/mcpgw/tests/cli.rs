use std::path::PathBuf;
use std::process::Command;

/// Path to the compiled `mcpgw` binary provided by Cargo to integration tests.
fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mcpgw")
}

/// Shared fixture at the workspace root, resolved relative to this crate so it
/// works regardless of the test's current working directory.
fn fixture() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/tools.json")
}

#[test]
fn search_subcommand_returns_relevant_tool() {
    let out = Command::new(bin())
        .arg("--catalog")
        .arg(fixture())
        .arg("search")
        .arg("weather forecast")
        .arg("--top-k")
        .arg("1")
        .output()
        .expect("run mcpgw");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();

    // Output must be a JSON array, and --top-k 1 must actually cap it to one element.
    let results: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout is not JSON: {e}\n{stdout}"));
    let arr = results.as_array().expect("output is a JSON array");
    assert_eq!(
        arr.len(),
        1,
        "--top-k 1 should return exactly one hit, got: {stdout}"
    );
    assert_eq!(
        arr[0]["name"], "weather__get_forecast",
        "stdout was: {stdout}"
    );
}

#[test]
fn get_details_subcommand_prints_tool() {
    let out = Command::new(bin())
        .arg("--catalog")
        .arg(fixture())
        .arg("get-details")
        .arg("github__create_issue")
        .output()
        .expect("run mcpgw");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("\"name\": \"create_issue\""),
        "stdout was: {stdout}"
    );
}

#[test]
fn get_details_unknown_tool_fails() {
    let out = Command::new(bin())
        .arg("--catalog")
        .arg(fixture())
        .arg("get-details")
        .arg("nope__missing")
        .output()
        .expect("run mcpgw");
    assert!(!out.status.success());
}

/// Write a throwaway TOML config to a unique temp path for `--config` tests.
fn write_temp_config(tag: &str, contents: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("mcpgw_cli_{}_{}.toml", std::process::id(), tag));
    std::fs::write(&path, contents).expect("write temp config");
    path
}

#[test]
fn search_uses_top_k_from_config_file() {
    let cfg = write_temp_config("topk", "[retrieval]\ntop_k = 2\n");
    let out = Command::new(bin())
        .arg("--catalog")
        .arg(fixture())
        .arg("--config")
        .arg(&cfg)
        .arg("search")
        .arg("github pull request repository issue")
        .output()
        .expect("run mcpgw");
    let _ = std::fs::remove_file(&cfg);

    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    let results: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout is not JSON: {e}\n{stdout}"));
    let arr = results.as_array().expect("output is a JSON array");
    // The query matches 3 github tools; the config's top_k=2 must cap the result
    // count to 2 (the built-in default of 8 would return all 3). This proves the
    // --config file was actually read and its top_k applied.
    assert_eq!(
        arr.len(),
        2,
        "config top_k=2 should cap results, got: {stdout}"
    );
}

#[test]
fn unimplemented_strategy_in_config_fails() {
    let cfg = write_temp_config("vector", "[retrieval]\nstrategy = \"vector\"\n");
    let out = Command::new(bin())
        .arg("--catalog")
        .arg(fixture())
        .arg("--config")
        .arg(&cfg)
        .arg("search")
        .arg("weather")
        .output()
        .expect("run mcpgw");
    let _ = std::fs::remove_file(&cfg);

    // "vector" is config-valid but the offline `search` CLI builds no embedder, so it must
    // surface as a runtime error (non-zero exit) through the binary.
    assert!(
        !out.status.success(),
        "expected failure for strategy requiring an embedder"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("requires an embedder"),
        "stderr was: {stderr}"
    );
}
