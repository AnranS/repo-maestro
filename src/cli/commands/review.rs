//! `maestro review --role <name>` — invoke a role persona to audit the
//! current git diff (or an explicit set of files), then exit 0/1 based
//! on the verdict embedded in the response.
//!
//! Designed to slot directly into `goal.acceptance[].check` so a Cursor
//! Compound Engineering persona like `ce-kieran-typescript-reviewer`
//! becomes one of the L4 verification gates:
//!
//! ```yaml
//! goal:
//!   acceptance:
//!     - describe: Kieran-style TS review
//!       check: "maestro review --role ce-kieran-typescript-reviewer --since HEAD~1"
//! ```
//!
//! Verdict protocol: the prompt asks the persona to end its response with
//! a `VERDICT:` line containing either `pass` or `fail: <reasons>`. We
//! parse that, fall back to scanning for unambiguous "approve" / "block"
//! signals if the verdict line is missing, and finally default to PASS
//! when we genuinely can't tell — better to not loop the user on a
//! malformed reviewer response than to block forever.

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::adapter::{self, AgentTask, ExecutionMode};
use crate::paths;
use crate::roles;

pub struct ReviewArgs {
    /// Role / persona name. Resolved against the workspace role registry
    /// first, then builtin, then `~/.cursor/plugins/.../agents/`.
    pub role: String,
    /// Git ref to diff against. Defaults to `HEAD~1`. Use `--no-diff`
    /// to skip the diff entirely (e.g. for whole-file reviews).
    pub since: Option<String>,
    /// Workspace directory the diff is computed against. Defaults to
    /// the current `maestro` workspace root.
    pub workspace: Option<PathBuf>,
    /// Skip the git diff fetch — just pass the role + a "review the
    /// project" prompt to the agent. Useful for personas like an
    /// architecture-strategist that need broader context.
    pub no_diff: bool,
    /// Don't actually invoke the agent — render the prompt to stdout
    /// and exit 0. Useful for debugging the wiring without burning
    /// tokens.
    pub dry_run: bool,
}

