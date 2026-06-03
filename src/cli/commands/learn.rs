use anyhow::Result;

use crate::cli::LearnCmd;
use crate::learn::{ProposalStatus, ProposalStore};
use crate::memory::MemoryStore;

pub fn run(c: LearnCmd) -> Result<()> {
    let store = ProposalStore::open()?;
    match c {
        LearnCmd::List => {
            let proposals = store.list()?;
            let pending: Vec<_> = proposals
                .iter()
                .filter(|p| p.status == ProposalStatus::Proposed)
                .collect();
            if pending.is_empty() {
                println!(
                    "(no pending learning proposals)\n  Enable with `learning.propose_guardrails: true` in .maestro/settings.yaml;\n  failed runs then distill guardrail proposals here for you to review."
                );
                return Ok(());
            }
            println!("Pending learning proposals (review, then `maestro learn promote <fingerprint> --trigger '<phrase>'`):\n");
            for p in pending {
                let trig = if p.trigger.trim().is_empty() {
                    "(none — set --trigger on promote)".to_string()
                } else {
                    format!("suggested trigger: {}", p.trigger)
                };
                println!(
                    "  {}  ×{}  [{}/{}]  {}\n      {}",
                    p.fingerprint, p.occurrences, p.kind, p.scope, p.description, trig
                );
            }
            Ok(())
        }
        LearnCmd::Show { fingerprint } => {
            let p = store.load(&fingerprint)?;
            println!("fingerprint : {}", p.fingerprint);
            println!("status      : {:?}", p.status);
            println!("kind        : {}", p.kind);
            println!("scope       : {}", p.scope);
            println!("name        : {}", p.name);
            println!("occurrences : {}", p.occurrences);
            println!("source runs : {}", p.source_runs.join(", "));
            println!(
                "trigger     : {}",
                if p.trigger.trim().is_empty() {
                    "(none — required on promote)"
                } else {
                    &p.trigger
                }
            );
            println!("\n{}", p.body);
            Ok(())
        }
        LearnCmd::Promote {
            fingerprint,
            scope,
            trigger,
        } => {
            let kind = store.load(&fingerprint).map(|p| p.kind).unwrap_or_default();
            let saved = store.promote(&fingerprint, scope.as_deref(), trigger.as_deref())?;
            if kind == "memory_curation" {
                println!("consolidated → kept {}", saved.display());
                println!("  (older near-duplicate L2 records tombstoned; no content merged)");
            } else {
                println!("promoted → {}", saved.display());
                println!("  (mirrored to .cursor/rules and .claude/skills; it fires for matching future tasks)");
            }
            Ok(())
        }
        LearnCmd::Reject { fingerprint } => {
            store.reject(&fingerprint)?;
            println!("rejected {fingerprint} (kept as an audit record; not re-proposed)");
            Ok(())
        }
        LearnCmd::ScanMemory => {
            let n = store.propose_memory_curation(&MemoryStore::open()?)?;
            if n == 0 {
                println!("(no near-duplicate L2 decisions found to curate)");
            } else {
                println!(
                    "wrote {n} memory-curation proposal(s) — review with `maestro learn list`, \
                     promote to keep the newest of each near-duplicate cluster"
                );
            }
            Ok(())
        }
    }
}
