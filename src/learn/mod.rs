//! Failure-driven learning: PROPOSE → REVIEW → PROMOTE.
//!
//! Maestro already learns *passively* (it archives L2 decisions after a verified
//! run and fans them back into related tasks). This module adds the first
//! *active* learning loop while preserving Maestro's deterministic, auditable,
//! human-governed nature:
//!
//!   1. PROPOSE — when enabled (`learning.propose_guardrails`, OFF by default),
//!      a finished run's FAILURES are distilled into inert "guardrail"
//!      proposals under `.maestro/proposals/`. This is purely deterministic
//!      (no LLM): each proposal is keyed by a fingerprint of the failure
//!      signature (reusing the same normalization the circuit-breaker uses), so
//!      a recurring failure bumps an `occurrences` counter instead of spamming
//!      duplicates. A proposal NEVER affects any future run — it is not a skill,
//!      not in any injection path.
//!   2. REVIEW — `maestro learn list/show` surface the proposals (highest
//!      recurrence first) for a human to read in full.
//!   3. PROMOTE — `maestro learn promote <fp> --trigger '<phrase>'` is the ONLY
//!      thing that changes behavior: it writes a normal skill via
//!      `skills::save` (which mirrors to `.cursor`/`.claude`), after which the
//!      guardrail fires for matching future tasks through the unchanged skill
//!      machinery. `reject` keeps the proposal as an audit record.
//!
//! The trigger is deliberately kept conservative (often empty) so an
//! auto-derived guardrail can never silently flood unrelated prompts; promotion
//! requires a concrete trigger, authored or confirmed by a human.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use crate::config::{Plan, TaskKind};
use crate::memory::MemoryStore;
use crate::paths;
use crate::scheduler::executor_util::error_signature;
use crate::scheduler::state::{RunState, TaskStatus};
use crate::skills::{self, SkillScope};

const PROPOSALS_DIR: &str = "proposals";
const EVIDENCE_CAP: usize = 800;
/// Cosine threshold above which two L2 decisions are treated as near-duplicates.
const CURATE_THRESHOLD: f64 = 0.82;
/// The placeholder `archive_l2_decision` writes; its presence means the record's
/// "what to remember" section is still un-edited, so the record is safe to dedup.
const L2_DEFAULT_PLACEHOLDER: &str = "_(Optional human edit) Capture the decision rationale here";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    Proposed,
    Promoted,
    Rejected,
}

/// The YAML frontmatter persisted for a proposal (everything except the body).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProposalFront {
    fingerprint: String,
    status: ProposalStatus,
    /// Always `guardrail` in v1; the field keeps the store open to future kinds.
    kind: String,
    /// Target skill scope when promoted: `_global` or a project name.
    scope: String,
    name: String,
    description: String,
    #[serde(default)]
    trigger: String,
    occurrences: u32,
    #[serde(default)]
    source_runs: Vec<String>,
    /// Kind-specific data. For `memory_curation`: the L2 relative paths in the
    /// near-duplicate cluster, newest first. Empty otherwise.
    #[serde(default)]
    payload: Vec<String>,
}

/// One learning proposal = its frontmatter + the guardrail markdown body.
#[derive(Debug, Clone)]
pub struct Proposal {
    pub fingerprint: String,
    pub status: ProposalStatus,
    pub kind: String,
    pub scope: String,
    pub name: String,
    pub description: String,
    pub trigger: String,
    pub occurrences: u32,
    pub source_runs: Vec<String>,
    pub payload: Vec<String>,
    pub body: String,
}

impl Proposal {
    fn front(&self) -> ProposalFront {
        ProposalFront {
            fingerprint: self.fingerprint.clone(),
            status: self.status,
            kind: self.kind.clone(),
            scope: self.scope.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            trigger: self.trigger.clone(),
            occurrences: self.occurrences,
            source_runs: self.source_runs.clone(),
            payload: self.payload.clone(),
        }
    }
}

/// A single distilled failure observation from a run.
#[derive(Debug, Clone)]
pub struct FailureSignal {
    pub project: String,
    pub kind: String,
    pub fingerprint: String,
    pub evidence: String,
}

/// Store of learning proposals, rooted at `.maestro/proposals/`.
pub struct ProposalStore {
    dir: PathBuf,
}

impl ProposalStore {
    pub fn open() -> Result<Self> {
        let dir = paths::maestro_dir()?.join(PROPOSALS_DIR);
        paths::ensure_dir(&dir)?;
        Ok(Self { dir })
    }

