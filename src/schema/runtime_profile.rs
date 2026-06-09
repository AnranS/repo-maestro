//! F-136a1 — RuntimeProfile: a read-only safety LABEL derived from a task's
//! `PermissionEvidence`. NO enforcement, NO behavior change — this projects the
//! existing permission evidence into a conservative risk tier the operator (and a
//! later runner) can read.
//!
//! Honesty rules (never silently downgrade to "safe"):
//! - Absent permission → `unknown` (or `requires_review` with an observed write) —
//!   never `review_only` (absent ≠ no-effects).
//! - Unknown provider (any `Enforcement::Unsupported`) / unrecognized schema →
//!   `requires_review` (fail-closed).
//! - `external_dir` / `mcp` are hardcoded `false` in v1 (`permissions.rs`), so the
//!   `high_risk_vm` / `tool_limited` tiers are FUTURE-READY: their rules are pinned
//!   here but unreachable from real data until those signals exist. Tests lock that
//!   today's `false` cannot fake-trigger them.
//! - Opaque providers' Soft dimensions are surfaced in `advisory` — NEVER claimed as
//!   per-tool hard enforcement (the real check stays the post-run F-126 policy gate).
//!
//! Cross-platform only. The Linux-only runner tiers (cgroup / rootless / gVisor /
//! Firecracker) are deferred to F-136c/e — a1 ships the label, nothing else.

use serde::{Deserialize, Serialize};

use crate::scheduler::state::RunState;
use crate::schema::permissions::{Enforcement, PermissionEvidence};

/// One mutually-exclusive safety label. `unknown` / `requires_review` are explicit
/// non-safe states — they must NEVER be rendered as "allow".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeProfile {
    ReviewOnly,
    WriteLocal,
    NetworkAllowlisted,
    ToolLimited,
    HighRiskVm,
    RequiresReview,
    Unknown,
}

impl RuntimeProfile {
    /// "Attention severity" for worst-case aggregation: `requires_review` dominates,
    /// then the privilege tiers, then `unknown` (we can't tell), then `review_only`
    /// (confirmed read-only). Higher = more attention needed.
    fn severity(self) -> u8 {
        match self {
            RuntimeProfile::RequiresReview => 6,
            RuntimeProfile::HighRiskVm => 5,
            RuntimeProfile::NetworkAllowlisted => 4,
            RuntimeProfile::WriteLocal => 3,
            RuntimeProfile::ToolLimited => 2,
            RuntimeProfile::Unknown => 1,
            RuntimeProfile::ReviewOnly => 0,
        }
    }
}

/// The per-task projection (pin 4: not a bare enum — explainable). Always present on
/// `TaskDetail`; the unknown/requires_review states live INSIDE `profile`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeProfileView {
    pub profile: RuntimeProfile,
    /// Human-readable why-this-label.
    pub reasons: Vec<String>,
    /// REQUESTED capabilities that are only `soft`-enforced — advisory, the provider
    /// does not hard-block them. NEVER presented as hard enforcement.
    pub advisory: Vec<String>,
    /// REQUESTED capabilities whose enforcement is `unsupported` (unknown provider).
    pub unsupported: Vec<String>,
}

/// Per-profile task count in a run (pin 5: keep the breakdown so one
/// `requires_review` doesn't erase the rest).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeProfileCount {
    pub profile: RuntimeProfile,
    pub count: u32,
}

/// The delivery-side summary: the worst-case label across the run's tasks PLUS the
/// full per-profile breakdown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeProfileSummary {
    pub worst: RuntimeProfile,
    pub counts: Vec<RuntimeProfileCount>,
    pub task_total: u32,
}

const DIMS: [&str; 6] = [
    "shell",
    "git_write",
    "network",
    "fs_write",
    "external_dir",
    "mcp",
];

