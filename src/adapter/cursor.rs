use super::{
    render_prompt_full, AgentAdapter, AgentResult, AgentTask, Artifacts, Capability, Usage,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

/// Adapter that shells out to the `cursor-agent` CLI.
///
/// Uses `--output-format stream-json --stream-partial-output` so we can
/// surface the agent's intermediate thinking — tool calls, partial text —
/// into the task log file in real time. The final result + token usage
/// still come from a single `result` event at the end of the stream.
pub struct CursorAdapter {
    binary: String,
}

impl CursorAdapter {
    pub fn new() -> Self {
        Self {
            binary: std::env::var("MAESTRO_CURSOR_AGENT").unwrap_or_else(|_| "cursor-agent".into()),
        }
    }
}

impl Default for CursorAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AgentAdapter for CursorAdapter {
    fn name(&self) -> &str {
        "cursor"
    }

    async fn run(&self, task: AgentTask) -> Result<AgentResult> {
        let full_prompt =
            render_prompt_full(&task.prompt, &task.context, task.role_prelude.as_deref());

        let mut cmd = Command::new(&self.binary);
        cmd.args(cursor_agent_args(
            &full_prompt,
            &task.workspace,
            task.model.as_deref(),
            task.resume_chat_id.as_deref(),
            &task.allowed_tools,
        ));

        cmd.stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Own process group so a timeout SIGKILLs the whole subtree (cursor-agent
        // plus every tool it spawned), not just the direct child.
        crate::proc::isolate_process_group(&mut cmd);
        // F-136b1: rebuild the child env from an allowlist (off by default) so
        // unrelated env-borne secrets are not handed to the untrusted agent.
        if task.harden.scrub_env {
            crate::runtime_harden::apply_env_scrub(
                &mut cmd,
                self.name(),
                &task.harden.extra_allow_env,
            );
        }

        tracing::debug!(?task.task_id, ?task.workspace, "spawning cursor-agent (stream-json)");

        // F-136b1: cap the task log (0 = unbounded passthrough, the default).
        let mut log_file =
            crate::runtime_harden::CappedLog::open(&task.log_path, task.harden.max_task_log_bytes)
                .await
                .with_context(|| format!("open log file {:?}", task.log_path))?;

        log_file
            .write_all(
                format!(
                    "[maestro] launching cursor-agent in {:?}\n[maestro] model: {}\n[maestro] context slices: {}\n[maestro] prompt:\n{}\n[maestro] ----\n",
                    task.workspace,
                    task.model.as_deref().unwrap_or("(cursor default)"),
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
                    cursor_policy_note(&task.allowed_tools)
                )
                .as_bytes(),
            )
            .await
            .ok();

        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawn cursor-agent for task {}", task.task_id))?;
        let child_pid = child.id();

        let stdout = child
            .stdout
            .take()
            .context("cursor-agent stdout pipe missing")?;
        let stderr = child
            .stderr
            .take()
            .context("cursor-agent stderr pipe missing")?;

        // stderr is drained in a background task and tee'd into the log so a
        // noisy CLI doesn't fill its pipe and block stdout. F-136b1 B3: the
        // shared pump hard-bounds it (disk + memory via a chunked drain),
        // writes a marker when it caps, and reads to the child's real EOF so the
        // pipe never blocks the child.
        let stderr_log_path = task.log_path.clone();
        let stderr_cap = task.harden.max_task_log_bytes;
        let stderr_pump = tokio::spawn(async move {
            crate::runtime_harden::pump_stderr_capped(stderr, &stderr_log_path, stderr_cap).await;
        });

        let stream_fut = drive_stream(stdout, &mut log_file);
        let outcome = tokio::time::timeout(task.timeout, stream_fut).await;
        // On timeout, tear down the whole subtree before finalizing the log.
        if outcome.is_err() {
            if let Some(pid) = child_pid {
                crate::proc::kill_process_group(pid);
            }
            stderr_pump.abort();
        }
        // F-136b1 B2: finalize on EVERY path (success / parse-error / timeout) so
        // a truncated log always gets its marker + tail — never silent. Idempotent.
        let _ = log_file.finalize().await;
        let parsed = match outcome {
            Ok(result) => result?,
            Err(_) => anyhow::bail!("cursor-agent timeout for task {}", task.task_id),
        };

        let status = child
            .wait()
            .await
            .with_context(|| format!("await cursor-agent for task {}", task.task_id))?;
        let _ = stderr_pump.await;

        if !status.success() {
            anyhow::bail!(
                "cursor-agent exited with status {:?} (steps={})",
                status.code(),
                parsed.steps.unwrap_or(0)
            );
        }

        Ok(parsed)
    }

    fn supports(&self, cap: Capability) -> bool {
        matches!(
            cap,
            Capability::StreamOutput | Capability::Resume | Capability::WorktreeIsolation
        )
    }
}

fn cursor_agent_args(
    prompt: &str,
    workspace: &Path,
    model: Option<&str>,
    resume_chat_id: Option<&str>,
    allow: &crate::modes::AllowedTools,
) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("-p"),
        OsString::from(prompt),
        OsString::from("--workspace"),
        workspace.as_os_str().to_os_string(),
        OsString::from("--output-format"),
        OsString::from("stream-json"),
        OsString::from("--stream-partial-output"),
    ];
    if cursor_policy_is_restricted(allow) {
        args.push(OsString::from("--sandbox"));
        args.push(OsString::from("enabled"));
    } else {
        args.push(OsString::from("--force"));
    }
    args.push(OsString::from("--trust"));

    if let Some(model) = model.filter(|m| !m.trim().is_empty()) {
        args.push(OsString::from("--model"));
        args.push(OsString::from(model));
    }

    if let Some(id) = resume_chat_id.filter(|id| !id.trim().is_empty()) {
        args.push(OsString::from("--resume"));
        args.push(OsString::from(id));
    }
    args
}

