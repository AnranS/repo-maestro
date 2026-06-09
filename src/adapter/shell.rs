use super::{AgentAdapter, AgentResult, AgentTask, Artifacts, Capability};
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::scheduler::trajectory::TrajectoryEventDraft;
use crate::schema::trajectory::{Redaction, TrajectoryEventKind, TrajectoryStatus};

/// Runs a shell command in the task's workspace. Used by `verify` tasks.
pub struct ShellAdapter;

impl ShellAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ShellAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AgentAdapter for ShellAdapter {
    fn name(&self) -> &str {
        "shell"
    }

    async fn run(&self, task: AgentTask) -> Result<AgentResult> {
        let cmd_str = task.prompt.clone();
        check_command(&cmd_str, &task.allowed_tools)?;
        if cmd_str.trim().is_empty() {
            anyhow::bail!("shell adapter: empty command");
        }
        let (trajectory_command, command_redaction) =
            crate::schema::redaction::redact_secret_text(&cmd_str);
        let mut trajectory = super::trajectory_writer_for(&task);
        let mut log_refs = BTreeMap::new();
        log_refs.insert("log".to_string(), super::log_ref(&task));
        if let Some(writer) = trajectory.as_mut() {
            super::append_trajectory_event(
                writer,
                &task.task_id,
                TrajectoryEventDraft {
                    kind: TrajectoryEventKind::Command,
                    tool_name: Some("shell".to_string()),
                    command: Some(trajectory_command.clone()),
                    status: Some(TrajectoryStatus::Started),
                    refs: log_refs.clone(),
                    usage: None,
                    redaction: command_redaction,
                },
            );
        }

        let mut log = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&task.log_path)
            .await
            .with_context(|| format!("open log {:?}", task.log_path))?;

        log.write_all(
            format!(
                "[shell] cd {:?}\n[shell] {}\n",
                task.workspace, trajectory_command
            )
            .as_bytes(),
        )
        .await
        .ok();

        // When an allowlist is active the command has already been validated as
        // a single argv with no shell control operators, so execute it directly
        // — never via `sh -c`. This closes the bypass where `sh` would still
        // expand `$VAR` / `${IFS}` / `~` at runtime (interpolating inherited-env
        // secrets or injecting word breaks) even though the shlex-parsed argv
        // the allowlist checked looked benign. With no allowlist set the
        // operator opted into full shell semantics, so we keep `sh -c` to
        // preserve pipes/redirects in verify commands.
        let direct_argv = if task.allowed_tools.allowed_commands.is_empty() {
            None
        } else {
            let argv = parse_simple_command(&cmd_str)?;
            (!argv.is_empty()).then_some(argv)
        };

        let mut command = match &direct_argv {
            Some(argv) => {
                let mut c = Command::new(&argv[0]);
                c.args(&argv[1..]);
                c
            }
            None => {
                let mut c = Command::new("sh");
                c.arg("-c").arg(&cmd_str);
                c
            }
        };
        command
            .current_dir(&task.workspace)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Own process group so a timeout tears down the whole subtree (the
        // command plus anything it spawned), not just the direct child.
        crate::proc::isolate_process_group(&mut command);

        let child = command
            .spawn()
            .with_context(|| format!("shell spawn for task {}", task.task_id))?;
        let child_pid = child.id();

        let output = match tokio::time::timeout(task.timeout, child.wait_with_output()).await {
            Ok(result) => {
                result.with_context(|| format!("shell wait for task {}", task.task_id))?
            }
            Err(_) => {
                if let Some(pid) = child_pid {
                    crate::proc::kill_process_group(pid);
                }
                if let Some(writer) = trajectory.as_mut() {
                    super::append_trajectory_event(
                        writer,
                        &task.task_id,
                        TrajectoryEventDraft {
                            kind: TrajectoryEventKind::Error,
                            tool_name: Some("shell".to_string()),
                            command: Some(trajectory_command),
                            status: Some(TrajectoryStatus::Interrupted),
                            refs: log_refs,
                            usage: None,
                            redaction: command_redaction,
                        },
                    );
                }
                anyhow::bail!("shell timeout for task {}", task.task_id);
            }
        };

        log.write_all(
            crate::schema::redaction::redact_secret_blob(&String::from_utf8_lossy(&output.stdout))
                .as_bytes(),
        )
        .await
        .ok();
        if !output.stderr.is_empty() {
            log.write_all(b"\n[stderr]\n").await.ok();
            log.write_all(
                crate::schema::redaction::redact_secret_blob(&String::from_utf8_lossy(
                    &output.stderr,
                ))
                .as_bytes(),
            )
            .await
            .ok();
        }
        if let Some(writer) = trajectory.as_mut() {
            super::append_trajectory_event(
                writer,
                &task.task_id,
                TrajectoryEventDraft {
                    kind: TrajectoryEventKind::Output,
                    tool_name: Some("shell".to_string()),
                    command: None,
                    status: Some(TrajectoryStatus::Completed),
                    refs: log_refs.clone(),
                    usage: None,
                    redaction: Redaction::None,
                },
            );
        }

        if !output.status.success() {
            if let Some(writer) = trajectory.as_mut() {
                super::append_trajectory_event(
                    writer,
                    &task.task_id,
                    TrajectoryEventDraft {
                        kind: TrajectoryEventKind::Error,
                        tool_name: Some("shell".to_string()),
                        command: Some(trajectory_command),
                        status: Some(TrajectoryStatus::Failed),
                        refs: log_refs,
                        usage: None,
                        redaction: command_redaction,
                    },
                );
            }
            anyhow::bail!("shell exited with status {:?}", output.status.code());
        }
        if let Some(writer) = trajectory.as_mut() {
            super::append_trajectory_event(
                writer,
                &task.task_id,
                TrajectoryEventDraft {
                    kind: TrajectoryEventKind::Final,
                    tool_name: Some("shell".to_string()),
                    command: Some(trajectory_command),
                    status: Some(TrajectoryStatus::Completed),
                    refs: log_refs,
                    usage: None,
                    redaction: command_redaction,
                },
            );
        }

        Ok(AgentResult {
            chat_id: None,
            artifacts: Artifacts::default(),
            transcript_summary: format!("shell ok ({}b stdout)", output.stdout.len()),
            usage: None,
            steps: None,
        })
    }