    /// For tests: a store rooted at an explicit directory.
    pub fn open_at(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn path_for(&self, fingerprint: &str) -> Result<PathBuf> {
        paths::validate_path_component("proposal fingerprint", fingerprint)?;
        Ok(self.dir.join(format!("{fingerprint}.md")))
    }

    /// All proposals, proposed-first then highest recurrence first.
    pub fn list(&self) -> Result<Vec<Proposal>> {
        let mut out = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().and_then(|s| s.to_str()) == Some("md") {
                    if let Ok(raw) = std::fs::read_to_string(&p) {
                        if let Some(prop) = parse_proposal(&raw) {
                            out.push(prop);
                        }
                    }
                }
            }
        }
        out.sort_by(|a, b| {
            let pa = a.status == ProposalStatus::Proposed;
            let pb = b.status == ProposalStatus::Proposed;
            pb.cmp(&pa)
                .then(b.occurrences.cmp(&a.occurrences))
                .then(a.fingerprint.cmp(&b.fingerprint))
        });
        Ok(out)
    }

    pub fn load(&self, fingerprint: &str) -> Result<Proposal> {
        let path = self.path_for(fingerprint)?;
        let raw =
            std::fs::read_to_string(&path).with_context(|| format!("read proposal {:?}", path))?;
        parse_proposal(&raw).with_context(|| format!("parse proposal {:?}", path))
    }

    fn write(&self, p: &Proposal) -> Result<PathBuf> {
        paths::ensure_dir(&self.dir)?;
        let path = self.path_for(&p.fingerprint)?;
        let yaml = serde_yaml::to_string(&p.front()).context("serialize proposal frontmatter")?;
        let content = format!("---\n{yaml}---\n\n{}\n", p.body.trim_end());
        std::fs::write(&path, content).with_context(|| format!("write proposal {:?}", path))?;
        Ok(path)
    }

    /// Insert a new proposal, or — if one with the same fingerprint already
    /// exists and is still `Proposed` — bump its recurrence. Promoted/rejected
    /// fingerprints are intentionally left alone (never re-proposed). Returns
    /// true when a NEW proposal was written.
    fn upsert(&self, p: Proposal) -> Result<bool> {
        let path = self.path_for(&p.fingerprint)?;
        if path.exists() {
            let mut existing = self.load(&p.fingerprint)?;
            if existing.status == ProposalStatus::Proposed {
                existing.occurrences = existing.occurrences.saturating_add(1);
                for run in &p.source_runs {
                    if !existing.source_runs.contains(run) {
                        existing.source_runs.push(run.clone());
                    }
                }
                self.write(&existing)?;
            }
            Ok(false)
        } else {
            self.write(&p)?;
            Ok(true)
        }
    }

    /// Distill a finished run's failures into guardrail proposals. Returns the
    /// number of NEW proposals written (recurring fingerprints bump an existing
    /// proposal's `occurrences` instead). Deterministic; no LLM.
    pub fn propose_from_failures(&self, state: &RunState) -> Result<usize> {
        let mut written = 0usize;
        for signal in extract_failure_signals(state) {
            if self.upsert(guardrail_from_signal(&signal, &state.run_id))? {
                written += 1;
            }
        }
        Ok(written)
    }

    /// Draft a reusable skill PLAYBOOK proposal from a verified, COMPLEX run
    /// (multi-project, contract-wiring, or many tasks). Deterministic — Maestro
    /// detects the reusable shape and drafts a skeleton; a human turns it into a
    /// real playbook and authors the trigger at promote time. Returns the number
    /// of NEW proposals written (0 or 1; a recurring run-shape bumps occurrences).
    pub fn synthesize_skill_proposal(&self, state: &RunState, plan: &Plan) -> Result<usize> {
        let shape = run_shape(state, plan);
        if !shape.is_complex() {
            return Ok(0);
        }
        Ok(self.upsert(skill_from_shape(state, plan, &shape))? as usize)
    }

    /// Mark a proposal rejected (kept as an audit record, never re-proposed).
    pub fn reject(&self, fingerprint: &str) -> Result<()> {
        let mut p = self.load(fingerprint)?;
        p.status = ProposalStatus::Rejected;
        self.write(&p)?;
        Ok(())
    }

    /// Promote a proposal — the ONLY operation that changes future behavior.
    /// `guardrail`/`skill` proposals become a live skill (a non-empty `trigger`
    /// is REQUIRED so a guardrail can't fire on everything); a `memory_curation`
    /// proposal consolidates its near-duplicate L2 cluster (keep newest,
    /// tombstone the rest — no trigger/scope needed). Returns the resulting path.
    pub fn promote(
        &self,
        fingerprint: &str,
        scope: Option<&str>,
        trigger: Option<&str>,
    ) -> Result<PathBuf> {
        let mut p = self.load(fingerprint)?;
        if p.status != ProposalStatus::Proposed {
            anyhow::bail!("proposal {fingerprint} is already {:?}", p.status);
        }
        let result = if p.kind == "memory_curation" {
            apply_memory_curation(&MemoryStore::open()?, &p)?
        } else {
            let effective_trigger = trigger
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| p.trigger.trim().to_string());
            if effective_trigger.is_empty() {
                anyhow::bail!(
                    "proposal {fingerprint} has no trigger — pass --trigger '<phrase the skill should fire on>'.\n  \
                     Triggers are kept conservative on purpose so a learned guardrail never floods unrelated prompts."
                );
            }
            let scope = scope
                .map(|s| s.to_string())
                .unwrap_or_else(|| p.scope.clone());
            let skill_scope = if scope == "global" || scope == "_global" {
                SkillScope::Global
            } else {
                SkillScope::Project(scope.clone())
            };
            let content = skill_markdown(&p, &effective_trigger)?;
            let saved = skills::save(&skill_scope, &p.name, &content)?;
            p.trigger = effective_trigger;
            p.scope = scope;
            saved
        };
        p.status = ProposalStatus::Promoted;
        self.write(&p)?;
        Ok(result)
    }

    /// Scan archived L2 decisions for near-duplicate clusters within a project
    /// and write a `memory_curation` proposal for each. Deterministic (a local
    /// TF-IDF cosine over each project's records); records whose "what to
    /// remember" section a human has edited are excluded. On-demand
    /// (`maestro learn scan-memory`) — never auto-applied. Returns NEW count.
    pub fn propose_memory_curation(&self, memory: &MemoryStore) -> Result<usize> {
        let l2 = memory.l2_root();
        let mut written = 0usize;
        let Ok(projects) = std::fs::read_dir(&l2) else {
            return Ok(0);
        };
        for proj_entry in projects.flatten() {
            if !proj_entry.path().is_dir() {
                continue;
            }
            let project = proj_entry.file_name().to_string_lossy().to_string();
            // (relative path, content) for eligible — i.e. un-edited — records.
            let mut docs: Vec<(String, String)> = Vec::new();
            if let Ok(files) = std::fs::read_dir(proj_entry.path()) {
                for f in files.flatten() {
                    let path = f.path();
                    if path.extension().and_then(|s| s.to_str()) != Some("md") {
                        continue;
                    }
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        // A human-edited "what to remember" section drops the
                        // placeholder → pin (exclude from dedup) to never eat a
                        // hand-written rationale.
                        if content.contains(L2_DEFAULT_PLACEHOLDER) {
                            let rel = format!("{project}/{}", f.file_name().to_string_lossy());
                            docs.push((rel, content));
                        }
                    }
                }
            }
            if docs.len() < 2 {
                continue;
            }
            for cluster in cluster_near_duplicates(&docs, CURATE_THRESHOLD) {
                let mut files: Vec<String> = cluster.iter().map(|i| docs[*i].0.clone()).collect();
                files.sort();
                files.reverse(); // newest (lexicographic-max date prefix) first
                if self.upsert(curation_proposal(&project, &files))? {
                    written += 1;
                }
            }
        }
        Ok(written)
    }
}

