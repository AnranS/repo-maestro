use super::{
    render_prompt_full, AgentAdapter, AgentResult, AgentTask, Artifacts, Capability, Usage,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::scheduler::trajectory::TrajectoryEventDraft;
use crate::schema::trajectory::{Redaction, TrajectoryEventKind, TrajectoryStatus};

/// Adapter that runs the Codex CLI in non-interactive mode.
///
/// The adapter intentionally stays small and local: maestro supplies the
/// workspace, prompt, memory, role prelude, timeout, and model. Codex owns the
/// actual coding loop. We use JSONL output when available so logs can show
/// streamed events without depending on an unstable text UI.
pub struct CodexAdapter {
    binary: String,
}

impl CodexAdapter {
    pub fn new() -> Self {
        Self {
            binary: std::env::var("MAESTRO_CODEX").unwrap_or_else(|_| "codex".into()),
        }
    }
}

impl Default for CodexAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AgentAdapter for CodexAdapter {
    fn name(&self) -> &str {
        "codex"
    }

    async fn run(&self, task: AgentTask) -> Result<AgentResult> {
        let full_prompt =
            render_prompt_full(&task.prompt, &task.context, task.role_prelude.as_deref());

        let mut cmd = Command::new(&self.binary);
        cmd.args(codex_exec_args(
            &task.workspace,
            task.model.as_deref(),
            &task.allowed_tools,
        ));

        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Own process group so a timeout SIGKILLs the whole subtree (codex plus
        // every tool it spawned), not just the direct child.
        crate::proc::isolate_process_group(&mut cmd);

        tracing::debug!(?task.task_id, ?task.workspace, "spawning codex exec --json");

        let mut log_file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&task.log_path)
            .await
            .with_context(|| format!("open log file {:?}", task.log_path))?;

        log_file
            .write_all(
                format!(
                    "[maestro] launching codex in {:?}\n[maestro] model: {}\n[maestro] context slices: {}\n[maestro] prompt:\n{}\n[maestro] ----\n",
                    task.workspace,
                    task.model.as_deref().unwrap_or("(codex default)"),
                    task.context.len(),
                    full_prompt
                )
                .as_bytes(),
            )
            .await
            .ok();
        log_file
            .write_all(
                format!(
                    "[maestro] policy: {}\n",
                    codex_policy_note(&task.allowed_tools)
                )
                .as_bytes(),
            )
            .await
            .ok();

        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawn codex for task {}", task.task_id))?;
        let child_pid = child.id();

        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(full_prompt.as_bytes())
                .await
                .context("write prompt to codex stdin")?;
            stdin.shutdown().await.ok();
        }

        let stdout = child.stdout.take().context("codex stdout pipe missing")?;
        let stderr = child.stderr.take().context("codex stderr pipe missing")?;

        let stderr_log_path = task.log_path.clone();
        let stderr_pump = tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            let mut log = match tokio::fs::OpenOptions::new()
                .append(true)
                .open(&stderr_log_path)
                .await
            {
                Ok(f) => f,
                Err(_) => return,
            };
            while let Ok(Some(line)) = lines.next_line().await {
                let red = crate::schema::redaction::redact_secret_blob(&line);
                let _ = log.write_all(b"[stderr] ").await;
                let _ = log.write_all(red.as_bytes()).await;
                let _ = log.write_all(b"\n").await;
            }
        });

        let mut trajectory = super::trajectory_writer_for(&task);
        let mut log_refs = BTreeMap::new();
        log_refs.insert("log".to_string(), super::log_ref(&task));
        let stream_fut = drive_json_stream(
            stdout,
            &mut log_file,
            trajectory.as_mut(),
            &task.task_id,
            log_refs.clone(),
        );
        let parsed = match tokio::time::timeout(task.timeout, stream_fut).await {
            Ok(result) => result?,
            Err(_) => {
                if let Some(pid) = child_pid {
                    crate::proc::kill_process_group(pid);
                }
                stderr_pump.abort();
                if let Some(writer) = trajectory.as_mut() {
                    super::append_trajectory_event(
                        writer,
                        &task.task_id,
                        TrajectoryEventDraft {
                            kind: TrajectoryEventKind::Error,
                            tool_name: Some("codex".to_string()),
                            command: None,
                            status: Some(TrajectoryStatus::Interrupted),
                            refs: log_refs,
                            usage: None,
                            redaction: Redaction::None,
                        },
                    );
                }
                anyhow::bail!("codex timeout for task {}", task.task_id);
            }
        };

        let status = child
            .wait()
            .await
            .with_context(|| format!("await codex for task {}", task.task_id))?;
        let _ = stderr_pump.await;

        if !status.success() {
            if let Some(writer) = trajectory.as_mut() {
                super::append_trajectory_event(
                    writer,
                    &task.task_id,
                    TrajectoryEventDraft {
                        kind: TrajectoryEventKind::Error,
                        tool_name: Some("codex".to_string()),
                        command: None,
                        status: Some(TrajectoryStatus::Failed),
                        refs: log_refs,
                        usage: None,
                        redaction: Redaction::None,
                    },
                );
            }
            anyhow::bail!(
                "codex exited with status {:?} (steps={})",
                status.code(),
                parsed.steps.unwrap_or(0)
            );
        }

        Ok(parsed)
    }

    fn supports(&self, cap: Capability) -> bool {
        matches!(
            cap,
            Capability::StreamOutput | Capability::WorktreeIsolation
        )
    }
}

