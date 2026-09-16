use std::process::Command;

fn cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_webspec-index"))
        .args(args)
        .env("CODEX_SANDBOX", "1")
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