/// Distill a run's failures into deterministic, fingerprinted signals: failed
/// acceptance checks, circuit-break / integration-conflict auto-actions, and
/// failed tasks. Deduped by fingerprint.
pub fn extract_failure_signals(state: &RunState) -> Vec<FailureSignal> {
    let mut out: Vec<FailureSignal> = Vec::new();

    for ac in &state.acceptance_results {
        if !ac.passed {
            let sig = error_signature(&format!("{} {}", ac.check, ac.output));
            push_unique(
                &mut out,
                FailureSignal {
                    project: "_global".into(),
                    kind: "acceptance_fail".into(),
                    fingerprint: fingerprint("_global", &sig),
                    evidence: format!("check `{}` failed:\n{}", ac.check, ac.output),
                },
            );
        }
    }

    // Task failures carry the real, task-specific error — distill them first
    // and remember which tasks actually produced a `task_failed` signal.
    // (Track the tasks we SIGNALLED, not merely those marked Failed: a Failed
    // task with no `error` — old run, odd deserialized state — produces no
    // signal, so its circuit-break must NOT be suppressed, or we'd drop the
    // fallback and emit zero signals. dali N1.)
    let mut task_failed_signal_tasks: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for (task_id, ts) in &state.tasks {
        if matches!(ts.status, TaskStatus::Failed) {
            if let Some(err) = &ts.error {
                let sig = error_signature(err);
                push_unique(
                    &mut out,
                    FailureSignal {
                        project: ts.project.clone(),
                        kind: "task_failed".into(),
                        fingerprint: fingerprint(&ts.project, &sig),
                        evidence: err.clone(),
                    },
                );
                task_failed_signal_tasks.insert(task_id.clone());
            }
        }
    }

    for a in &state.auto_actions {
        if !matches!(a.kind.as_str(), "circuit_break" | "integration_conflict") {
            continue;
        }
        // A circuit-break is a *consequence* of an underlying task failure that's
        // already captured above; its detail ("stopped retrying after N identical
        // failures") carries no task-specific lesson. Skip it when the same task
        // already produced a `task_failed` signal, so one failing task doesn't
        // yield two near-duplicate guardrail proposals (F-LEARN-001). Keep it
        // only as a fallback (a circuit-break with no Failed task) and always
        // keep `integration_conflict` — that *is* a task-specific lesson.
        if a.kind == "circuit_break" {
            if let Some(t) = &a.task {
                if task_failed_signal_tasks.contains(t) {
                    continue;
                }
            }
        }
        let project = a
            .task
            .as_ref()
            .and_then(|t| state.tasks.get(t))
            .map(|ts| ts.project.clone())
            .unwrap_or_else(|| "_global".into());
        let sig = error_signature(&a.detail);
        push_unique(
            &mut out,
            FailureSignal {
                project: project.clone(),
                kind: a.kind.clone(),
                fingerprint: fingerprint(&project, &sig),
                evidence: a.detail.clone(),
            },
        );
    }

    out
}

fn push_unique(out: &mut Vec<FailureSignal>, sig: FailureSignal) {
    if !out.iter().any(|s| s.fingerprint == sig.fingerprint) {
        out.push(sig);
    }
}