fn codex_exec_args(
    workspace: &Path,
    model: Option<&str>,
    allow: &crate::modes::AllowedTools,
) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("exec"),
        OsString::from("--cd"),
        workspace.as_os_str().to_os_string(),
        OsString::from("--sandbox"),
        OsString::from("workspace-write"),
        OsString::from("--skip-git-repo-check"),
        OsString::from("--json"),
    ];
    if !allow.network {
        args.push(OsString::from("-c"));
        args.push(OsString::from(
            "sandbox_workspace_write.network_access=false",
        ));
    }
    if let Some(model) = model.filter(|m| !m.trim().is_empty()) {
        args.push(OsString::from("--model"));
        args.push(OsString::from(model));
    }
    args.push(OsString::from("-"));
    args
}

fn codex_policy_note(allow: &crate::modes::AllowedTools) -> String {
    let network = if allow.network {
        "network=provider default".to_string()
    } else {
        "network=hard via Codex sandbox config".to_string()
    };
    let shell = if allow.shell {
        "shell=soft prompt policy".to_string()
    } else {
        "shell=soft prompt policy (Codex CLI has no stable shell-disable flag)".to_string()
    };
    let git = if allow.git_write {
        "git_write=soft prompt policy".to_string()
    } else {
        "git_write=soft prompt policy (Codex CLI has no git-only deny flag)".to_string()
    };
    format!("{shell}; {git}; {network}")
}

