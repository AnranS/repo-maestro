//! F-126 — node tool-policy violation evaluator.
//!
//! A PURE function over a finished task's own state (its resolved
//! `PermissionEvidence` + observed `artifacts`). No clock, no IO, no agent-tool
//! interception — maestro cannot intercept an opaque provider's individual tool
//! calls, so this is a post-run audit verdict only. The scheduler turns a `Gate`
//! verdict into the existing approval gate (before integration), behind the
//! opt-in `defaults.gate_on_policy_violation` flag.
//!
//! Decision rules (v1):
//! - **present** policy + observed effect OUTSIDE the requested capability → `Gate`:
//!   - changed files need a write capability (`git_write || fs_write`);
//!   - a PR (`pr_url`) needs `git_write`.
//!   - `branch` is NOT a signal — the executor auto-fills it with the per-task
//!     worktree isolation branch, so it can't distinguish a real push (F-126-fu).
//!   - (network / mcp / external_dir have no reliable observed signal in v1.)
//! - **absent** policy (legacy run) + any observed effect → `FlagOnly` (record,
//!   never block a legacy run).
//! - **corrupt / untrusted** policy (permission `schema_version` mismatch) → `Gate`
//!   (fail-closed — never auto-allow).
//! - no observed effects → `Allow`.

use crate::scheduler::state::TaskState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyDecision {
    /// Within policy (or nothing observed) — integrate normally.
    Allow,
    /// Worth a finding, but not a gate (e.g. a legacy run with no policy).
    FlagOnly,
    /// Pause for approval before integration.
    Gate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyVerdict {
    pub decision: PolicyDecision,
    pub reasons: Vec<String>,
}

impl PolicyVerdict {
    fn allow() -> Self {
        Self {
            decision: PolicyDecision::Allow,
            reasons: Vec::new(),
        }
    }
    fn of(decision: PolicyDecision, reasons: Vec<String>) -> Self {
        Self { decision, reasons }
    }
}

/// Whether a finished task shows a real WRITE side effect. ONLY `files_changed`
/// (net diff) and `pr_url` (adapter-produced) count — `branch` is the executor's
/// auto per-task worktree isolation branch (set only when empty), so it can't
/// distinguish a real push (F-126-fu). Shared so the policy gate and the
/// RuntimeProfile derivation (F-136a1) never drift on what "observed write" means.
pub(crate) fn observed_write(t: &TaskState) -> bool {
    let changed_files = !t.artifacts.files_changed.is_empty();
    let pr = t
        .artifacts
        .pr_url
        .as_deref()
        .is_some_and(|s| !s.trim().is_empty());
    changed_files || pr
}

/// Evaluate a finished task's tool policy against its observed effects. Pure.
pub fn evaluate(t: &TaskState) -> PolicyVerdict {
    let changed_files = !t.artifacts.files_changed.is_empty();
    // `artifacts.branch` is NOT a signal (see `observed_write`): only `files_changed`
    // (net diff) and `pr_url` (adapter-produced, never auto-filled) are reliable.
    let pr = t
        .artifacts
        .pr_url
        .as_deref()
        .is_some_and(|s| !s.trim().is_empty());
    let observed_any = observed_write(t);

    let Some(pe) = t.permission.as_ref() else {
        // Absent policy (legacy run): flag observed effects, never gate.
        return if observed_any {
            PolicyVerdict::of(
                PolicyDecision::FlagOnly,
                vec![
                    "observed side effects but no permission evidence (legacy run) — flagged, not gated"
                        .to_string(),
                ],
            )
        } else {
            PolicyVerdict::allow()
        };
    };

    // Corrupt / untrusted policy → fail-closed.
    if pe.schema_version != crate::schema::PERMISSION_V1 {
        return PolicyVerdict::of(
            PolicyDecision::Gate,
            vec![format!(
                "permission evidence schema_version {:?} != {} — untrusted policy, fail-closed",
                pe.schema_version,
                crate::schema::PERMISSION_V1
            )],
        );
    }

    let req = &pe.requested;
    let write_requested = req.git_write || req.fs_write;
    let mut reasons = Vec::new();
    if changed_files && !write_requested {
        reasons.push(
            "changed files but neither git_write nor fs_write was requested in this task's policy"
                .to_string(),
        );
    }
    if pr && !req.git_write {
        reasons.push("opened a PR but git_write was not requested".to_string());
    }

    if reasons.is_empty() {
        PolicyVerdict::allow()
    } else {
        PolicyVerdict::of(PolicyDecision::Gate, reasons)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::Artifacts;
    use crate::scheduler::state::TaskStatus;
    use crate::schema::permissions::{
        Enforcement, PermissionEvidence, PermissionRequest, ResolvedPermission,
    };
    use std::collections::BTreeMap;

    fn task() -> TaskState {
        TaskState {
            id: "T0".into(),
            project: "p".into(),
            agent: "mock".into(),
            status: TaskStatus::Done,
            started_at: None,
            ended_at: None,
            chat_id: None,
            error: None,
            attempts: 0,
            risk_level: None,
            artifacts: Artifacts::default(),
            permission: None,
            workflow_outputs: BTreeMap::new(),
            log_path: "T0.log".into(),
            trajectory_path: None,
            depends_on: vec![],
            parallel_group: None,
            requires_approval_after: false,
            kind: "agent".into(),
            memory_used: vec![],
            context_bytes: None,
            skills_triggered: vec![],
            usage: None,
            steps: None,
            role: None,
            resolved_agent_profile: None,
            resolved_review_profile: None,
            workspace_path: None,
            worktree_path: None,
        }
    }

    fn permission(git_write: bool, fs_write: bool) -> PermissionEvidence {
        PermissionEvidence {
            schema_version: PermissionEvidence::SCHEMA_VERSION.to_string(),
            task_id: "T0".into(),
            provider_id: "mock".into(),
            mode_id: "m".into(),
            requested: PermissionRequest {
                shell: true,
                git_write,
                network: false,
                fs_write,
                external_dir: false,
                mcp: false,
                allowed_commands: vec![],
            },
            resolved: ResolvedPermission::all(Enforcement::Soft),
        }
    }

    #[test]
    fn no_effects_allows() {
        assert_eq!(evaluate(&task()).decision, PolicyDecision::Allow);
    }

    #[test]
    fn present_compliant_allows() {
        let mut t = task();
        t.artifacts.files_changed = vec!["src/a.rs".into()];
        t.permission = Some(permission(true, false)); // git_write requested
        assert_eq!(evaluate(&t).decision, PolicyDecision::Allow);
    }

    #[test]
    fn present_changed_files_without_write_gates() {
        let mut t = task();
        t.artifacts.files_changed = vec!["src/a.rs".into()];
        t.permission = Some(permission(false, false)); // no write requested
        let v = evaluate(&t);
        assert_eq!(v.decision, PolicyDecision::Gate);
        assert!(v.reasons.iter().any(|r| r.contains("git_write")));
    }

    #[test]
    fn branch_alone_is_not_a_signal_and_does_not_gate() {
        // F-126-fu: a worktree branch (no diff, no PR) is NOT a write signal.
        let mut t = task();
        t.artifacts.branch = Some("maestro/worktree/T0".into());
        t.permission = Some(permission(false, false)); // no write requested
        assert_eq!(evaluate(&t).decision, PolicyDecision::Allow);
    }

    #[test]
    fn present_pr_url_without_git_write_gates() {
        let mut t = task();
        t.artifacts.pr_url = Some("https://example.invalid/pr/1".into());
        t.permission = Some(permission(false, true)); // fs_write but not git_write
        let v = evaluate(&t);
        assert_eq!(v.decision, PolicyDecision::Gate);
        assert!(v.reasons.iter().any(|r| r.contains("PR")));
    }

    #[test]
    fn absent_policy_with_effects_flags_only() {
        let mut t = task();
        t.artifacts.files_changed = vec!["src/a.rs".into()];
        // permission stays None (legacy)
        assert_eq!(evaluate(&t).decision, PolicyDecision::FlagOnly);
    }

    #[test]
    fn absent_policy_no_effects_allows() {
        assert_eq!(evaluate(&task()).decision, PolicyDecision::Allow);
    }

    #[test]
    fn corrupt_schema_version_fails_closed() {
        let mut t = task();
        t.artifacts.files_changed = vec!["src/a.rs".into()];
        let mut pe = permission(true, true); // would otherwise be compliant
        pe.schema_version = "maestro.permission.v0-bogus".into();
        t.permission = Some(pe);
        let v = evaluate(&t);
        assert_eq!(v.decision, PolicyDecision::Gate);
        assert!(v.reasons.iter().any(|r| r.contains("fail-closed")));
    }
}