    fn supports(&self, cap: Capability) -> bool {
        matches!(cap, Capability::StreamOutput)
    }
}

pub fn check_command(cmd: &str, allow: &crate::modes::AllowedTools) -> Result<()> {
    if !allow.shell {
        anyhow::bail!("shell disabled for this mode");
    }
    if allow.allowed_commands.is_empty() {
        return Ok(());
    }
    let argv = parse_simple_command(cmd)?;
    let trimmed = cmd.trim();
    let normalized = argv.join(" ");
    let ok = allow
        .allowed_commands
        .iter()
        .any(|pattern| glob_match(pattern, &normalized));
    if !ok {
        anyhow::bail!("command not in allowlist: {trimmed}");
    }
    Ok(())
}

fn parse_simple_command(cmd: &str) -> Result<Vec<String>> {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    reject_shell_control_syntax(trimmed)?;
    let argv = shlex::split(trimmed).context("command has unsupported shell quoting")?;
    if argv.iter().any(|arg| is_shell_control_token(arg)) {
        anyhow::bail!("shell control operators are not allowed in allowed_commands");
    }
    Ok(argv)
}

fn reject_shell_control_syntax(cmd: &str) -> Result<()> {
    let mut quote = Quote::None;
    let mut chars = cmd.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Quote::Single => {
                if c == '\'' {
                    quote = Quote::None;
                }
            }
            Quote::Double => {
                if c == '"' {
                    quote = Quote::None;
                } else if c == '`' || (c == '$' && chars.peek() == Some(&'(')) {
                    anyhow::bail!("shell command substitution is not allowed in allowed_commands");
                }
            }
            Quote::None => match c {
                '\'' => quote = Quote::Single,
                '"' => quote = Quote::Double,
                '`' => {
                    anyhow::bail!("shell command substitution is not allowed in allowed_commands");
                }
                '$' if chars.peek() == Some(&'(') => {
                    anyhow::bail!("shell command substitution is not allowed in allowed_commands");
                }
                ';' | '|' | '&' | '<' | '>' | '\n' | '\r' => {
                    anyhow::bail!("shell control operators are not allowed in allowed_commands");
                }
                _ => {}
            },
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Quote {
    None,
    Single,
    Double,
}

fn is_shell_control_token(arg: &str) -> bool {
    matches!(arg, "&&" | "||" | "|" | ";" | "&" | "<" | ">" | "<<" | ">>")
}

fn glob_match(pattern: &str, value: &str) -> bool {
    let pattern = pattern.trim();
    if pattern == "*" {
        return true;
    }
    let parts = pattern.split('*').collect::<Vec<_>>();
    let mut rest = value;
    if let Some(first) = parts.first().filter(|p| !p.is_empty()) {
        if !rest.starts_with(first) {
            return false;
        }
        rest = &rest[first.len()..];
    }
    for part in parts
        .iter()
        .skip(1)
        .take(parts.len().saturating_sub(2))
        .filter(|p| !p.is_empty())
    {
        let Some(idx) = rest.find(part) else {
            return false;
        };
        rest = &rest[idx + part.len()..];
    }
    if let Some(last) = parts.last().filter(|p| !p.is_empty()) {
        return rest.ends_with(last);
    }
    true
}

pub async fn run_with_mode(cmd: &str, mode: &crate::modes::Mode) -> Result<()> {
    check_command(cmd, &mode.allowed_tools)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{AgentAdapter, AgentTask, ExecutionMode, TrajectoryContext};
    use crate::modes::AllowedTools;
    use std::time::Duration;

    fn allow(pattern: &str) -> AllowedTools {
        AllowedTools {
            shell: true,
            allowed_commands: vec![pattern.into()],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn shell_timeout_records_interrupted_trajectory() {
        let temp = tempfile::tempdir().unwrap();
        let trajectory_path = temp.path().join("T_timeout.ndjson");
        let task = AgentTask {
            task_id: "T_timeout".to_string(),
            workspace: temp.path().to_path_buf(),
            prompt: "sleep 1".to_string(),
            context: Vec::new(),
            timeout: Duration::from_millis(20),
            mode: ExecutionMode::Apply,
            resume_chat_id: None,
            log_path: temp.path().join("shell.log"),
            trajectory: Some(TrajectoryContext {
                run_id: "run-1".to_string(),
                task_id: "T_timeout".to_string(),
                provider_id: "shell".to_string(),
                path: trajectory_path.clone(),
            }),
            model: None,
            role_prelude: None,
            allowed_tools: AllowedTools {
                shell: true,
                ..Default::default()
            },
            harden: Default::default(),
        };

        let err = ShellAdapter::new().run(task).await.unwrap_err();
        assert!(err.to_string().contains("shell timeout"));
        let events = crate::scheduler::trajectory::read_trajectory(&trajectory_path).unwrap();
        assert!(events.iter().any(|event| {
            event.kind == TrajectoryEventKind::Error
                && event.status == Some(TrajectoryStatus::Interrupted)
        }));
    }

    #[test]
    fn allowlist_accepts_simple_argv_command() {
        check_command("npm test -- --runInBand", &allow("npm test*")).unwrap();
        check_command("cargo test config::plan", &allow("cargo test*")).unwrap();
    }

    #[tokio::test]
    async fn allowlisted_command_runs_argv_directly_without_shell_expansion() {
        // Active allowlist => exec argv directly (no `sh -c`), so `$VAR` is
        // passed literally and never interpolated. This is the fix for the
        // env-secret exfiltration bypass; `sh -c` would have expanded it.
        std::env::set_var("MAESTRO_NOEXPAND_SENTINEL", "EXPANDED_VALUE_LEAK");
        let temp = tempfile::tempdir().unwrap();
        let task = AgentTask {
            task_id: "T_noexpand".to_string(),
            workspace: temp.path().to_path_buf(),
            prompt: "echo $MAESTRO_NOEXPAND_SENTINEL".to_string(),
            context: Vec::new(),
            timeout: Duration::from_secs(10),
            mode: ExecutionMode::Apply,
            resume_chat_id: None,
            log_path: temp.path().join("shell.log"),
            trajectory: None,
            model: None,
            role_prelude: None,
            allowed_tools: allow("echo*"),
            harden: Default::default(),
        };
        ShellAdapter::new().run(task).await.unwrap();
        let log = std::fs::read_to_string(temp.path().join("shell.log")).unwrap();
        assert!(
            !log.contains("EXPANDED_VALUE_LEAK"),
            "env var was expanded by a shell; argv-direct exec expected. log: {log}"
        );
    }

    #[test]
    fn allowlist_rejects_shell_composition_even_with_matching_prefix() {
        for cmd in [
            "npm test && rm -rf /tmp/x",
            "npm test; rm -rf /tmp/x",
            "npm test | cat",
            "npm test > /tmp/out",
            "npm test $(rm -rf /tmp/x)",
            "npm test `rm -rf /tmp/x`",
        ] {
            let err = check_command(cmd, &allow("npm test*")).unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains("not allowed"),
                "{cmd:?} should fail with a hard policy error, got {msg:?}"
            );
        }
    }
}