async fn drive_json_stream<R>(
    stdout: R,
    log: &mut tokio::fs::File,
    mut trajectory: Option<&mut crate::scheduler::trajectory::TrajectoryWriter>,
    task_id: &str,
    log_refs: BTreeMap<String, crate::schema::artifacts::ArtifactRef>,
) -> Result<AgentResult>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut lines = BufReader::new(stdout).lines();
    let mut transcript = String::new();
    let mut last_text = String::new();
    let mut chat_id: Option<String> = None;
    let mut usage: Option<Usage> = None;
    let mut steps: u32 = 0;

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let _ = log.write_all(line.as_bytes()).await;
        let _ = log.write_all(b"\n").await;

        let value: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => {
                if let Some(writer) = trajectory.as_mut() {
                    super::append_trajectory_event(
                        writer,
                        task_id,
                        TrajectoryEventDraft {
                            kind: TrajectoryEventKind::Error,
                            tool_name: Some("codex_parse".to_string()),
                            command: None,
                            status: Some(TrajectoryStatus::Failed),
                            refs: log_refs.clone(),
                            usage: None,
                            redaction: Redaction::Partial,
                        },
                    );
                }
                transcript.push_str(&line);
                transcript.push('\n');
                continue;
            }
        };

        if let Some(id) = find_string(&value, &["session_id", "conversation_id", "sessionId"]) {
            chat_id.get_or_insert(id);
        }
        if looks_like_tool_event(&value) {
            steps += 1;
            if let Some(writer) = trajectory.as_mut() {
                let (command, redaction) = trajectory_command_redacted(&value);
                super::append_trajectory_event(
                    writer,
                    task_id,
                    TrajectoryEventDraft {
                        kind: TrajectoryEventKind::ToolCall,
                        tool_name: trajectory_tool_name(&value),
                        command,
                        status: trajectory_status(&value),
                        refs: log_refs.clone(),
                        usage: None,
                        redaction,
                    },
                );
            }
        }
        if let Some(u) = parse_usage(&value) {
            if let Some(writer) = trajectory.as_mut() {
                super::append_trajectory_event(
                    writer,
                    task_id,
                    TrajectoryEventDraft {
                        kind: TrajectoryEventKind::Usage,
                        tool_name: None,
                        command: None,
                        status: Some(TrajectoryStatus::Completed),
                        refs: log_refs.clone(),
                        usage: Some(u.clone()),
                        redaction: Redaction::None,
                    },
                );
            }
            usage = Some(u);
        }
        if let Some(text) = find_text(&value) {
            if !text.trim().is_empty() {
                last_text = text.clone();
                transcript.push_str(&text);
                if !text.ends_with('\n') {
                    transcript.push('\n');
                }
                if let Some(writer) = trajectory.as_mut() {
                    super::append_trajectory_event(
                        writer,
                        task_id,
                        TrajectoryEventDraft {
                            kind: TrajectoryEventKind::Output,
                            tool_name: None,
                            command: None,
                            status: Some(TrajectoryStatus::Completed),
                            refs: log_refs.clone(),
                            usage: None,
                            redaction: Redaction::None,
                        },
                    );
                }
            }
        }
        if !looks_like_tool_event(&value)
            && parse_usage(&value).is_none()
            && find_text(&value).is_none()
        {
            if let Some(writer) = trajectory.as_mut() {
                super::append_trajectory_event(
                    writer,
                    task_id,
                    TrajectoryEventDraft {
                        kind: TrajectoryEventKind::Output,
                        tool_name: Some("codex.unknown".to_string()),
                        command: None,
                        status: Some(TrajectoryStatus::Completed),
                        refs: log_refs.clone(),
                        usage: None,
                        redaction: Redaction::None,
                    },
                );
            }
        }
    }

    let summary = if !last_text.trim().is_empty() {
        last_text
    } else {
        transcript.trim().to_string()
    };

    let _ = log
        .write_all(format!("[maestro] ---- ({steps} codex event step(s))\n").as_bytes())
        .await;
    if let Some(writer) = trajectory.as_mut() {
        super::append_trajectory_event(
            writer,
            task_id,
            TrajectoryEventDraft {
                kind: TrajectoryEventKind::Final,
                tool_name: Some("codex".to_string()),
                command: None,
                status: Some(TrajectoryStatus::Completed),
                refs: log_refs,
                usage: usage.clone(),
                redaction: Redaction::None,
            },
        );
    }

    Ok(AgentResult {
        chat_id,
        artifacts: Artifacts::default(),
        transcript_summary: summary,
        usage,
        steps: if steps > 0 { Some(steps) } else { None },
    })
}

fn trajectory_tool_name(value: &Value) -> Option<String> {
    find_string(value, &["tool_name", "tool", "command_name", "type"])
}

fn trajectory_command(value: &Value) -> Option<String> {
    find_string(value, &["command", "cmd", "shell_command"])
}

