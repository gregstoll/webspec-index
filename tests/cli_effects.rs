use std::process::Command;
use std::sync::OnceLock;

fn test_db_dir() -> &'static tempfile::TempDir {
    static DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    DIR.get_or_init(|| tempfile::tempdir().expect("tempdir for cli_effects tests"))
}

fn cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_webspec-index"))
        .args(args)
        .env("CODEX_SANDBOX", "1")
        .env("SPEC_INDEX_TEST_DB", test_db_dir().path().join("index.db"))
        .output()
        .expect("run webspec-index")
}

#[test]
fn effects_all_conflicts_with_a_subject_without_opening_the_index() {
    let output = cli(&["effects", "--all", "HTML#navigate"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be used with"));
}

#[test]
fn effects_selectors_are_mutually_exclusive() {
    let output = cli(&[
        "effects",
        "HTML#navigate",
        "--step",
        "2",
        "--body-id",
        "body_exact",
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be used with"));
}

#[test]
fn effects_rejects_zero_budgets_before_opening_the_index() {
    let output = cli(&["effects", "HTML#navigate", "--max-states", "0"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("0 is not in 1.."));
}

#[test]
fn effects_rebuild_requires_all() {
    let output = cli(&["effects", "HTML#navigate", "--rebuild"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--all"));
}

#[test]
fn query_views_reject_pr_previews_before_any_network_access() {
    let output = cli(&["query", "HTML#navigate", "--pr", "1", "--involving", "url"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("slice_unavailable:") && !stderr.contains("slice_slice"),
        "{stderr}"
    );
}