/// Derive a task's RuntimeProfile from its permission evidence + the observed-write
/// signal (the F-126 definition, passed in so this stays pure). Conservative-up:
/// assume the most capability the signals permit; never a silent downgrade.
pub fn derive(permission: Option<&PermissionEvidence>, observed_write: bool) -> RuntimeProfileView {
    // 1/2. Absent permission — absent ≠ no-effects.
    let Some(pe) = permission else {
        return if observed_write {
            bare(
                RuntimeProfile::RequiresReview,
                "observed write effects but no permission evidence (audit gap)",
            )
        } else {
            bare(RuntimeProfile::Unknown, "no permission evidence recorded")
        };
    };

    // 3. Unrecognized schema → fail-closed (mirrors the F-126 gate).
    if pe.schema_version != crate::schema::PERMISSION_V1 {
        return bare(
            RuntimeProfile::RequiresReview,
            "unrecognized permission schema_version — untrusted, fail-closed",
        );
    }

    let req = [
        pe.requested.shell,
        pe.requested.git_write,
        pe.requested.network,
        pe.requested.fs_write,
        pe.requested.external_dir,
        pe.requested.mcp,
    ];
    let enf = [
        pe.resolved.shell,
        pe.resolved.git_write,
        pe.resolved.network,
        pe.resolved.fs_write,
        pe.resolved.external_dir,
        pe.resolved.mcp,
    ];
    // Advisory / unsupported channels, scoped to REQUESTED capabilities.
    let advisory: Vec<String> = (0..6)
        .filter(|&i| req[i] && enf[i] == Enforcement::Soft)
        .map(|i| DIMS[i].to_string())
        .collect();
    let unsupported: Vec<String> = (0..6)
        .filter(|&i| req[i] && enf[i] == Enforcement::Unsupported)
        .map(|i| DIMS[i].to_string())
        .collect();

    // 4. Unknown provider: ANY dimension resolves to `unsupported` (known providers
    // never produce `unsupported`) → enforcement undeterminable → requires_review.
    if enf.contains(&Enforcement::Unsupported) {
        return RuntimeProfileView {
            profile: RuntimeProfile::RequiresReview,
            reasons: vec![
                "unknown provider — enforcement unsupported, cannot classify safely".to_string(),
            ],
            advisory,
            unsupported,
        };
    }

    // 5–9. Tier ladder (conservative-up, first match wins).
    let r = &pe.requested;
    let (profile, reason) = if r.external_dir {
        (
            RuntimeProfile::HighRiskVm,
            "external_dir (host directory access) requested",
        )
    } else if r.network {
        (
            RuntimeProfile::NetworkAllowlisted,
            "network egress requested",
        )
    } else if r.git_write || r.fs_write || r.shell {
        (
            RuntimeProfile::WriteLocal,
            "local write capability (git_write / fs_write / shell requested)",
        )
    } else if r.mcp {
        (
            RuntimeProfile::ToolLimited,
            "narrow tools (mcp) only — no general shell / network / write",
        )
    } else {
        (
            RuntimeProfile::ReviewOnly,
            "no write / network / external-dir / tool capability requested",
        )
    };
    RuntimeProfileView {
        profile,
        reasons: vec![reason.to_string()],
        advisory,
        unsupported,
    }
}

fn bare(profile: RuntimeProfile, reason: &str) -> RuntimeProfileView {
    RuntimeProfileView {
        profile,
        reasons: vec![reason.to_string()],
        advisory: Vec::new(),
        unsupported: Vec::new(),
    }
}

