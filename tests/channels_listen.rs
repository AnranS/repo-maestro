use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn channels_listen_prints_dispatch_decision() {
    let temp = workspace_with_channels();
    let output = run_listen(
        temp.path(),
        serde_json::json!({
            "thread_id": "thread-1",
            "sender_open_id": "ou_owner",
            "message_id": "om_xxx",
            "create_time": "2026-05-24T08:00:00Z",
            "body": "plan",
            "attachments": []
        })
        .to_string(),
    );

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.starts_with(r#"{"Dispatch":"#),
        "unexpected stdout: {stdout}"
    );
}

#[test]
fn channels_listen_prints_reject_decision_for_untrusted_run() {
    let temp = workspace_with_channels();
    let output = run_listen(
        temp.path(),
        serde_json::json!({
            "thread_id": "thread-1",
            "sender_open_id": "ou_untrusted",
            "message_id": "om_xxx",
            "create_time": "2026-05-24T08:00:00Z",
            "body": "run --run",
            "attachments": []
        })
        .to_string(),
    );

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.starts_with(r#"{"RejectWithReply":"#),
        "unexpected stdout: {stdout}"
    );
}

fn workspace_with_channels() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let config_dir = temp.path().join(".maestro");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("channels.yaml"),
        r#"
version: 1
channels:
  feishu:
    enabled: true
    allowed_senders:
      - open_id: ou_owner
    allowed_actions: [plan, run, approve, status]
"#,
    )
    .unwrap();
    temp
}

fn run_listen(workspace: &std::path::Path, input: String) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_maestro"))
        .args(["channels", "listen", "--once", "--from-stdin"])
        .current_dir(workspace)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}