fn cursor_policy_is_restricted(allow: &crate::modes::AllowedTools) -> bool {
    !allow.shell || !allow.git_write || !allow.network || !allow.allowed_commands.is_empty()
}

fn cursor_policy_note(allow: &crate::modes::AllowedTools) -> String {
    if cursor_policy_is_restricted(allow) {
        "sandbox=hard via cursor-agent --sandbox enabled; fine-grained shell/git/network limits=soft prompt policy"
            .to_string()
    } else {
        "unrestricted provider defaults (--force)".to_string()
    }
}

/// Pump `cursor-agent --output-format stream-json` line by line into both
/// the task log (digested for humans) and a final `AgentResult`. We:
///   - accumulate assistant deltas as the "transcript",
///   - track each `tool_use` event as one step (and log "[tool] NAME"),
///   - capture session id from `system`/`assistant`/`result` events,
///   - take the canonical reply from the `result` event,
///   - parse `usage` from the same `result`.
async fn drive_stream<R>(
    stdout: R,
    log: &mut crate::runtime_harden::CappedLog,
) -> Result<AgentResult>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut lines = BufReader::new(stdout).lines();

    let mut streamed_text = String::new();
    let mut final_text = String::new();
    let mut chat_id: Option<String> = None;
    let mut usage: Option<Usage> = None;
    let mut steps: u32 = 0;

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let kind = value.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match kind {
            "system" => {
                if let Some(sid) = value.get("session_id").and_then(|v| v.as_str()) {
                    chat_id.get_or_insert_with(|| sid.to_string());
                }
            }
            "assistant" => {
                let mut chunk = String::new();
                if let Some(content) = value.pointer("/message/content").and_then(|v| v.as_array())
                {
                    for piece in content {
                        // Track tool_use entries that show up inline.
                        if piece.get("type").and_then(|v| v.as_str()) == Some("tool_use") {
                            steps += 1;
                            let tool_name = piece
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("(unknown)");
                            let _ = log
                                .write_all(format!("[tool] {tool_name}\n").as_bytes())
                                .await;
                        }
                        if let Some(t) = piece.get("text").and_then(|v| v.as_str()) {
                            chunk.push_str(t);
                        }
                    }
                }
                if !chunk.is_empty() {
                    streamed_text.push_str(&chunk);
                    // Don't echo every delta into the log — assistants emit
                    // a final cumulative copy at the end and it'd duplicate.
                }
                if let Some(sid) = value.get("session_id").and_then(|v| v.as_str()) {
                    chat_id.get_or_insert_with(|| sid.to_string());
                }
            }
            "tool_use" | "tool_call" => {
                steps += 1;
                let tool_name = value
                    .pointer("/tool/name")
                    .or_else(|| value.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("(unknown)");
                let _ = log
                    .write_all(format!("[tool] {tool_name}\n").as_bytes())
                    .await;
            }
            "result" => {
                if let Some(t) = value.get("result").and_then(|v| v.as_str()) {
                    final_text = t.to_string();
                }
                if let Some(sid) = value.get("session_id").and_then(|v| v.as_str()) {
                    chat_id.get_or_insert_with(|| sid.to_string());
                }
                if let Some(u) = parse_usage(&value) {
                    usage = Some(u);
                }
            }
            _ => {}
        }
    }

    let transcript = if !final_text.is_empty() {
        final_text
    } else {
        streamed_text
    };

    let _ = log
        .write_all(format!("[maestro] ---- ({steps} step(s))\n").as_bytes())
        .await;
    if !transcript.is_empty() {
        let _ = log.write_all(transcript.as_bytes()).await;
        if !transcript.ends_with('\n') {
            let _ = log.write_all(b"\n").await;
        }
    }

    Ok(AgentResult {
        chat_id,
        artifacts: Artifacts::default(),
        transcript_summary: transcript,
        usage,
        steps: if steps > 0 { Some(steps) } else { None },
    })
}