/// Aggregate every task in a run into a worst-case label + a per-profile breakdown.
/// Pure (takes the loaded `RunState`). `None` when the run has no tasks.
pub fn summarize(state: &RunState) -> Option<RuntimeProfileSummary> {
    let mut counts: std::collections::BTreeMap<RuntimeProfile, u32> =
        std::collections::BTreeMap::new();
    let mut total = 0u32;
    for t in state.tasks.values() {
        let observed = crate::scheduler::policy_gate::observed_write(t);
        let p = derive(t.permission.as_ref(), observed).profile;
        *counts.entry(p).or_insert(0) += 1;
        total += 1;
    }
    if total == 0 {
        return None;
    }
    let worst = counts
        .keys()
        .copied()
        .max_by_key(|p| p.severity())
        .expect("non-empty");
    Some(RuntimeProfileSummary {
        worst,
        counts: counts
            .into_iter()
            .map(|(profile, count)| RuntimeProfileCount { profile, count })
            .collect(),
        task_total: total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::permissions::{PermissionRequest, ResolvedPermission};

    fn pe(provider: &str, req: PermissionRequest) -> PermissionEvidence {
        PermissionEvidence {
            schema_version: PermissionEvidence::SCHEMA_VERSION.to_string(),
            task_id: "T".into(),
            provider_id: provider.into(),
            mode_id: "m".into(),
            resolved: crate::schema::permissions::provider_permission_profile(provider),
            requested: req,
        }
    }

    fn req(shell: bool, git_write: bool, network: bool) -> PermissionRequest {
        PermissionRequest {
            shell,
            git_write,
            network,
            fs_write: git_write, // mirrors resolve_permission_evidence aliasing
            external_dir: false,
            mcp: false,
            allowed_commands: vec![],
        }
    }

    #[test]
    fn absent_no_effects_is_unknown_not_review_only() {
        let v = derive(None, false);
        assert_eq!(v.profile, RuntimeProfile::Unknown);
    }

    #[test]
    fn absent_with_observed_write_is_requires_review() {
        let v = derive(None, true);
        assert_eq!(v.profile, RuntimeProfile::RequiresReview);
    }

    #[test]
    fn no_caps_is_review_only() {
        let v = derive(Some(&pe("mock", req(false, false, false))), false);
        assert_eq!(v.profile, RuntimeProfile::ReviewOnly);
    }

    #[test]
    fn shell_only_no_declared_write_is_write_local_not_review_only() {
        // pin 2: shell can write even without a declared write → conservative.
        let v = derive(Some(&pe("mock", req(true, false, false))), false);
        assert_eq!(v.profile, RuntimeProfile::WriteLocal);
    }

    #[test]
    fn git_write_is_write_local() {
        let v = derive(Some(&pe("mock", req(false, true, false))), false);
        assert_eq!(v.profile, RuntimeProfile::WriteLocal);
    }

    #[test]
    fn network_is_network_allowlisted() {
        let v = derive(Some(&pe("mock", req(false, false, true))), false);
        assert_eq!(v.profile, RuntimeProfile::NetworkAllowlisted);
    }

    #[test]
    fn unknown_provider_is_requires_review() {
        let v = derive(Some(&pe("totally-unknown", req(true, true, true))), false);
        assert_eq!(v.profile, RuntimeProfile::RequiresReview);
        assert!(!v.unsupported.is_empty());
    }

    #[test]
    fn schema_mismatch_is_requires_review() {
        let mut p = pe("mock", req(false, false, false));
        p.schema_version = "maestro.permission.v0-bogus".into();
        assert_eq!(
            derive(Some(&p), false).profile,
            RuntimeProfile::RequiresReview
        );
    }

    #[test]
    fn soft_dims_go_to_advisory_never_claimed_hard() {
        // shell provider: git_write/network/fs_write are Soft (advisory). Extra test
        // matrix #1 — soft must NOT be presented as hard.
        let v = derive(Some(&pe("shell", req(true, true, true))), false);
        assert!(v.advisory.contains(&"git_write".to_string()));
        assert!(v.advisory.contains(&"network".to_string()));
        // shell itself is Hard on the shell provider → not advisory.
        assert!(!v.advisory.contains(&"shell".to_string()));
    }

    #[test]
    fn external_dir_and_mcp_false_today_cannot_fake_trigger_future_tiers() {
        // pin 3 + future-ready lock: with external_dir/mcp false, NO real input can
        // reach high_risk_vm / tool_limited.
        for shell in [false, true] {
            for gw in [false, true] {
                for net in [false, true] {
                    let v = derive(Some(&pe("mock", req(shell, gw, net))), false);
                    assert_ne!(v.profile, RuntimeProfile::HighRiskVm);
                    assert_ne!(v.profile, RuntimeProfile::ToolLimited);
                }
            }
        }
    }

    #[test]
    fn external_dir_true_reaches_high_risk_vm_future_ready() {
        // The rule is pinned even though v1 data can't set external_dir=true.
        let mut r = req(true, true, true);
        r.external_dir = true;
        assert_eq!(
            derive(Some(&pe("mock", r)), false).profile,
            RuntimeProfile::HighRiskVm
        );
    }

    #[test]
    fn mcp_only_reaches_tool_limited_future_ready() {
        let mut r = req(false, false, false);
        r.mcp = true;
        assert_eq!(
            derive(Some(&pe("mock", r)), false).profile,
            RuntimeProfile::ToolLimited
        );
    }

    #[test]
    fn severity_ordering_requires_review_dominates_and_unknown_above_review_only() {
        // worst-case aggregation: requires_review is the most-attention label; the
        // privilege tiers rank above unknown, which ranks above confirmed read-only.
        assert!(RuntimeProfile::RequiresReview.severity() > RuntimeProfile::HighRiskVm.severity());
        assert!(
            RuntimeProfile::NetworkAllowlisted.severity() > RuntimeProfile::WriteLocal.severity()
        );
        assert!(RuntimeProfile::WriteLocal.severity() > RuntimeProfile::Unknown.severity());
        assert!(RuntimeProfile::Unknown.severity() > RuntimeProfile::ReviewOnly.severity());
    }

    fn resolved_with_unsupported() -> ResolvedPermission {
        ResolvedPermission::all(Enforcement::Unsupported)
    }

    #[test]
    fn requested_unsupported_listed_in_unsupported_channel() {
        let p = PermissionEvidence {
            schema_version: PermissionEvidence::SCHEMA_VERSION.to_string(),
            task_id: "T".into(),
            provider_id: "x".into(),
            mode_id: "m".into(),
            requested: req(true, false, false),
            resolved: resolved_with_unsupported(),
        };
        let v = derive(Some(&p), false);
        assert_eq!(v.profile, RuntimeProfile::RequiresReview);
        assert!(v.unsupported.contains(&"shell".to_string()));
    }
}
