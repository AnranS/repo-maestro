//! Voting / compare selection (the Anthropic "Parallelization · Voting"
//! pattern): run the same task on several agents, then pick the best result.
//!
//! The decision is deliberately transparent and deterministic — no LLM judge.
//! A candidate is only eligible if it passed the project check; among passers
//! we prefer one that actually changed something (a no-op that happens to pass
//! the baseline check shouldn't win over real work), then the smallest diff,
//! then input order. The caller surfaces every candidate so a human can
//! override.

/// One agent's attempt at the task.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoteCandidate {
    pub agent: String,
    /// Project check passed in this candidate's worktree.
    pub verified: bool,
    /// Number of files the agent changed.
    pub files_changed: usize,
    /// Set if the agent run itself errored (spawn/exec failure).
    pub error: Option<String>,
}

/// Why a winner was (or wasn't) chosen — for the report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VoteOutcome {
    /// Index of the winning candidate + a short reason.
    Winner { index: usize, reason: String },
    /// No candidate is acceptable; reason explains (none passed, etc.).
    NoWinner { reason: String },
}

/// Pick the winner among `candidates`. Eligibility = verified. Preference:
/// changed-something over no-op, then fewer files, then input order.
pub fn select_winner(candidates: &[VoteCandidate]) -> VoteOutcome {
    if candidates.is_empty() {
        return VoteOutcome::NoWinner {
            reason: "no candidates".to_string(),
        };
    }
    let passers: Vec<usize> = candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| c.verified && c.error.is_none())
        .map(|(i, _)| i)
        .collect();
    if passers.is_empty() {
        return VoteOutcome::NoWinner {
            reason: "no candidate passed the project check".to_string(),
        };
    }

    // Prefer a passer that actually changed something; among the chosen group
    // pick the smallest diff, then the earliest in input order.
    let changed: Vec<usize> = passers
        .iter()
        .copied()
        .filter(|&i| candidates[i].files_changed > 0)
        .collect();
    let pool = if changed.is_empty() {
        &passers
    } else {
        &changed
    };
    let best = *pool
        .iter()
        .min_by_key(|&&i| (candidates[i].files_changed, i))
        .expect("non-empty pool");

    let c = &candidates[best];
    let reason = if c.files_changed == 0 {
        format!(
            "{}: passed check (no other candidate made changes)",
            c.agent
        )
    } else {
        format!(
            "{}: passed check with the smallest passing change ({} file{})",
            c.agent,
            c.files_changed,
            if c.files_changed == 1 { "" } else { "s" }
        )
    };
    VoteOutcome::Winner {
        index: best,
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(agent: &str, verified: bool, files: usize) -> VoteCandidate {
        VoteCandidate {
            agent: agent.into(),
            verified,
            files_changed: files,
            error: None,
        }
    }

    #[test]
    fn prefers_a_passer_that_made_changes_over_a_passing_noop() {
        // mock listed first but changed nothing; codex made the change + passed
        let cands = vec![cand("mock", true, 0), cand("codex", true, 3)];
        match select_winner(&cands) {
            VoteOutcome::Winner { index, .. } => assert_eq!(index, 1, "codex should win"),
            o => panic!("expected winner, got {o:?}"),
        }
    }

    #[test]
    fn among_changers_picks_the_smallest_diff() {
        let cands = vec![cand("a", true, 9), cand("b", true, 2), cand("c", true, 5)];
        assert_eq!(
            select_winner(&cands),
            VoteOutcome::Winner {
                index: 1,
                reason: "b: passed check with the smallest passing change (2 files)".to_string(),
            }
        );
    }

    #[test]
    fn no_winner_when_none_pass() {
        let cands = vec![cand("a", false, 3), cand("b", false, 1)];
        assert!(matches!(
            select_winner(&cands),
            VoteOutcome::NoWinner { .. }
        ));
    }

    #[test]
    fn errored_candidate_is_ineligible_even_if_marked_verified() {
        let mut c = cand("flaky", true, 2);
        c.error = Some("spawn failed".into());
        let cands = vec![c, cand("ok", true, 4)];
        match select_winner(&cands) {
            VoteOutcome::Winner { index, .. } => assert_eq!(index, 1),
            o => panic!("expected winner, got {o:?}"),
        }
    }

    #[test]
    fn all_noop_passers_still_yield_a_winner() {
        let cands = vec![cand("a", true, 0), cand("b", true, 0)];
        match select_winner(&cands) {
            VoteOutcome::Winner { index, .. } => assert_eq!(index, 0),
            o => panic!("expected winner, got {o:?}"),
        }
    }
}