/// Look for the `usage` object cursor-agent puts in its final JSON. Field
/// names vary across versions, so try the obvious aliases. Returns `None`
/// if nothing usable is found — callers expect the absence to mean
/// "adapter didn't report it".
pub(crate) fn parse_usage(json: &serde_json::Value) -> Option<Usage> {
    let usage_obj = json.get("usage").or_else(|| json.get("token_usage"))?;

    let input = usage_obj
        .get("input_tokens")
        .or_else(|| usage_obj.get("prompt_tokens"))
        .or_else(|| usage_obj.get("inputTokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let output = usage_obj
        .get("output_tokens")
        .or_else(|| usage_obj.get("completion_tokens"))
        .or_else(|| usage_obj.get("outputTokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let cost = usage_obj
        .get("cost_usd")
        .or_else(|| usage_obj.get("total_cost_usd"))
        .or_else(|| usage_obj.get("cost"))
        .and_then(|v| v.as_f64());
    let model = json
        .get("model")
        .or_else(|| usage_obj.get("model"))
        .and_then(|v| v.as_str())
        .map(String::from);

    if input == 0 && output == 0 && cost.is_none() {
        return None;
    }
    Some(Usage {
        input_tokens: input,
        output_tokens: output,
        cost_usd: cost,
        model,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn run_drive(events: &[&str]) -> AgentResult {
        let stream = events.join("\n");
        let cursor = std::io::Cursor::new(stream.into_bytes());
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let mut log = crate::runtime_harden::CappedLog::open(tmp.path(), 0)
            .await
            .unwrap();
        drive_stream(tokio::io::BufReader::new(cursor), &mut log)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn parses_session_id_and_result_from_stream() {
        let r = run_drive(&[
            r#"{"type":"system","session_id":"sess-42"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi "}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"world"}]}}"#,
            r#"{"type":"result","session_id":"sess-42","result":"all done","usage":{"input_tokens":12,"output_tokens":3,"cost_usd":0.001}}"#,
        ])
        .await;
        assert_eq!(r.chat_id.as_deref(), Some("sess-42"));
        assert_eq!(r.transcript_summary, "all done");
        let u = r.usage.expect("usage parsed");
        assert_eq!(u.input_tokens, 12);
        assert_eq!(u.output_tokens, 3);
        assert_eq!(u.cost_usd, Some(0.001));
    }

    #[tokio::test]
    async fn counts_tool_use_events_as_steps() {
        let r = run_drive(&[
            r#"{"type":"system","session_id":"s"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"read_file"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"edit_file"}]}}"#,
            r#"{"type":"tool_use","name":"grep"}"#,
            r#"{"type":"result","result":"ok"}"#,
        ])
        .await;
        assert_eq!(r.steps, Some(3));
    }

    #[tokio::test]
    async fn falls_back_to_streamed_text_when_no_result_event() {
        let r = run_drive(&[
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"only delta"}]}}"#,
        ])
        .await;
        assert_eq!(r.transcript_summary, "only delta");
        assert!(r.usage.is_none());
        assert!(r.steps.is_none());
    }

    #[tokio::test]
    async fn ignores_malformed_lines() {
        // junk lines mixed in should not crash the loop
        let r = run_drive(&["{not json", r#"{"type":"result","result":"survived"}"#]).await;
        assert_eq!(r.transcript_summary, "survived");
    }

    #[test]
    fn restricted_policy_enables_sandbox_and_omits_force() {
        let allow = crate::modes::AllowedTools {
            allowed_commands: vec!["npm test*".into()],
            ..Default::default()
        };
        let args = cursor_agent_args(
            "prompt",
            Path::new("/tmp/work"),
            Some("gpt-5"),
            Some("sess-1"),
            &allow,
        );
        let rendered = args
            .iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert!(rendered.contains(&"--sandbox".to_string()));
        assert!(rendered.contains(&"enabled".to_string()));
        assert!(!rendered.contains(&"--force".to_string()));
        assert!(rendered.contains(&"--model".to_string()));
        assert!(rendered.contains(&"--resume".to_string()));
    }

    /// B2: even when the agent exits non-zero (so `run` returns Err), a log that
    /// exceeded the cap must still carry the truncation marker — finalize runs on
    /// every path, not just the clean-parse path. Drives a fake cursor-agent
    /// (Unix shell) that over-produces valid stream-json then exits 1.
    #[cfg(unix)]
    #[tokio::test]
    async fn finalizes_truncation_marker_even_when_agent_exits_nonzero() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("fake-cursor.sh");
        std::fs::write(
            &fake,
            "#!/bin/sh\ni=0\nwhile [ $i -lt 40 ]; do printf '{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"chunk %s with padding padding padding padding\"}]}}\\n' \"$i\"; i=$((i+1)); done\nprintf '{\"type\":\"result\",\"result\":\"done\"}\\n'\nexit 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

        let log_path = dir.path().join("task.log");
        let task = AgentTask {
            task_id: "T_cap".into(),
            workspace: dir.path().to_path_buf(),
            prompt: "hi".into(),
            context: Vec::new(),
            timeout: std::time::Duration::from_secs(30),
            mode: crate::adapter::ExecutionMode::Apply,
            resume_chat_id: None,
            log_path: log_path.clone(),
            trajectory: None,
            model: None,
            role_prelude: None,
            allowed_tools: Default::default(),
            harden: crate::runtime_harden::Hardening {
                scrub_env: false,
                extra_allow_env: Vec::new(),
                max_task_log_bytes: 512,
            },
        };
        let adapter = CursorAdapter {
            binary: fake.to_string_lossy().to_string(),
        };
        let res = adapter.run(task).await;
        assert!(res.is_err(), "non-zero agent exit surfaces as an error");
        let body = std::fs::read_to_string(&log_path).unwrap();
        assert!(
            body.contains("task log truncated at cap"),
            "marker written despite the non-zero exit; log was {} bytes",
            body.len()
        );
    }

    /// B3 (re-review): the stderr pump must DRAIN to the child's real EOF, not
    /// stop after `cap` bytes — otherwise a child that writes more stderr than
    /// the cap blocks (full pipe) or gets EPIPE and fails. Fake agent floods
    /// stderr with ~480 KB (>> the OS pipe buffer), THEN writes a valid stdout
    /// result and exits 0. The adapter must SUCCEED (proving the child wasn't
    /// blocked/SIGPIPE'd), the stdout result must parse, and the log must be
    /// bounded with the stderr marker.
    #[cfg(unix)]
    #[tokio::test]
    async fn stderr_drains_to_eof_so_child_completes_even_when_stderr_exceeds_cap() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("fake-cursor-noisy.sh");
        std::fs::write(
            &fake,
            "#!/bin/sh\ni=0\nwhile [ $i -lt 8000 ]; do echo \"noisy stderr line $i padding padding padding padding\" >&2; i=$((i+1)); done\nprintf '{\"type\":\"result\",\"result\":\"final answer\"}\\n'\nexit 0\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

        let log_path = dir.path().join("task.log");
        let task = AgentTask {
            task_id: "T_noisy".into(),
            workspace: dir.path().to_path_buf(),
            prompt: "hi".into(),
            context: Vec::new(),
            timeout: std::time::Duration::from_secs(30),
            mode: crate::adapter::ExecutionMode::Apply,
            resume_chat_id: None,
            log_path: log_path.clone(),
            trajectory: None,
            model: None,
            role_prelude: None,
            allowed_tools: Default::default(),
            harden: crate::runtime_harden::Hardening {
                scrub_env: false,
                extra_allow_env: Vec::new(),
                max_task_log_bytes: 2048,
            },
        };
        let adapter = CursorAdapter {
            binary: fake.to_string_lossy().to_string(),
        };
        let parsed = adapter
            .run(task)
            .await
            .expect("child completes (exit 0) — stderr was drained, not SIGPIPE'd / blocked");
        assert_eq!(
            parsed.transcript_summary, "final answer",
            "stdout result parsed"
        );
        let body = std::fs::read_to_string(&log_path).unwrap();
        assert!(
            body.contains("stderr truncated (cap exceeded)"),
            "stderr capped + marked"
        );
        assert!(
            body.len() < 32_000,
            "log bounded despite ~480 KB of stderr, got {}",
            body.len()
        );
    }
}