fn trajectory_command_redacted(value: &Value) -> (Option<String>, Redaction) {
    let Some(command) = trajectory_command(value) else {
        return (None, Redaction::None);
    };
    let (redacted, marker) = crate::schema::redaction::redact_secret_text(&command);
    (Some(redacted), marker)
}

fn trajectory_status(value: &Value) -> Option<TrajectoryStatus> {
    let raw = find_string(value, &["status", "state"])?;
    let status = raw.to_ascii_lowercase();
    if status.contains("start") || status.contains("progress") || status.contains("running") {
        Some(TrajectoryStatus::Started)
    } else if status.contains("complete") || status.contains("success") || status == "ok" {
        Some(TrajectoryStatus::Completed)
    } else if status.contains("fail") || status.contains("error") {
        Some(TrajectoryStatus::Failed)
    } else if status.contains("interrupt") || status.contains("cancel") {
        Some(TrajectoryStatus::Interrupted)
    } else {
        None
    }
}

fn find_text(value: &Value) -> Option<String> {
    for key in [
        "result",
        "text",
        "content",
        "message",
        "output",
        "last_message",
    ] {
        if let Some(s) = find_string(value, &[key]) {
            return Some(s);
        }
    }
    None
}

fn find_string(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(Value::String(s)) = map.get(*key) {
                    return Some(s.clone());
                }
            }
            for child in map.values() {
                if let Some(s) = find_string(child, keys) {
                    return Some(s);
                }
            }
            None
        }
        Value::Array(items) => items.iter().find_map(|v| find_string(v, keys)),
        _ => None,
    }
}

fn looks_like_tool_event(value: &Value) -> bool {
    contains_toolish_type(value)
}

fn contains_toolish_type(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.iter().any(|(key, child)| {
            if matches!(key.as_str(), "type" | "event" | "kind") && is_toolish_type_value(child) {
                return true;
            }
            contains_toolish_type(child)
        }),
        Value::Array(items) => items.iter().any(contains_toolish_type),
        _ => false,
    }
}

fn is_toolish_type_value(value: &Value) -> bool {
    let Some(s) = value.as_str() else {
        return false;
    };
    let k = s.to_ascii_lowercase();
    k.contains("tool") || k.contains("exec") || k.contains("command") || k.contains("file_change")
}