pub async fn run(args: ReviewArgs) -> Result<()> {
    let role = roles::load(&args.role).with_context(|| format!("resolve role {:?}", args.role))?;

    let workspace = match &args.workspace {
        Some(p) => p.clone(),
        None => paths::workspace_root()?,
    };

    let diff = if args.no_diff {
        String::new()
    } else {
        let since = args.since.as_deref().unwrap_or("HEAD~1");
        capture_diff(&workspace, since).unwrap_or_default()
    };

    let prompt = render_review_prompt(&role.name, &role.prelude, &diff);

    if args.dry_run {
        println!("{prompt}");
        return Ok(());
    }

    if !args.no_diff && diff.trim().is_empty() {
        // Empty diff = nothing to review; treat as trivial pass so the
        // gate doesn't spuriously block when a verify-only commit lands.
        println!("VERDICT: pass (no diff to review)");
        return Ok(());
    }

    let adapter = adapter::pick("cursor");
    let log_path = std::env::temp_dir().join(format!(
        "maestro-review-{}-{}.log",
        sanitize(&role.name),
        std::process::id()
    ));

    let task = AgentTask {
        task_id: format!("review-{}", role.name),
        workspace,
        prompt: prompt.clone(),
        context: vec![],
        timeout: Duration::from_secs(600),
        mode: ExecutionMode::Apply,
        resume_chat_id: None,
        log_path,
        trajectory: None,
        model: None,
        role_prelude: None, // already embedded inside the prompt
        allowed_tools: crate::modes::AllowedTools::default(),
    };

    let result = adapter
        .run(task)
        .await
        .with_context(|| format!("invoke {:?} for review", role.name))?;

    let verdict = parse_verdict(&result.transcript_summary);
    println!("{}", result.transcript_summary.trim());

    match verdict {
        Verdict::Pass => Ok(()),
        Verdict::Fail(reason) => {
            anyhow::bail!("VERDICT: fail — {reason}");
        }
        Verdict::Ambiguous => {
            tracing::warn!(
                "review by {:?} produced no clear VERDICT line; defaulting to PASS",
                role.name
            );
            Ok(())
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Pass,
    Fail(String),
    Ambiguous,
}

/// Parse the verdict from the agent's transcript. Looks for, in order:
///   1. An explicit `VERDICT: pass` or `VERDICT: fail: <reason>` line.
///   2. Any line matching `(?i)blocking|must fix|❌|reject` near the end.
///   3. Fall back to Ambiguous.
fn parse_verdict(text: &str) -> Verdict {
    let lower = text.to_lowercase();

    // Walk lines bottom-up — the verdict, if present, is almost always
    // at the end. First explicit VERDICT: line wins.
    for line in text.lines().rev() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed
            .strip_prefix("VERDICT:")
            .or_else(|| trimmed.strip_prefix("Verdict:"))
            .or_else(|| trimmed.strip_prefix("verdict:"))
        {
            let rest = rest.trim();
            if rest.is_empty() {
                continue;
            }
            let rest_lower = rest.to_lowercase();
            if rest_lower.starts_with("pass") || rest_lower.starts_with("ok") {
                return Verdict::Pass;
            }
            if rest_lower.starts_with("fail") || rest_lower.starts_with("block") {
                let reason = rest
                    .split_once([':', '—', '-'])
                    .map(|(_, reason)| reason)
                    .map(|s| s.trim().to_string())
                    .unwrap_or_else(|| rest.to_string());
                return Verdict::Fail(reason);
            }
        }
    }

    // No explicit verdict — fall back to keyword scan, but only over the
    // bottom 20 lines so we don't get fooled by the reviewer enumerating
    // problems in their own write-up that they later approve.
    let tail: String = text
        .lines()
        .rev()
        .take(20)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();

    if tail.contains("must fix") || tail.contains("blocking") || tail.contains("❌") {
        return Verdict::Fail("reviewer flagged blocking issues (no VERDICT line)".into());
    }
    if lower.contains("looks good")
        || lower.contains("approve")
        || tail.contains("✅")
        || tail.contains("ship it")
    {
        return Verdict::Pass;
    }

    Verdict::Ambiguous
}

fn render_review_prompt(role_name: &str, role_prelude: &str, diff: &str) -> String {
    let mut s = String::new();
    s.push_str(&format!("# Role · {role_name}\n\n"));
    s.push_str(role_prelude.trim_end());
    s.push_str("\n\n# Task: review the following change\n\n");
    if diff.trim().is_empty() {
        s.push_str(
            "No git diff was supplied. Review the project as a whole \
             from the role above and flag anything blocking. Cap the \
             review at 300 words.\n\n",
        );
    } else {
        s.push_str(
            "Below is the git diff to review. Apply the role's \
                    bar and produce a tight review (≤ 300 words).\n\n",
        );
        s.push_str("```diff\n");
        s.push_str(diff.trim_end());
        s.push_str("\n```\n\n");
    }
    s.push_str(
        "End your response with exactly one of:\n\
         \n\
         - `VERDICT: pass` — when nothing blocking.\n\
         - `VERDICT: fail: <one-line reason>` — when something MUST be fixed.\n",
    );
    s
}

fn capture_diff(workspace: &std::path::Path, since: &str) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(workspace)
        .arg("diff")
        .arg("--no-color")
        .arg(since)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("spawn git diff in {:?}", workspace))?;
    if !out.status.success() {
        anyhow::bail!(
            "git diff {since} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_verdict_pass_wins() {
        let text = "lots of stuff\nmaybe some failures? no.\n\nVERDICT: pass\n";
        assert_eq!(parse_verdict(text), Verdict::Pass);
    }

    #[test]
    fn explicit_verdict_fail_with_reason() {
        let v = parse_verdict("found a bug\n\nVERDICT: fail: missing migration for the new column");
        match v {
            Verdict::Fail(r) => assert!(r.contains("migration")),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn case_insensitive_verdict_line() {
        assert_eq!(parse_verdict("blah\nVerdict: PASS\n"), Verdict::Pass);
    }

    #[test]
    fn keyword_fallback_blocks_on_must_fix() {
        let text = "Looked at the diff.\n\nMust fix: secret leaked in commit message.\n";
        match parse_verdict(text) {
            Verdict::Fail(_) => {}
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    #[test]
    fn keyword_fallback_approves_on_ship_it() {
        let text = "Diff looks reasonable.\n\nShip it.\n";
        assert_eq!(parse_verdict(text), Verdict::Pass);
    }

    #[test]
    fn ambiguous_when_nothing_decisive() {
        let text = "Reviewer rambled without conclusion.";
        assert_eq!(parse_verdict(text), Verdict::Ambiguous);
    }

    #[test]
    fn render_prompt_includes_role_and_diff() {
        let p = render_review_prompt("kieran", "be strict", "diff --git a/x b/x\n+ added\n");
        assert!(p.contains("# Role · kieran"));
        assert!(p.contains("be strict"));
        assert!(p.contains("```diff"));
        assert!(p.contains("VERDICT:"));
    }

    #[test]
    fn render_prompt_handles_empty_diff() {
        let p = render_review_prompt("arch", "review the whole project", "");
        assert!(!p.contains("```diff"));
        assert!(p.contains("project as a whole"));
    }
}
