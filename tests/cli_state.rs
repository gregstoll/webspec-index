use std::process::Command;

#[test]
fn state_requires_a_selector_before_opening_the_index() {
    let output = Command::new(env!("CARGO_BIN_EXE_webspec-index"))
        .args(["state"])
        .env("CODEX_SANDBOX", "1")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("<SELECTOR>"));
}

#[test]
fn coverage_conflicts_with_a_selector() {
    let output = Command::new(env!("CARGO_BIN_EXE_webspec-index"))
        .args(["state", "--coverage", "HTML", "HTML#x"])
        .env("CODEX_SANDBOX", "1")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be used with"));
}
