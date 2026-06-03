//! Tool-use protocol for chat: assistant emits fenced `maestro-action` blocks that
//! the UI can offer the user as "Run" buttons. When approved, the backend
//! invokes the corresponding `maestro` subcommand and streams the output back.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActionVerb {
    Work,
    Run,
    Approve,
    Status,
    Rerun,
    PlanValidate,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActionStatus {
    Pending,
    Approved,
    Rejected,
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Action {
    pub id: String,
    pub verb: ActionVerb,

    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub args: BTreeMap<String, String>,

    #[serde(default)]
    pub status: Option<ActionStatus>,

    /// Human-readable summary the UI can show on the confirm button.
    #[serde(default)]
    pub label: String,

    /// Captured stdout/stderr after execution.
    #[serde(default)]
    pub output: Option<String>,
}

impl Action {
    pub fn label_for(verb: ActionVerb, args: &BTreeMap<String, String>) -> String {
        match verb {
            ActionVerb::Work => format!(
                "maestro work {}",
                args.get("spec").cloned().unwrap_or_else(|| "<goal>".into())
            ),
            ActionVerb::Run => format!(
                "maestro run {}",
                args.get("plan").cloned().unwrap_or_else(|| "<plan>".into())
            ),
            ActionVerb::Approve => format!(
                "maestro approve {}",
                args.get("task").cloned().unwrap_or_else(|| "<task>".into())
            ),
            ActionVerb::Status => "maestro status".into(),
            ActionVerb::Rerun => {
                let plan = args.get("plan").cloned().unwrap_or_else(|| "<plan>".into());
                let from = args
                    .get("from")
                    .map(|f| format!(" --from {f}"))
                    .unwrap_or_default();
                format!("maestro rerun {plan}{from}")
            }
            ActionVerb::PlanValidate => format!(
                "maestro plan validate {}",
                args.get("plan").cloned().unwrap_or_else(|| "<plan>".into())
            ),
        }
    }
}

/// Parses ```maestro-action ... ``` fenced YAML blocks out of assistant text and
/// returns them as Action records. Unknown verbs are silently skipped so the
/// model can't trick us into running arbitrary commands.
pub fn parse_actions(text: &str) -> Vec<Action> {
    let mut out = vec![];
    let bytes = text.as_bytes();
    let needle = b"```maestro-action";

    let mut i = 0usize;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] == needle {
            // find the newline after the open fence
            let mut start = i + needle.len();
            while start < bytes.len() && bytes[start] != b'\n' {
                start += 1;
            }
            if start >= bytes.len() {
                break;
            }
            start += 1; // skip newline

            // find the closing ```
            let close_needle = b"```";
            let mut end = start;
            while end + close_needle.len() <= bytes.len() {
                if &bytes[end..end + close_needle.len()] == close_needle {
                    break;
                }
                end += 1;
            }
            if end + close_needle.len() > bytes.len() {
                break;
            }

            let body = std::str::from_utf8(&bytes[start..end]).unwrap_or("");
            if let Some(action) = parse_one(body) {
                out.push(action);
            }
            i = end + close_needle.len();
        } else {
            i += 1;
        }
    }
    out
}

fn parse_one(body: &str) -> Option<Action> {
    let raw: serde_yaml::Value = serde_yaml::from_str(body).ok()?;
    let mapping = raw.as_mapping()?;

    let verb_str = mapping
        .get(serde_yaml::Value::String("verb".into()))
        .and_then(|v| v.as_str())?
        .to_ascii_lowercase();

    let verb = match verb_str.as_str() {
        "work" => ActionVerb::Work,
        "run" => ActionVerb::Run,
        "approve" => ActionVerb::Approve,
        "status" => ActionVerb::Status,
        "rerun" => ActionVerb::Rerun,
        "plan_validate" | "plan-validate" | "validate" => ActionVerb::PlanValidate,
        _ => return None,
    };

    let mut args = BTreeMap::new();
    for (k, v) in mapping {
        let key = k.as_str()?.to_string();
        if key == "verb" {
            continue;
        }
        if let Some(s) = v.as_str() {
            args.insert(key, s.to_string());
        } else if let Some(b) = v.as_bool() {
            args.insert(key, b.to_string());
        } else if let Some(n) = v.as_i64() {
            args.insert(key, n.to_string());
        }
    }

    Some(Action {
        id: Uuid::new_v4().to_string(),
        label: Action::label_for(verb, &args),
        verb,
        args,
        status: Some(ActionStatus::Pending),
        output: None,
    })
}

/// Runs the action's underlying maestro subcommand and returns captured output.
pub async fn execute_action(action: &Action) -> Result<String> {
    execute_action_with_session(action, None).await
}

