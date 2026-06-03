//! Task routing (the Anthropic "Routing" workflow pattern): pick which agent /
//! model handles a task from its static characteristics, before it runs.
//!
//! maestro resolves agent+model per project/task already; routing adds a
//! declarative policy layer on top — e.g. send contract-critical changes to a
//! stronger model, or cheap routine work to a lighter one — without editing
//! every project. Pure + first-match-wins; an empty rule set is a no-op.

use serde::{Deserialize, Serialize};

/// What a rule matches on. Every `Some` field must match; `None` = wildcard.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteMatch {
    /// `"agent"` | `"verify"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Whether the task's project declares a contract (provides/consumes) —
    /// i.e. a change here ripples across projects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub touches_contract: Option<bool>,
}

/// When `when` matches, override the agent and/or model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteRule {
    pub when: RouteMatch,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// The overrides a route resolved to (either may be `None`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RouteOutcome {
    pub agent: Option<String>,
    pub model: Option<String>,
}

impl RouteMatch {
    fn matches(&self, kind: &str, touches_contract: bool) -> bool {
        self.kind.as_deref().is_none_or(|k| k == kind)
            && self.touches_contract.is_none_or(|c| c == touches_contract)
    }
}

/// First matching rule wins; returns its overrides (empty if nothing matches).
pub fn route(rules: &[RouteRule], kind: &str, touches_contract: bool) -> RouteOutcome {
    for r in rules {
        if r.when.matches(kind, touches_contract) {
            return RouteOutcome {
                agent: r.agent.clone(),
                model: r.model.clone(),
            };
        }
    }
    RouteOutcome::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(
        kind: Option<&str>,
        contract: Option<bool>,
        agent: Option<&str>,
        model: Option<&str>,
    ) -> RouteRule {
        RouteRule {
            when: RouteMatch {
                kind: kind.map(String::from),
                touches_contract: contract,
            },
            agent: agent.map(String::from),
            model: model.map(String::from),
        }
    }

    #[test]
    fn empty_rules_are_a_noop() {
        assert_eq!(route(&[], "agent", true), RouteOutcome::default());
    }

    #[test]
    fn contract_critical_change_routes_to_a_stronger_model() {
        let rules = vec![rule(Some("agent"), Some(true), None, Some("opus"))];
        let out = route(&rules, "agent", true);
        assert_eq!(out.model.as_deref(), Some("opus"));
        // a non-contract task isn't affected
        assert_eq!(route(&rules, "agent", false), RouteOutcome::default());
    }

    #[test]
    fn first_match_wins() {
        let rules = vec![
            rule(Some("verify"), None, Some("shell"), None),
            rule(None, Some(true), None, Some("opus")),
        ];
        // a contract verify task matches the first rule (verify), not the second
        assert_eq!(
            route(&rules, "verify", true).agent.as_deref(),
            Some("shell")
        );
        // a contract agent task falls through to the second
        assert_eq!(route(&rules, "agent", true).model.as_deref(), Some("opus"));
    }

    #[test]
    fn wildcard_kind_matches_any() {
        let rules = vec![rule(None, Some(false), Some("mock"), None)];
        assert_eq!(route(&rules, "agent", false).agent.as_deref(), Some("mock"));
        assert_eq!(
            route(&rules, "verify", false).agent.as_deref(),
            Some("mock")
        );
    }

    #[test]
    fn config_round_trips_through_yaml() {
        let rules = vec![rule(Some("agent"), Some(true), Some("codex"), Some("opus"))];
        let yaml = serde_yaml::to_string(&rules).unwrap();
        let back: Vec<RouteRule> = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(rules, back);
    }
}
