//! Declarative reaction rules for closing the PR loop.
//!
//! The 2026 "agent orchestrator" pattern (ComposioHQ et al.): instead of a
//! human shepherding every PR, map a PR/CI *event* to an *action* —
//! re-dispatch the agent with the event's context, merge, or escalate to a
//! human — bounded by a retry cap and a time-based escalation. This module is
//! the pure decision engine; wiring it to live `gh`/CI events is a separate,
//! remote-dependent layer.

use serde::{Deserialize, Serialize};

/// A PR/CI event observed for a task's pull request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReactionEvent {
    /// CI checks failed on the PR.
    CiFailed,
    /// A reviewer requested changes.
    ChangesRequested,
    /// Approved by a reviewer and all checks are green.
    ApprovedGreen,
}

/// What to do in response to an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReactionAction {
    /// Re-run the task's agent with the event context (logs / review) injected.
    Redispatch,
    /// Merge the pull request.
    Merge,
    /// Hand off to a human.
    Escalate,
    /// Do nothing.
    Ignore,
}

/// One rule: when `on` happens, take `action`, capped by `retries` and an
/// optional wall-clock `escalate_after_secs` after which it goes to a human.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReactionRule {
    pub on: ReactionEvent,
    pub action: ReactionAction,
    /// Trigger automatically vs. wait for human confirmation.
    #[serde(default)]
    pub auto: bool,
    /// Max number of redispatches for this event before escalating.
    #[serde(default)]
    pub retries: u32,
    /// Escalate to a human once this many seconds have elapsed since the event
    /// first appeared, regardless of the action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escalate_after_secs: Option<u64>,
}

/// An ordered set of rules; first match wins.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReactionConfig {
    pub rules: Vec<ReactionRule>,
}

impl Default for ReactionConfig {
    /// Sane defaults: fix CI automatically (≤2 tries), address review comments
    /// (≤3 tries), auto-merge once approved & green; everything escalates after
    /// 30 minutes stuck.
    fn default() -> Self {
        let escalate = Some(1800);
        ReactionConfig {
            rules: vec![
                ReactionRule {
                    on: ReactionEvent::CiFailed,
                    action: ReactionAction::Redispatch,
                    auto: true,
                    retries: 2,
                    escalate_after_secs: escalate,
                },
                ReactionRule {
                    on: ReactionEvent::ChangesRequested,
                    action: ReactionAction::Redispatch,
                    auto: true,
                    retries: 3,
                    escalate_after_secs: escalate,
                },
                ReactionRule {
                    on: ReactionEvent::ApprovedGreen,
                    action: ReactionAction::Merge,
                    auto: true,
                    retries: 0,
                    escalate_after_secs: None,
                },
            ],
        }
    }
}

/// The engine's decision for a single observed event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReactionOutcome {
    /// Re-run the agent; `context` is the human-readable why (logs/review) to
    /// inject into the retry prompt.
    Redispatch { context: String },
    /// Merge the PR.
    Merge,
    /// Hand to a human; `reason` explains why automation stopped.
    Escalate { reason: String },
    /// No matching rule / nothing to do.
    Ignore,
}

/// Decide what to do for `event`, given the rules, how many times we've already
/// reacted to it (`attempts`), how long it's been stuck (`elapsed_secs`), and a
/// short `detail` describing the event (CI log excerpt, review summary, …).
pub fn decide(
    cfg: &ReactionConfig,
    event: ReactionEvent,
    attempts: u32,
    elapsed_secs: u64,
    detail: &str,
) -> ReactionOutcome {
    let Some(rule) = cfg.rules.iter().find(|r| r.on == event) else {
        return ReactionOutcome::Ignore;
    };

    // A rule that isn't auto always defers to a human.
    if !rule.auto && rule.action != ReactionAction::Ignore {
        return ReactionOutcome::Escalate {
            reason: format!("{event:?} requires manual confirmation"),
        };
    }

    // Time-based escalation wins over the configured action.
    if let Some(after) = rule.escalate_after_secs {
        if elapsed_secs >= after {
            return ReactionOutcome::Escalate {
                reason: format!("{event:?} unresolved after {elapsed_secs}s (limit {after}s)"),
            };
        }
    }

    match rule.action {
        ReactionAction::Redispatch => {
            if attempts < rule.retries {
                ReactionOutcome::Redispatch {
                    context: format!(
                        "{event:?} (attempt {}/{}): {detail}",
                        attempts + 1,
                        rule.retries
                    ),
                }
            } else {
                ReactionOutcome::Escalate {
                    reason: format!(
                        "{event:?}: exhausted {} automatic retr{}",
                        rule.retries,
                        if rule.retries == 1 { "y" } else { "ies" }
                    ),
                }
            }
        }
        ReactionAction::Merge => ReactionOutcome::Merge,
        ReactionAction::Escalate => ReactionOutcome::Escalate {
            reason: format!("{event:?}: rule escalates to a human"),
        },
        ReactionAction::Ignore => ReactionOutcome::Ignore,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ci_failed_redispatches_then_escalates_on_retry_exhaustion() {
        let cfg = ReactionConfig::default();
        // first two failures → redispatch with context
        match decide(&cfg, ReactionEvent::CiFailed, 0, 10, "tests red") {
            ReactionOutcome::Redispatch { context } => assert!(context.contains("CiFailed")),
            other => panic!("expected redispatch, got {other:?}"),
        }
        assert!(matches!(
            decide(&cfg, ReactionEvent::CiFailed, 1, 10, "tests red"),
            ReactionOutcome::Redispatch { .. }
        ));
        // third (attempts == retries) → escalate
        assert!(matches!(
            decide(&cfg, ReactionEvent::CiFailed, 2, 10, "tests red"),
            ReactionOutcome::Escalate { .. }
        ));
    }

    #[test]
    fn approved_green_merges() {
        let cfg = ReactionConfig::default();
        assert_eq!(
            decide(&cfg, ReactionEvent::ApprovedGreen, 0, 0, ""),
            ReactionOutcome::Merge
        );
    }

    #[test]
    fn time_limit_escalates_regardless_of_retries_left() {
        let cfg = ReactionConfig::default();
        assert!(matches!(
            decide(&cfg, ReactionEvent::CiFailed, 0, 4000, "still red"),
            ReactionOutcome::Escalate { .. }
        ));
    }

    #[test]
    fn non_auto_rule_defers_to_human() {
        let cfg = ReactionConfig {
            rules: vec![ReactionRule {
                on: ReactionEvent::ChangesRequested,
                action: ReactionAction::Redispatch,
                auto: false,
                retries: 5,
                escalate_after_secs: None,
            }],
        };
        assert!(matches!(
            decide(&cfg, ReactionEvent::ChangesRequested, 0, 0, "fix naming"),
            ReactionOutcome::Escalate { .. }
        ));
    }

    #[test]
    fn unconfigured_event_is_ignored() {
        let cfg = ReactionConfig { rules: vec![] };
        assert_eq!(
            decide(&cfg, ReactionEvent::CiFailed, 0, 0, ""),
            ReactionOutcome::Ignore
        );
    }

    #[test]
    fn config_round_trips_through_yaml() {
        let cfg = ReactionConfig::default();
        let yaml = serde_yaml::to_string(&cfg).unwrap();
        let back: ReactionConfig = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(cfg, back);
    }
}