fn parse_usage(value: &Value) -> Option<Usage> {
    let usage_obj = value.get("usage").or_else(|| value.get("token_usage"))?;
    let input = usage_obj
        .get("input_tokens")
        .or_else(|| usage_obj.get("prompt_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let output = usage_obj
        .get("output_tokens")
        .or_else(|| usage_obj.get("completion_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let model = usage_obj
        .get("model")
        .or_else(|| value.get("model"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Some(Usage {
        input_tokens: input,
        output_tokens: output,
        cost_usd: None,
        model,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{AgentAdapter, AgentTask, ExecutionMode, TrajectoryContext};
    use std::path::PathBuf;
    use std::time::Duration;

    #[test]
    fn codex_args_match_current_exec_cli() {
        let args = codex_exec_args(
            &PathBuf::from("/tmp/work"),
            Some("gpt-5"),
            &crate::modes::AllowedTools::default(),
        );
        let rendered: Vec<String> = args
            .iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect();
        assert_eq!(
            rendered,
            vec![
                "exec",
                "--cd",
                "/tmp/work",
                "--sandbox",
                "workspace-write",
                "--skip-git-repo-check",
                "--json",
                "--model",
                "gpt-5",
                "-"
            ]
        );
        assert!(!rendered.iter().any(|arg| arg == "--ask-for-approval"));
    }

    #[test]
    fn codex_args_disable_network_when_policy_disallows_it() {
        let allow = crate::modes::AllowedTools {
            network: false,
            ..Default::default()
        };
        let args = codex_exec_args(&PathBuf::from("/tmp/work"), None, &allow);
        let rendered = args
            .iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert!(rendered.contains(&"-c".to_string()));
        assert!(rendered.contains(&"sandbox_workspace_write.network_access=false".to_string()));
    }

    #[test]
    fn counts_nested_codex_tool_events() {
        let command_event: Value = serde_json::json!({
            "type": "item.completed",
            "item": { "type": "command_execution", "status": "completed" }
        });
        let file_event: Value = serde_json::json!({
            "type": "item.started",
            "item": { "type": "file_change", "status": "in_progress" }
        });
        let message_event: Value = serde_json::json!({
            "type": "item.completed",
            "item": { "type": "agent_message", "text": "done" }
        });

        assert!(looks_like_tool_event(&command_event));
        assert!(looks_like_tool_event(&file_event));
        assert!(!looks_like_tool_event(&message_event));
    }

    #[test]
    fn parses_nested_usage_objects() {
        let v: Value = serde_json::json!({
            "usage": {
                "input_tokens": 11,
                "output_tokens": 7,
                "model": "gpt-5"
            }
        });
        let usage = parse_usage(&v).unwrap();
        assert_eq!(usage.input_tokens, 11);
        assert_eq!(usage.output_tokens, 7);
        assert_eq!(usage.model.as_deref(), Some("gpt-5"));
    }

    #[tokio::test]
    async fn writes_codex_trajectory_for_tool_usage_and_final_events() {
        let temp = tempfile::tempdir().unwrap();
        let log_path = temp.path().join("codex.log");
        let mut log = tokio::fs::File::create(&log_path).await.unwrap();
        let mut writer = crate::scheduler::trajectory::TrajectoryWriter::new(
            temp.path(),
            "run-1",
            "T_codex",
            "codex",
        )
        .unwrap();
        let trajectory_path = writer.path();
        let mut refs = BTreeMap::new();
        refs.insert(
            "log".to_string(),
            crate::schema::artifacts::ArtifactRef {
                kind: "log".to_string(),
                source: crate::schema::artifacts::ArtifactSource::AgentTask,
                task_id: Some("T_codex".to_string()),
                path: Some(log_path.to_string_lossy().to_string()),
                uri: None,
                name: None,
                bytes: None,
            },
        );
        let input = concat!(
            r#"{"type":"item.completed","item":{"type":"command_execution","status":"completed","command":"cargo test"}}"#,
            "\n",
            r#"{"usage":{"input_tokens":11,"output_tokens":7,"model":"gpt-5"}}"#,
            "\n",
            r#"{"type":"result","result":"done"}"#,
            "\n"
        );

        let result = drive_json_stream(
            input.as_bytes(),
            &mut log,
            Some(&mut writer),
            "T_codex",
            refs,
        )
        .await
        .unwrap();

        assert_eq!(result.transcript_summary, "done");
        let events = crate::scheduler::trajectory::read_trajectory(&trajectory_path).unwrap();
        assert!(events
            .iter()
            .any(|event| event.kind == crate::schema::trajectory::TrajectoryEventKind::ToolCall));
        assert!(events
            .iter()
            .any(|event| event.kind == crate::schema::trajectory::TrajectoryEventKind::Usage));
        assert_eq!(
            events.last().unwrap().kind,
            crate::schema::trajectory::TrajectoryEventKind::Final
        );
    }

    #[tokio::test]
    async fn writes_codex_unknown_events_as_output_markers_with_log_ref() {
        let temp = tempfile::tempdir().unwrap();
        let log_path = temp.path().join("codex.log");
        let mut log = tokio::fs::File::create(&log_path).await.unwrap();
        let mut writer = crate::scheduler::trajectory::TrajectoryWriter::new(
            temp.path(),
            "run-1",
            "T_codex",
            "codex",
        )
        .unwrap();
        let trajectory_path = writer.path();
        let mut refs = BTreeMap::new();
        refs.insert(
            "log".to_string(),
            crate::schema::artifacts::ArtifactRef {
                kind: "log".to_string(),
                source: crate::schema::artifacts::ArtifactSource::AgentTask,
                task_id: Some("T_codex".to_string()),
                path: Some(log_path.to_string_lossy().to_string()),
                uri: None,
                name: None,
                bytes: None,
            },
        );

        drive_json_stream(
            br#"{"type":"future_event","payload":{"shape":"new"}}"#.as_slice(),
            &mut log,
            Some(&mut writer),
            "T_codex",
            refs,
        )
        .await
        .unwrap();

        let events = crate::scheduler::trajectory::read_trajectory(&trajectory_path).unwrap();
        assert!(events.iter().any(|event| {
            event.kind == crate::schema::trajectory::TrajectoryEventKind::Output
                && event.tool_name.as_deref() == Some("codex.unknown")
                && event.refs.contains_key("log")
        }));
    }

    #[tokio::test]
    async fn writes_codex_parse_errors_as_partial_redaction_events() {
        let temp = tempfile::tempdir().unwrap();
        let log_path = temp.path().join("codex.log");
        let mut log = tokio::fs::File::create(&log_path).await.unwrap();
        let mut writer = crate::scheduler::trajectory::TrajectoryWriter::new(
            temp.path(),
            "run-1",
            "T_codex",
            "codex",
        )
        .unwrap();
        let trajectory_path = writer.path();
        let mut refs = BTreeMap::new();
        refs.insert(
            "log".to_string(),
            crate::schema::artifacts::ArtifactRef {
                kind: "log".to_string(),
                source: crate::schema::artifacts::ArtifactSource::AgentTask,
                task_id: Some("T_codex".to_string()),
                path: Some(log_path.to_string_lossy().to_string()),
                uri: None,
                name: None,
                bytes: None,
            },
        );

        drive_json_stream(
            b"{not-json}\n".as_slice(),
            &mut log,
            Some(&mut writer),
            "T_codex",
            refs,
        )
        .await
        .unwrap();

        let events = crate::scheduler::trajectory::read_trajectory(&trajectory_path).unwrap();
        assert!(events.iter().any(|event| {
            event.kind == crate::schema::trajectory::TrajectoryEventKind::Error
                && event.tool_name.as_deref() == Some("codex_parse")
                && event.status == Some(crate::schema::trajectory::TrajectoryStatus::Failed)
                && event.redaction == crate::schema::trajectory::Redaction::Partial
        }));
    }

    #[tokio::test]
    async fn codex_timeout_records_interrupted_trajectory() {
        let temp = tempfile::tempdir().unwrap();
        let fake_codex = temp.path().join("fake-codex");
        std::fs::write(&fake_codex, "#!/bin/sh\nsleep 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&fake_codex).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&fake_codex, perms).unwrap();
        }

        let trajectory_path = temp.path().join("T_codex_timeout.ndjson");
        let task = AgentTask {
            task_id: "T_codex_timeout".to_string(),
            workspace: temp.path().to_path_buf(),
            prompt: "hello".to_string(),
            context: Vec::new(),
            timeout: Duration::from_millis(20),
            mode: ExecutionMode::Apply,
            resume_chat_id: None,
            log_path: temp.path().join("codex-timeout.log"),
            trajectory: Some(TrajectoryContext {
                run_id: "run-1".to_string(),
                task_id: "T_codex_timeout".to_string(),
                provider_id: "codex".to_string(),
                path: trajectory_path.clone(),
            }),
            model: None,
            role_prelude: None,
            allowed_tools: Default::default(),
        };

        let adapter = CodexAdapter {
            binary: fake_codex.to_string_lossy().to_string(),
        };
        let err = adapter.run(task).await.unwrap_err();
        assert!(err.to_string().contains("codex timeout"));
        let events = crate::scheduler::trajectory::read_trajectory(&trajectory_path).unwrap();
        assert!(events.iter().any(|event| {
            event.kind == crate::schema::trajectory::TrajectoryEventKind::Error
                && event.status == Some(crate::schema::trajectory::TrajectoryStatus::Interrupted)
        }));
    }
}
