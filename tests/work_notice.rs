use std::process::Command;

#[test]
fn work_dry_run_prints_planner_granularity_notice() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("Cargo.toml"),
        r#"
[package]
name = "fixture"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();
    std::fs::create_dir_all(temp.path().join("src")).unwrap();
    std::fs::write(temp.path().join("src/lib.rs"), "pub fn fixture() {}\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_maestro"))
        .args([
            "work",
            "audit error handling in src/lib.rs",
            "--root",
            ".",
            "--agent",
            "mock",
            "--dry",
        ])
        .current_dir(temp.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("→ notice:"),
        "stdout should surface planner notice:\n{stdout}"
    );
    assert!(
        stdout.contains("planner is project-level"),
        "stdout should name project-level planner limitation:\n{stdout}"
    );
}
