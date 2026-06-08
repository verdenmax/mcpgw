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
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("weather__get_forecast"), "stdout was: {stdout}");
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
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("\"name\": \"create_issue\""), "stdout was: {stdout}");
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