pub async fn execute_action_with_session(
    action: &Action,
    session_id: Option<&str>,
) -> Result<String> {
    let argv = build_argv(action)?;
    let exe = std::env::current_exe().context("locate maestro executable")?;
    let mut cmd = Command::new(&exe);
    cmd.args(&argv)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    if matches!(
        action.verb,
        ActionVerb::Run | ActionVerb::Rerun | ActionVerb::Work
    ) {
        if let Some(sid) = session_id {
            cmd.env("MAESTRO_SESSION_ID", sid);
        }
    }

    let mut child = cmd.spawn().context("spawn maestro subcommand")?;

    let mut out = String::new();
    if let Some(stdout) = child.stdout.take() {
        let mut reader = BufReader::new(stdout).lines();
        while let Some(line) = reader.next_line().await? {
            out.push_str(&line);
            out.push('\n');
        }
    }

    let status = child.wait().await?;

    let mut stderr_buf = String::new();
    if let Some(stderr) = child.stderr.take() {
        let mut reader = BufReader::new(stderr).lines();
        while let Some(line) = reader.next_line().await? {
            stderr_buf.push_str(&line);
            stderr_buf.push('\n');
        }
    }
    if !stderr_buf.is_empty() {
        out.push_str("\n[stderr]\n");
        out.push_str(&stderr_buf);
    }

    if !status.success() {
        anyhow::bail!("maestro {:?} exited {:?}\n{}", argv, status.code(), out);
    }
    Ok(out)
}

/// Translate an action into the argv we'd pass to `maestro`. Kept public so
/// integration tests can pin the verb-to-CLI mapping without spawning a
/// subprocess.
pub fn build_argv(action: &Action) -> Result<Vec<String>> {
    Ok(match action.verb {
        ActionVerb::Work => {
            let spec = action
                .args
                .get("spec")
                .cloned()
                .context("action `work` missing `spec`")?;
            let mut v = vec!["work".into(), spec];
            if let Some(root) = action.args.get("root") {
                v.push("--root".into());
                v.push(root.clone());
            }
            if let Some(agent) = action.args.get("agent") {
                v.push("--agent".into());
                v.push(agent.clone());
            }
            if let Some(out) = action.args.get("out") {
                v.push("--out".into());
                v.push(out.clone());
            }
            if let Some(project) = action.args.get("project") {
                v.push("--project".into());
                v.push(project.clone());
            }
            if action.args.get("run").map(|s| s == "true").unwrap_or(false) {
                v.push("--run".into());
            }
            if action.args.get("dry").map(|s| s == "true").unwrap_or(false) {
                v.push("--dry".into());
            }
            v
        }
        ActionVerb::Run => {
            let plan = action
                .args
                .get("plan")
                .cloned()
                .context("action `run` missing `plan`")?;
            verify_plan_hash_arg(&plan, &action.args)?;
            let mut v = vec!["run".into(), plan];
            if let Some(only) = action.args.get("only") {
                v.push("--only".into());
                v.push(only.clone());
            }
            if let Some(skip) = action.args.get("skip") {
                v.push("--skip".into());
                v.push(skip.clone());
            }
            v
        }
        ActionVerb::Approve => {
            let task = action
                .args
                .get("task")
                .cloned()
                .context("action `approve` missing `task`")?;
            vec!["approve".into(), task]
        }
        ActionVerb::Status => vec!["status".into()],
        ActionVerb::Rerun => {
            let plan = action
                .args
                .get("plan")
                .cloned()
                .context("action `rerun` missing `plan`")?;
            verify_plan_hash_arg(&plan, &action.args)?;
            let mut v = vec!["rerun".into(), plan];
            if let Some(from) = action.args.get("from") {
                v.push("--from".into());
                v.push(from.clone());
            }
            v
        }
        ActionVerb::PlanValidate => {
            let plan = action
                .args
                .get("plan")
                .cloned()
                .context("action `plan_validate` missing `plan`")?;
            verify_plan_hash_arg(&plan, &action.args)?;
            vec!["plan".into(), "validate".into(), plan]
        }
    })
}

fn verify_plan_hash_arg(plan: &str, args: &BTreeMap<String, String>) -> Result<()> {
    let Some(expected) = args.get("plan_hash").filter(|h| !h.trim().is_empty()) else {
        return Ok(());
    };
    let actual = crate::file_guard::file_hash(Path::new(plan))?;
    if crate::file_guard::hash_matches(expected, &actual) {
        return Ok(());
    }
    anyhow::bail!(
        "action plan_hash mismatch for {plan}: expected {expected}, actual {actual}. Re-run `maestro plan hash {plan}` and update the action block."
    )
}