/// Deterministic, std-version-independent fingerprint (FNV-1a) of a normalized
/// failure signature, scoped by project. Doubles as the proposal filename stem.
fn fingerprint(project: &str, signature: &str) -> String {
    let s = format!("{project}::{signature}");
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

fn guardrail_from_signal(sig: &FailureSignal, run_id: &str) -> Proposal {
    let short = &sig.fingerprint[..8.min(sig.fingerprint.len())];
    Proposal {
        fingerprint: sig.fingerprint.clone(),
        status: ProposalStatus::Proposed,
        kind: "guardrail".into(),
        scope: sig.project.clone(),
        name: format!("guardrail-{short}"),
        description: format!("Guardrail learned from {} in {}", sig.kind, sig.project),
        trigger: suggest_trigger(&sig.evidence),
        occurrences: 1,
        source_runs: vec![run_id.to_string()],
        payload: Vec::new(),
        body: render_guardrail_body(sig, run_id),
    }
}

fn render_guardrail_body(sig: &FailureSignal, run_id: &str) -> String {
    let evidence = sanitize_fence(&truncate_chars(&sig.evidence, EVIDENCE_CAP));
    format!(
        "# Guardrail (learned from a past failure)\n\n\
         A task in `{project}` previously hit a `{kind}` failure (run `{run_id}`). \
         Before doing similar work, account for what went wrong below so it isn't repeated.\n\n\
         **What went wrong**\n\n```\n{evidence}\n```\n\n\
         **Guardrail** _(review and tighten this before promoting)_\n\n\
         - Replace this line with the concrete do/don't that avoids the failure above.\n",
        project = sig.project,
        kind = sig.kind,
    )
}

/// The deterministic "shape" of a run, used to decide whether it's worth
/// drafting a reusable skill and to key recurring run-shapes.
struct RunShape {
    projects: Vec<String>,
    agent_tasks: usize,
    contracts: Vec<String>,
}

impl RunShape {
    /// Worth distilling into a reusable playbook? Multi-project, contract
    /// wiring, or a non-trivial number of tasks.
    fn is_complex(&self) -> bool {
        // A playbook captures a reusable WORK shape (ordered agent steps,
        // contracts, what verified it), so it needs actual work to replay —
        // at least one agent task. Without this a verified verify-only run
        // (e.g. a multi-project audit) was synthesized into a useless
        // "0-task run" playbook with no steps (F-LEARN-002). The breadth
        // signal (multi-project / contract-wiring / many tasks) still gates
        // on top of that.
        self.agent_tasks >= 1
            && (self.projects.len() >= 2 || !self.contracts.is_empty() || self.agent_tasks >= 3)
    }

    /// Deterministic confidence bucket from evidence signals (NOT model-reported).
    fn confidence(&self) -> &'static str {
        let multi = self.projects.len() >= 2;
        let contract = !self.contracts.is_empty();
        if multi && contract {
            "high"
        } else if multi || contract {
            "medium"
        } else {
            "low"
        }
    }

    fn fingerprint(&self) -> String {
        let mut projects = self.projects.clone();
        projects.sort();
        let mut contracts: Vec<String> = self.contracts.iter().map(|c| last_segment(c)).collect();
        contracts.sort();
        contracts.dedup();
        fingerprint(
            "skill-shape",
            &format!("{}|{}", projects.join(","), contracts.join(",")),
        )
    }
}

fn run_shape(state: &RunState, plan: &Plan) -> RunShape {
    let mut projects: Vec<String> = state
        .tasks
        .values()
        .map(|t| t.project.clone())
        .filter(|p| !p.is_empty() && p != "_global")
        .collect();
    projects.sort();
    projects.dedup();
    let agent_tasks = plan
        .tasks
        .iter()
        .filter(|t| matches!(t.kind, TaskKind::Agent))
        .count();
    let contracts: Vec<String> = state
        .auto_actions
        .iter()
        .filter(|a| a.kind == "contract_wired")
        .map(|a| a.detail.clone())
        .collect();
    RunShape {
        projects,
        agent_tasks,
        contracts,
    }
}

fn skill_from_shape(state: &RunState, plan: &Plan, shape: &RunShape) -> Proposal {
    let fp = shape.fingerprint();
    let short = &fp[..8.min(fp.len())];
    let scope = if shape.projects.len() == 1 {
        shape.projects[0].clone()
    } else {
        "_global".into()
    };
    let projects_label = if shape.projects.is_empty() {
        "_global".to_string()
    } else {
        shape.projects.join("+")
    };
    Proposal {
        fingerprint: fp.clone(),
        status: ProposalStatus::Proposed,
        kind: "skill".into(),
        scope,
        name: format!("play-{short}"),
        description: format!(
            "Playbook drafted from a {}-task run across {} (confidence: {})",
            shape.agent_tasks,
            projects_label,
            shape.confidence()
        ),
        trigger: String::new(),
        occurrences: 1,
        source_runs: vec![state.run_id.clone()],
        payload: Vec::new(),
        body: render_skill_body(state, plan, shape),
    }
}

fn render_skill_body(state: &RunState, plan: &Plan, shape: &RunShape) -> String {
    let mut steps = String::new();
    for tid in &state.task_order {
        let project = state
            .tasks
            .get(tid)
            .map(|t| t.project.clone())
            .unwrap_or_default();
        let what = plan
            .tasks
            .iter()
            .find(|t| &t.id == tid)
            .map(|t| {
                t.prompt
                    .lines()
                    .next()
                    .map(|l| l.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .or_else(|| t.command.clone())
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        let what = sanitize_fence(&truncate_chars(&what, 120));
        let project = if project.is_empty() {
            "_global"
        } else {
            &project
        };
        steps.push_str(&format!("- `{project}` — {what}\n"));
    }
    let contracts = if shape.contracts.is_empty() {
        "none".to_string()
    } else {
        let mut cc: Vec<String> = shape.contracts.iter().map(|c| last_segment(c)).collect();
        cc.sort();
        cc.dedup();
        cc.join(", ")
    };
    let passed: Vec<&str> = state
        .acceptance_results
        .iter()
        .filter(|r| r.passed)
        .map(|r| r.check.as_str())
        .collect();
    let verified = if passed.is_empty() {
        "(no acceptance checks)".to_string()
    } else {
        passed.join("; ")
    };
    let projects = if shape.projects.is_empty() {
        "_global".to_string()
    } else {
        shape.projects.join(", ")
    };
    format!(
        "# Playbook (drafted from a successful run)\n\n\
         A verified run landed a change across `{projects}`. This is a deterministic draft of the \
         reusable steps — review and turn it into a real how-to before promoting.\n\n\
         **Goal of that run:** {spec}\n\n\
         **What it did, in order**\n\n{steps}\n\
         **Contracts involved:** {contracts}\n\n\
         **Verified by:** {verified}\n\n\
         **Playbook** _(replace with the generalizable how-to before promoting)_\n\n\
         - Replace this with the concrete, reusable steps for this kind of change.\n",
        spec = sanitize_fence(&truncate_chars(&state.spec, 200)),
    )
}

fn last_segment(s: &str) -> String {
    s.rsplit(['/', '\\']).next().unwrap_or(s).to_string()
}

// ── memory curation: near-duplicate L2 clustering (new code; the retrieval
//    scorer only does query-vs-doc, so this pairwise doc-vs-doc TF-IDF is
//    self-contained) ──────────────────────────────────────────────────────

fn tokenize_curate(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 3 && !t.chars().all(|c| c.is_ascii_digit()))
        .map(|t| t.to_ascii_lowercase())
        .collect()
}

/// Cluster documents whose pairwise TF-IDF cosine similarity is `>= threshold`,
/// via union-find. Returns only clusters of size >= 2 (the dedup candidates).
fn cluster_near_duplicates(docs: &[(String, String)], threshold: f64) -> Vec<Vec<usize>> {
    let n = docs.len();
    let toks: Vec<Vec<String>> = docs.iter().map(|(_, c)| tokenize_curate(c)).collect();
    let mut df: HashMap<&str, usize> = HashMap::new();
    for t in &toks {
        for w in t.iter().collect::<BTreeSet<&String>>() {
            *df.entry(w.as_str()).or_default() += 1;
        }
    }
    let vecs: Vec<HashMap<&str, f64>> = toks
        .iter()
        .map(|t| {
            let mut tf: HashMap<&str, f64> = HashMap::new();
            for w in t {
                *tf.entry(w.as_str()).or_default() += 1.0;
            }
            for (w, weight) in tf.iter_mut() {
                let idf = ((n as f64 + 1.0) / (df.get(w).copied().unwrap_or(1) as f64)).ln() + 1.0;
                *weight *= idf;
            }
            tf
        })
        .collect();

    let mut parent: Vec<usize> = (0..n).collect();
    for i in 0..n {
        for j in (i + 1)..n {
            if cosine(&vecs[i], &vecs[j]) >= threshold {
                let (ri, rj) = (uf_find(&mut parent, i), uf_find(&mut parent, j));
                parent[ri] = rj;
            }
        }
    }
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        let r = uf_find(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }
    groups.into_values().filter(|g| g.len() >= 2).collect()
}

fn uf_find(parent: &mut Vec<usize>, x: usize) -> usize {
    if parent[x] != x {
        let r = uf_find(parent, parent[x]);
        parent[x] = r;
    }
    parent[x]
}

fn cosine(a: &HashMap<&str, f64>, b: &HashMap<&str, f64>) -> f64 {
    let dot: f64 = a.iter().filter_map(|(k, v)| b.get(k).map(|w| v * w)).sum();
    let na = a.values().map(|v| v * v).sum::<f64>().sqrt();
    let nb = b.values().map(|v| v * v).sum::<f64>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

fn curation_proposal(project: &str, files: &[String]) -> Proposal {
    let mut sorted = files.to_vec();
    sorted.sort();
    let fp = fingerprint(
        "memory-curation",
        &format!("{project}|{}", sorted.join(",")),
    );
    let short = &fp[..8.min(fp.len())];
    let keep = files.first().cloned().unwrap_or_default();
    let tombstone = files
        .iter()
        .skip(1)
        .map(|f| format!("- `{f}`"))
        .collect::<Vec<_>>()
        .join("\n");
    let body = format!(
        "# Memory curation (near-duplicate L2 decisions)\n\n\
         Project `{project}` has {} near-duplicate L2 decision records. Promoting keeps the NEWEST and \
         tombstones the older near-duplicates — deterministic, no content is merged, so no decision is \
         conflated (e.g. two different contract versions are never fused into one).\n\n\
         **Keep (newest):** `{keep}`\n\n\
         **Tombstone:**\n{tombstone}\n",
        files.len(),
    );
    Proposal {
        fingerprint: fp.clone(),
        status: ProposalStatus::Proposed,
        kind: "memory_curation".into(),
        scope: project.to_string(),
        name: format!("curate-{short}"),
        description: format!(
            "Dedup {} near-duplicate L2 records in {}",
            files.len(),
            project
        ),
        trigger: String::new(),
        occurrences: 1,
        source_runs: Vec::new(),
        payload: files.to_vec(),
        body,
    }
}

/// Apply a memory-curation proposal: among its cluster, keep the newest existing
/// record and delete the older near-duplicates. Deterministic; no content merge.
fn apply_memory_curation(memory: &MemoryStore, p: &Proposal) -> Result<PathBuf> {
    let l2 = memory.l2_root();
    let existing: Vec<&String> = p
        .payload
        .iter()
        .filter(|f| !f.contains("..") && l2.join(f).exists())
        .collect();
    if existing.len() < 2 {
        anyhow::bail!(
            "curation cluster for {} no longer has >= 2 existing records (memory changed) — nothing to do",
            p.fingerprint
        );
    }
    let keep = existing
        .iter()
        .max_by(|a, b| a.as_str().cmp(b.as_str()))
        .copied()
        .unwrap();
    for f in &existing {
        if *f == keep {
            continue;
        }
        let _ = std::fs::remove_file(l2.join(f));
    }
    Ok(l2.join(keep))
}

/// Suggest a CONSERVATIVE trigger from failure evidence: only high-specificity
/// tokens (paths, dotted names, identifiers with uppercase), never generic
/// build/test words. Returns "" when nothing specific is found — promotion then
/// requires the human to author a trigger.
fn suggest_trigger(evidence: &str) -> String {
    const STOP: &[&str] = &[
        "error", "errors", "failed", "failure", "test", "tests", "build", "lint", "check",
        "checks", "cargo", "npm", "pnpm", "yarn", "running", "exited", "status", "code", "stderr",
        "stdout", "warning", "command", "exit", "panic", "thread", "result",
    ];
    let mut toks: Vec<String> = Vec::new();
    for raw in evidence.split(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                ',' | ';' | '(' | ')' | '"' | '\'' | '`' | '[' | ']' | ':'
            )
    }) {
        let t =
            raw.trim_matches(|c: char| !c.is_alphanumeric() && !matches!(c, '/' | '.' | '_' | '-'));
        if t.len() < 5 {
            continue;
        }
        if STOP.contains(&t.to_ascii_lowercase().as_str()) {
            continue;
        }
        let specific = t.contains('/')
            || (t.contains('.') && t.len() > 6)
            || t.chars().any(|c| c.is_ascii_uppercase());
        if specific && !toks.iter().any(|x| x == t) {
            toks.push(t.to_string());
            if toks.len() >= 3 {
                break;
            }
        }
    }
    toks.join("|")
}

fn skill_markdown(p: &Proposal, trigger: &str) -> Result<String> {
    #[derive(Serialize)]
    struct SkillFrontOut {
        name: String,
        description: String,
        trigger: String,
    }
    let front = serde_yaml::to_string(&SkillFrontOut {
        name: p.name.clone(),
        description: p.description.clone(),
        trigger: trigger.to_string(),
    })
    .context("serialize skill frontmatter")?;
    Ok(format!("---\n{front}---\n\n{}\n", p.body.trim_end()))
}

/// Split `---\n<yaml>\n---\n\n<body>` into a parsed Proposal.
fn parse_proposal(raw: &str) -> Option<Proposal> {
    let s = raw.strip_prefix("---")?.trim_start_matches('\n');
    let end = s.find("\n---")?;
    let front_str = &s[..end];
    let body = s[end + 4..].trim_start_matches('\n');
    let front: ProposalFront = serde_yaml::from_str(front_str).ok()?;
    Some(Proposal {
        fingerprint: front.fingerprint,
        status: front.status,
        kind: front.kind,
        scope: front.scope,
        name: front.name,
        description: front.description,
        trigger: front.trigger,
        occurrences: front.occurrences,
        source_runs: front.source_runs,
        payload: front.payload,
        body: body.to_string(),
    })
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let capped: String = s.chars().take(max).collect();
    format!("{capped}\n… (truncated)")
}

fn sanitize_fence(s: &str) -> String {
    s.replace("```", "ʼʼʼ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::state::{AcceptanceResult, AutoAction};
    use chrono::Utc;

    fn empty_state() -> RunState {
        let plan = crate::config::Plan {
            spec: "t".into(),
            created_by: None,
            confirmed_at: None,
            contracts_change: vec![],
            tasks: vec![],
            verification: Default::default(),
            notice: None,
            goal: None,
        };
        let projects = crate::config::ProjectsConfig {
            version: 1,
            defaults: Default::default(),
            projects: Default::default(),
        };
        RunState::new("run-1".into(), &plan, &projects, 1, std::env::temp_dir())
    }

    fn failed_acceptance() -> AcceptanceResult {
        AcceptanceResult {
            describe: "build".into(),
            check: "cargo build -p shared/types".into(),
            passed: false,
            exit_code: Some(101),
            output: "error[E0432]: unresolved import `shared/types/index`".into(),
            started_at: Utc::now(),
            ended_at: Utc::now(),
        }
    }

    #[test]
    fn extracts_and_fingerprints_failures_deterministically() {
        let mut state = empty_state();
        state.acceptance_results.push(failed_acceptance());
        state.auto_actions.push(AutoAction {
            kind: "integration_conflict".into(),
            task: None,
            detail: "integration apply failed: shared/types/index.d.ts".into(),
        });
        let a = extract_failure_signals(&state);
        let b = extract_failure_signals(&state);
        assert_eq!(a.len(), 2);
        let fa: Vec<&str> = a.iter().map(|s| s.fingerprint.as_str()).collect();
        let fb: Vec<&str> = b.iter().map(|s| s.fingerprint.as_str()).collect();
        assert_eq!(fa, fb, "fingerprints must be deterministic");
    }

    #[test]
    fn circuit_break_is_subsumed_by_its_task_failure() {
        // F-LEARN-001: a single failing task that also trips the circuit
        // breaker must yield ONE guardrail signal (the task_failed, which
        // carries the real error), not two — the circuit_break is a
        // consequence with no task-specific lesson.
        let plan: crate::config::Plan = serde_yaml::from_str(
            "spec: t\ntasks:\n  - id: T_x\n    project: p\n    prompt: do x\n",
        )
        .unwrap();
        let mut state = state_from_plan(&plan);
        if let Some(ts) = state.tasks.get_mut("T_x") {
            ts.status = TaskStatus::Failed;
            ts.error = Some("shell exited with status Some(1)".into());
        }
        state.auto_actions.push(AutoAction {
            kind: "circuit_break".into(),
            task: Some("T_x".into()),
            detail: "stopped retrying after 2 identical failure(s)".into(),
        });

        let sigs = extract_failure_signals(&state);
        assert_eq!(
            sigs.len(),
            1,
            "one failing task + its circuit-break must collapse to one signal, got {:?}",
            sigs.iter().map(|s| &s.kind).collect::<Vec<_>>()
        );
        assert_eq!(sigs[0].kind, "task_failed");

        // A circuit-break with no corresponding Failed task is still kept.
        let mut orphan = state_from_plan(&plan);
        orphan.auto_actions.push(AutoAction {
            kind: "circuit_break".into(),
            task: Some("T_ghost".into()),
            detail: "stopped retrying".into(),
        });
        let osigs = extract_failure_signals(&orphan);
        assert_eq!(osigs.len(), 1);
        assert_eq!(osigs[0].kind, "circuit_break");
    }

    #[test]
    fn failed_without_error_keeps_circuit_break_fallback() {
        // dali N1: a task marked Failed but with no `error` produces no
        // task_failed signal, so its circuit-break must NOT be suppressed —
        // otherwise we'd drop the only signal and learn nothing from the run.
        let plan: crate::config::Plan = serde_yaml::from_str(
            "spec: t\ntasks:\n  - id: T_x\n    project: p\n    prompt: do x\n",
        )
        .unwrap();
        let mut state = state_from_plan(&plan);
        if let Some(ts) = state.tasks.get_mut("T_x") {
            ts.status = TaskStatus::Failed;
            ts.error = None; // Failed, but no error text recorded
        }
        state.auto_actions.push(AutoAction {
            kind: "circuit_break".into(),
            task: Some("T_x".into()),
            detail: "stopped retrying after 2 identical failure(s)".into(),
        });

        let sigs = extract_failure_signals(&state);
        assert_eq!(
            sigs.len(),
            1,
            "Failed-without-error must keep the circuit_break fallback, got {:?}",
            sigs.iter().map(|s| &s.kind).collect::<Vec<_>>()
        );
        assert_eq!(sigs[0].kind, "circuit_break");
    }

    #[test]
    fn propose_writes_then_recurs_by_fingerprint() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ProposalStore::open_at(tmp.path().to_path_buf());
        let mut state = empty_state();
        state.acceptance_results.push(failed_acceptance());

        let n1 = store.propose_from_failures(&state).unwrap();
        assert_eq!(n1, 1, "first run writes one proposal");

        // A second run with the SAME failure bumps occurrences, not a duplicate.
        let mut state2 = empty_state();
        state2.run_id = "run-2".into();
        state2.acceptance_results.push(failed_acceptance());
        let n2 = store.propose_from_failures(&state2).unwrap();
        assert_eq!(n2, 0, "recurring failure does not create a new proposal");

        let pending = store.list().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].occurrences, 2);
        assert_eq!(pending[0].source_runs, vec!["run-1", "run-2"]);
    }

    #[test]
    fn promote_requires_a_trigger() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ProposalStore::open_at(tmp.path().to_path_buf());
        // A proposal whose evidence yields no specific token → empty trigger.
        let p = Proposal {
            fingerprint: "deadbeefdeadbeef".into(),
            status: ProposalStatus::Proposed,
            kind: "guardrail".into(),
            scope: "_global".into(),
            name: "guardrail-deadbeef".into(),
            description: "x".into(),
            trigger: String::new(),
            occurrences: 1,
            source_runs: vec!["run-1".into()],
            payload: Vec::new(),
            body: "# g".into(),
        };
        store.write(&p).unwrap();
        let err = store
            .promote("deadbeefdeadbeef", None, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("no trigger"), "got: {err}");
    }

    #[test]
    fn proposal_round_trips_through_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ProposalStore::open_at(tmp.path().to_path_buf());
        let mut state = empty_state();
        state.acceptance_results.push(failed_acceptance());
        store.propose_from_failures(&state).unwrap();
        let loaded = &store.list().unwrap()[0];
        assert_eq!(loaded.kind, "guardrail");
        assert_eq!(loaded.status, ProposalStatus::Proposed);
        assert!(loaded.body.contains("Guardrail"));
    }

    #[test]
    fn reject_marks_status_and_stops_reproposal() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ProposalStore::open_at(tmp.path().to_path_buf());
        let mut state = empty_state();
        state.acceptance_results.push(failed_acceptance());
        store.propose_from_failures(&state).unwrap();
        let fp = store.list().unwrap()[0].fingerprint.clone();
        store.reject(&fp).unwrap();

        // Same failure again must NOT resurrect a proposed candidate.
        let n = store.propose_from_failures(&state).unwrap();
        assert_eq!(n, 0);
        let after = store.load(&fp).unwrap();
        assert_eq!(after.status, ProposalStatus::Rejected);
    }

    #[test]
    fn suggest_trigger_skips_generic_tokens() {
        assert_eq!(
            suggest_trigger("error: test build failed running cargo"),
            ""
        );
        let t = suggest_trigger("unresolved import shared/types/index.d.ts");
        assert!(t.contains("shared/types/index.d.ts"), "got: {t}");
    }

    fn state_from_plan(plan: &crate::config::Plan) -> RunState {
        let projects = crate::config::ProjectsConfig {
            version: 1,
            defaults: Default::default(),
            projects: Default::default(),
        };
        RunState::new("run-1".into(), plan, &projects, 1, std::env::temp_dir())
    }

    #[test]
    fn synthesizes_skill_for_multiproject_run() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ProposalStore::open_at(tmp.path().to_path_buf());
        let plan: crate::config::Plan = serde_yaml::from_str(
            "spec: add email across api and web\n\
             tasks:\n  \
             - id: T_api\n    project: api\n    prompt: add email to the user model\n  \
             - id: T_web\n    project: web\n    prompt: render the email field\n",
        )
        .unwrap();
        let state = state_from_plan(&plan);
        assert_eq!(store.synthesize_skill_proposal(&state, &plan).unwrap(), 1);
        let p = &store.list().unwrap()[0];
        assert_eq!(p.kind, "skill");
        assert!(
            p.trigger.is_empty(),
            "trigger is authored by a human at promote"
        );
        assert!(p.body.contains("Playbook"));
        assert!(p.description.contains("confidence"));

        // The same run-shape recurs instead of creating a duplicate.
        let mut again = state_from_plan(&plan);
        again.run_id = "run-2".into();
        assert_eq!(store.synthesize_skill_proposal(&again, &plan).unwrap(), 0);
        assert_eq!(store.list().unwrap()[0].occurrences, 2);
    }

    #[test]
    fn skips_synthesis_for_trivial_run() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ProposalStore::open_at(tmp.path().to_path_buf());
        let plan: crate::config::Plan = serde_yaml::from_str(
            "spec: tiny\ntasks:\n  - id: T1\n    project: api\n    prompt: do one small thing\n",
        )
        .unwrap();
        let state = state_from_plan(&plan);
        assert_eq!(store.synthesize_skill_proposal(&state, &plan).unwrap(), 0);
    }

    #[test]
    fn skips_synthesis_for_verify_only_multiproject_run() {
        // F-LEARN-002: a verified multi-project run with ZERO agent tasks
        // (e.g. a pure audit of smoke checks) has no work shape to replay, so
        // it must NOT be synthesized into a "0-task run" playbook — even
        // though it's multi-project.
        let tmp = tempfile::tempdir().unwrap();
        let store = ProposalStore::open_at(tmp.path().to_path_buf());
        let plan: crate::config::Plan = serde_yaml::from_str(
            "spec: audit\ntasks:\n\
             \x20 - id: T_a\n    project: api\n    kind: verify\n    command: \"true\"\n\
             \x20 - id: T_b\n    project: web\n    kind: verify\n    command: \"true\"\n\
             \x20 - id: T_c\n    project: cli\n    kind: verify\n    command: \"true\"\n",
        )
        .unwrap();
        let state = state_from_plan(&plan);
        assert_eq!(
            store.synthesize_skill_proposal(&state, &plan).unwrap(),
            0,
            "a verify-only run (0 agent tasks) must not produce a playbook"
        );
    }

    fn write_l2(mem: &MemoryStore, project: &str, file: &str, decision: &str) {
        let dir = mem.l2_root().join(project);
        std::fs::create_dir_all(&dir).unwrap();
        let content = format!(
            "# Decision · x\n\n### `T1` — Done\n{decision}\n\n## What to remember next time\n\n\
             _(Optional human edit) Capture the decision rationale here so future plans can avoid re-doing the analysis._\n",
        );
        std::fs::write(dir.join(file), content).unwrap();
    }

    #[test]
    fn clusters_near_duplicate_docs() {
        let docs = vec![
            (
                "a".to_string(),
                "add an email field to the user model and expose it in the api response"
                    .to_string(),
            ),
            (
                "b".to_string(),
                "add an email field to the user model and expose it in the api response body"
                    .to_string(),
            ),
            (
                "c".to_string(),
                "refactor the build pipeline to cache dependencies and speed up ci runs"
                    .to_string(),
            ),
        ];
        let clusters = cluster_near_duplicates(&docs, 0.5);
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].len(), 2);
    }

    #[test]
    fn curation_proposes_and_applies_dedup() {
        let mtmp = tempfile::tempdir().unwrap();
        let mem = MemoryStore::open_at(mtmp.path().to_path_buf()).unwrap();
        write_l2(
            &mem,
            "api",
            "2026-01-01-x-rA.md",
            "add an email field to the user model and expose it in the api response",
        );
        write_l2(
            &mem,
            "api",
            "2026-01-02-x-rB.md",
            "add an email field to the user model and expose it in the api response body",
        );
        write_l2(
            &mem,
            "api",
            "2026-01-03-x-rC.md",
            "refactor the build pipeline to cache dependencies and speed up ci runs",
        );

        let ptmp = tempfile::tempdir().unwrap();
        let store = ProposalStore::open_at(ptmp.path().to_path_buf());
        assert_eq!(store.propose_memory_curation(&mem).unwrap(), 1);
        let p = store
            .list()
            .unwrap()
            .into_iter()
            .find(|p| p.kind == "memory_curation")
            .unwrap();
        assert_eq!(p.payload.len(), 2);

        let kept = apply_memory_curation(&mem, &p).unwrap();
        assert!(
            kept.ends_with("2026-01-02-x-rB.md"),
            "keeps the newest, got {kept:?}"
        );
        assert!(
            !mem.l2_root().join("api/2026-01-01-x-rA.md").exists(),
            "older near-duplicate is tombstoned"
        );
        assert!(
            mem.l2_root().join("api/2026-01-03-x-rC.md").exists(),
            "the distinct record is untouched"
        );
    }

    #[test]
    fn curation_excludes_human_edited_records() {
        let mtmp = tempfile::tempdir().unwrap();
        let mem = MemoryStore::open_at(mtmp.path().to_path_buf()).unwrap();
        write_l2(
            &mem,
            "api",
            "2026-01-01-x-rA.md",
            "add an email field to the user model and expose it in the api response",
        );
        // B is human-edited (no placeholder) → pinned → excluded, so the only
        // eligible record is A and nothing clusters.
        let dir = mem.l2_root().join("api");
        std::fs::write(
            dir.join("2026-01-02-x-rB.md"),
            "# Decision\nadd an email field to the user model and expose it in the api response body\n\n## What to remember next time\nWe deliberately chose X over Y because of Z.\n",
        )
        .unwrap();
        let ptmp = tempfile::tempdir().unwrap();
        let store = ProposalStore::open_at(ptmp.path().to_path_buf());
        assert_eq!(store.propose_memory_curation(&mem).unwrap(), 0);
    }
}
