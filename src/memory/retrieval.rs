//! Lexical (TF-IDF) retrieval over `.maestro/memory/{l1_facts,l2_decisions}/**`
//! plus failed-acceptance reports under `.maestro/runs/*/REPLAN.md`.
//!
//! ## Why hand-rolled
//!
//! Our corpus is tiny — typically a few dozen to a few hundred markdown
//! files. The cost of a real search engine (tantivy: hundreds of KB of
//! deps + extra compile time, sqlite-vec: native extension + embedding
//! pipeline) would dwarf the benefit. TF-IDF with cosine similarity over
//! a hash-map index is ~150 lines, zero new deps, and answers a typical
//! query in single-digit milliseconds.
//!
//! ## What we index
//!
//!   - Every file under `memory/l1_facts/<topic>/*` — chunked per file
//!   - Every file under `memory/l2_decisions/<project>/*` — chunked per file
//!   - Every `REPLAN.md` under `runs/*/` — these are useful for explicit
//!     replan/retry flows, but normal task-context retrieval filters them out
//!     so stale failures do not pollute unrelated downstream prompts.
//!
//! Each indexed unit is a [`Chunk`] with a stable id (relative path),
//! source kind (so the prompt can label slices `[l1] api/openapi.yaml`
//! vs `[replan] 20260516-…`), and raw text.
//!
//! ## What we DON'T do
//!
//!   - Embedding-based semantic retrieval. Add later if lexical proves
//!     insufficient; the public API takes a query string so the caller
//!     doesn't care which scoring method is in use.
//!   - Persistent on-disk index. The full index for ~500 files takes
//!     <50 ms to build on cold start. We re-build per query; that's
//!     cheaper than maintaining cache invalidation against arbitrary
//!     external edits.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::adapter::MemorySlice;
use crate::paths;

/// One indexed unit. Maps roughly to a single markdown file.
#[derive(Debug, Clone)]
pub struct Chunk {
    /// Workspace-relative identifier, e.g. `l1/api/openapi.yaml` or
    /// `replan/20260516-220355_abc123`. Stable across runs so the same
    /// chunk hits the same address.
    pub id: String,
    /// Coarse origin so the prompt-renderer can prefix differently.
    pub source: ChunkSource,
    /// On-disk path. `None` if the chunk is synthetic.
    pub path: Option<PathBuf>,
    /// Raw text. We index this, we also surface it on retrieval.
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkSource {
    L1Fact,
    L2Decision,
    Replan,
}

impl ChunkSource {
    fn label(&self) -> &'static str {
        match self {
            ChunkSource::L1Fact => "l1",
            ChunkSource::L2Decision => "l2",
            ChunkSource::Replan => "replan",
        }
    }
}

/// A scored retrieval result.
#[derive(Debug, Clone)]
pub struct Hit {
    pub chunk: Chunk,
    pub score: f32,
}

impl Hit {
    /// Convert back to a [`MemorySlice`] for prompt injection. Prefixes
    /// the topic with the source kind so the agent sees `l2/server/...`
    /// vs `l1/api/...` vs `replan/...` and can weigh them differently.
    pub fn into_slice(self) -> MemorySlice {
        MemorySlice {
            topic: format!("{}/{}", self.chunk.source.label(), self.chunk.id),
            content: self.chunk.content,
        }
    }
}

/// Build a retrieval index by scanning the current workspace's
/// `.maestro/memory/` and `.maestro/runs/*/REPLAN.md`. Errors propagate
/// only on fundamentally broken filesystem state — individual unreadable
/// files are skipped with a warning.
pub fn build_index() -> Result<Vec<Chunk>> {
    let root = paths::maestro_dir().context("resolve .maestro/ for index")?;
    let mut chunks = Vec::new();

    // L1 facts: memory/l1_facts/<topic>/<file>
    let l1_root = root.join("memory").join(super::L1_DIR);
    if l1_root.is_dir() {
        collect_under(&l1_root, &l1_root, ChunkSource::L1Fact, &mut chunks)?;
    }

    // L2 decisions: memory/l2_decisions/<project>/<file>
    let l2_root = root.join("memory").join(super::L2_DIR);
    if l2_root.is_dir() {
        collect_under(&l2_root, &l2_root, ChunkSource::L2Decision, &mut chunks)?;
    }

    // Replan prompts: runs/<id>/REPLAN.md — these encode previously-
    // failed acceptance checks plus the planner's marching orders.
    // Indexed without recursion so we don't accidentally drag in
    // unrelated task logs.
    let runs_root = root.join("runs");
    if runs_root.is_dir() {
        for entry in std::fs::read_dir(&runs_root)? {
            let entry = entry?;
            let p = entry.path();
            if !p.is_dir() {
                continue;
            }
            // skip the `current` symlink — we'd double-count whatever it
            // points at.
            if entry.file_name() == "current" {
                continue;
            }
            let replan = p.join("REPLAN.md");
            if replan.is_file() {
                let id = entry.file_name().to_string_lossy().to_string();
                if let Ok(text) = std::fs::read_to_string(&replan) {
                    chunks.push(Chunk {
                        id,
                        source: ChunkSource::Replan,
                        path: Some(replan),
                        content: text,
                    });
                }
            }
        }
    }

    Ok(chunks)
}

fn collect_under(base: &Path, dir: &Path, source: ChunkSource, out: &mut Vec<Chunk>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let p = entry.path();
        if p.is_dir() {
            collect_under(base, &p, source, out)?;
        } else if p.is_file() {
            let name = p
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            if name.starts_with('.') {
                continue;
            }
            // Skip binary-ish files. We index markdown / yaml / json /
            // proto / txt — basically anything an agent could read.
            if !is_text_like(&name) {
                continue;
            }
            let id = p
                .strip_prefix(base)
                .unwrap_or(&p)
                .to_string_lossy()
                .to_string();
            match std::fs::read_to_string(&p) {
                Ok(text) => out.push(Chunk {
                    id,
                    source,
                    path: Some(p),
                    content: text,
                }),
                Err(e) => tracing::warn!("skip {:?} in retrieval index: {e:#}", p),
            }
        }
    }
    Ok(())
}

fn is_text_like(name: &str) -> bool {
    let lower = name.to_lowercase();
    [
        ".md",
        ".markdown",
        ".txt",
        ".yaml",
        ".yml",
        ".json",
        ".toml",
        ".proto",
        ".graphql",
        ".sql",
        ".rst",
    ]
    .iter()
    .any(|ext| lower.ends_with(ext))
}

/// Score every chunk against `query` with TF-IDF + cosine similarity and
/// return the top `k` non-zero matches. Returns at most `k` results;
/// callers can further trim by length budget downstream.
pub fn retrieve(chunks: &[Chunk], query: &str, k: usize) -> Vec<Hit> {
    if chunks.is_empty() || query.trim().is_empty() || k == 0 {
        return vec![];
    }

    // Document frequencies — counted across the full corpus once.
    let docs_tokens: Vec<Vec<String>> = chunks.iter().map(|c| tokenize(&c.content)).collect();
    let n_docs = docs_tokens.len() as f32;

    let mut df: HashMap<String, u32> = HashMap::new();
    for tokens in &docs_tokens {
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for t in tokens {
            if seen.insert(t.as_str()) {
                *df.entry(t.clone()).or_insert(0) += 1;
            }
        }
    }

    // IDF as ln((N+1)/(df+1)) + 1 — smoothed so unseen terms don't
    // explode and singleton matches in tiny corpora still score.
    let idf = |term: &str| -> f32 {
        let df = *df.get(term).unwrap_or(&0) as f32;
        ((n_docs + 1.0) / (df + 1.0)).ln() + 1.0
    };

    // Build the query vector.
    let q_tokens = tokenize(query);
    let mut q_vec: HashMap<String, f32> = HashMap::new();
    for t in &q_tokens {
        *q_vec.entry(t.clone()).or_insert(0.0) += 1.0;
    }
    for (t, tf) in q_vec.iter_mut() {
        *tf *= idf(t);
    }
    let q_norm = vec_norm(&q_vec);
    if q_norm == 0.0 {
        return vec![];
    }

    // Score each document.
    let mut hits: Vec<Hit> = Vec::with_capacity(chunks.len());
    for (i, chunk) in chunks.iter().enumerate() {
        let tokens = &docs_tokens[i];
        if tokens.is_empty() {
            continue;
        }
        let mut doc_vec: HashMap<String, f32> = HashMap::new();
        for t in tokens {
            *doc_vec.entry(t.clone()).or_insert(0.0) += 1.0;
        }
        for (t, tf) in doc_vec.iter_mut() {
            *tf *= idf(t);
        }
        let d_norm = vec_norm(&doc_vec);
        if d_norm == 0.0 {
            continue;
        }
        // Dot product over the smaller of the two vectors (query is
        // usually short — iterate it).
        let mut dot = 0.0_f32;
        for (t, qw) in &q_vec {
            if let Some(dw) = doc_vec.get(t) {
                dot += qw * dw;
            }
        }
        if dot == 0.0 {
            continue;
        }
        let score = dot / (q_norm * d_norm);
        hits.push(Hit {
            chunk: chunk.clone(),
            score,
        });
    }

    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hits.truncate(k);
    hits
}

/// Retrieve context for a normal workflow task.
///
/// Replan prompts are indexed for dedicated replan/retry flows, but they are
/// too noisy for routine task dispatch: a failed first attempt can otherwise
/// be injected into later tasks just because it shares domain terms. Workflow
/// inputs/outputs carry deterministic run-local context; retrieval should add
/// stable L1/L2 knowledge around that, not stale failure transcripts.
pub fn retrieve_task_context(
    chunks: &[Chunk],
    query: &str,
    k: usize,
    relevant_projects: &[String],
) -> Vec<Hit> {
    let task_chunks: Vec<Chunk> = chunks
        .iter()
        .filter(|chunk| chunk.source != ChunkSource::Replan)
        .filter(|chunk| {
            // L2 decisions are project-scoped (id = `<project>/<file>`). When the
            // caller passes a relevant-project set (the task's project + its
            // dependencies), keep only L2 from those — a task shouldn't pull an
            // unrelated project's decisions just because the prompt shares words.
            // L1 facts are topic-based, not project-scoped, so always eligible.
            if relevant_projects.is_empty() || chunk.source != ChunkSource::L2Decision {
                return true;
            }
            let proj = chunk.id.split('/').next().unwrap_or("");
            relevant_projects.iter().any(|p| p == proj)
        })
        .cloned()
        .collect();
    retrieve(&task_chunks, query, k)
}

/// Trim a hit list to fit inside a total-content byte budget. Drops the
/// lowest-scoring hits first. Each hit's content is also truncated to
/// `per_hit_max` bytes to keep any single oversize slice from blowing
/// the budget.
pub fn fit_budget(mut hits: Vec<Hit>, total_budget: usize, per_hit_max: usize) -> Vec<Hit> {
    if hits.is_empty() {
        return hits;
    }
    // Truncate per-hit first so the budget calc is realistic.
    for h in &mut hits {
        if h.chunk.content.len() > per_hit_max {
            let mut cut = per_hit_max;
            while cut < h.chunk.content.len() && !h.chunk.content.is_char_boundary(cut) {
                cut += 1;
            }
            let mut t = h.chunk.content[..cut].to_string();
            t.push_str("\n… (truncated)");
            h.chunk.content = t;
        }
    }
    let mut total = 0usize;
    let mut out = Vec::new();
    for h in hits {
        let sz = h.chunk.content.len();
        if total + sz > total_budget && !out.is_empty() {
            break;
        }
        total += sz;
        out.push(h);
    }
    out
}

fn tokenize(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(text.len() / 6);
    let mut buf = String::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            for c in ch.to_lowercase() {
                buf.push(c);
            }
        } else if !buf.is_empty() {
            if !is_stopword(&buf) && buf.len() >= 2 {
                out.push(std::mem::take(&mut buf));
            } else {
                buf.clear();
            }
        }
    }
    if !buf.is_empty() && !is_stopword(&buf) && buf.len() >= 2 {
        out.push(buf);
    }
    out
}

fn vec_norm(v: &HashMap<String, f32>) -> f32 {
    v.values().map(|x| x * x).sum::<f32>().sqrt()
}

/// English / code-comment stopwords. Conservative list — better to leave
/// a generic token in than to strip something the user actually queried
/// for. Lowercase only; tokenizer lowercases before checking.
fn is_stopword(t: &str) -> bool {
    matches!(
        t,
        "the"
            | "a"
            | "an"
            | "and"
            | "or"
            | "but"
            | "if"
            | "is"
            | "are"
            | "was"
            | "were"
            | "be"
            | "to"
            | "of"
            | "in"
            | "on"
            | "at"
            | "by"
            | "for"
            | "with"
            | "as"
            | "from"
            | "this"
            | "that"
            | "it"
            | "its"
            | "we"
            | "you"
            | "i"
            | "do"
            | "does"
            | "use"
            | "using"
            | "new"
            | "see"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(id: &str, source: ChunkSource, content: &str) -> Chunk {
        Chunk {
            id: id.into(),
            source,
            path: None,
            content: content.into(),
        }
    }

    #[test]
    fn tokenize_splits_on_non_alnum_and_lowercases() {
        let t = tokenize("Hello, World! foo_bar 123");
        assert_eq!(t, vec!["hello", "world", "foo_bar", "123"]);
    }

    #[test]
    fn tokenize_drops_stopwords_and_singletons() {
        let t = tokenize("the quick brown fox");
        assert_eq!(t, vec!["quick", "brown", "fox"]);
    }

    #[test]
    fn retrieve_returns_most_similar_first() {
        let chunks = vec![
            chunk(
                "a",
                ChunkSource::L1Fact,
                "Login flow uses JWT tokens stored in localStorage.",
            ),
            chunk(
                "b",
                ChunkSource::L1Fact,
                "Database migrations are managed via Alembic.",
            ),
            chunk(
                "c",
                ChunkSource::L2Decision,
                "We decided to switch login from cookies to JWT for stateless auth.",
            ),
        ];
        let hits = retrieve(&chunks, "how does login auth work", 3);
        assert!(!hits.is_empty());
        // Both A and C are about login; both should beat B.
        let ids: Vec<&str> = hits.iter().map(|h| h.chunk.id.as_str()).collect();
        assert!(ids[0] == "a" || ids[0] == "c", "got {ids:?}");
        let last_or_missing = ids.last();
        // B may be filtered entirely (zero overlap) — that's also fine.
        if let Some(&last) = last_or_missing {
            if ids.len() == 3 {
                assert_eq!(last, "b");
            }
        }
    }

    #[test]
    fn retrieve_handles_empty_inputs() {
        assert!(retrieve(&[], "anything", 5).is_empty());
        let chunks = vec![chunk("a", ChunkSource::L1Fact, "stuff")];
        assert!(retrieve(&chunks, "", 5).is_empty());
        assert!(retrieve(&chunks, "stuff", 0).is_empty());
    }

    #[test]
    fn retrieve_caps_at_k() {
        let chunks: Vec<Chunk> = (0..10)
            .map(|i| chunk(&format!("d{i}"), ChunkSource::L1Fact, "common word here"))
            .collect();
        let hits = retrieve(&chunks, "common word", 3);
        assert!(hits.len() <= 3);
    }

    #[test]
    fn task_context_retrieval_excludes_replan_chunks() {
        let chunks = vec![
            chunk(
                "20260522-abc",
                ChunkSource::Replan,
                "calculator safeDivide quotient failure failure failure",
            ),
            chunk(
                "conventions/calculator.md",
                ChunkSource::L1Fact,
                "calculator safeDivide returns null for zero divisor",
            ),
        ];
        let hits = retrieve_task_context(&chunks, "fix calculator safeDivide failure", 5, &[]);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].chunk.source, ChunkSource::L1Fact);
    }

    #[test]
    fn retrieve_task_context_scopes_l2_to_relevant_projects() {
        let chunks = vec![
            chunk(
                "api/2026-05-20-x.md",
                ChunkSource::L2Decision,
                "user email field added",
            ),
            chunk(
                "billing/2026-05-20-y.md",
                ChunkSource::L2Decision,
                "user email invoice change",
            ),
            chunk(
                "conventions/x.md",
                ChunkSource::L1Fact,
                "user email convention",
            ),
        ];
        // Scope to `api` (+ no deps): the billing L2 must be excluded even though
        // it matches the query; the api L2 and the (project-agnostic) L1 stay.
        let hits = retrieve_task_context(&chunks, "user email", 5, &["api".to_string()]);
        let ids: Vec<&str> = hits.iter().map(|h| h.chunk.id.as_str()).collect();
        assert!(ids.iter().any(|i| i.starts_with("api/")));
        assert!(ids.iter().any(|i| i.starts_with("conventions/")));
        assert!(
            !ids.iter().any(|i| i.starts_with("billing/")),
            "got: {ids:?}"
        );
        // No scope → billing L2 is eligible again.
        let unscoped = retrieve_task_context(&chunks, "user email", 5, &[]);
        assert!(unscoped.iter().any(|h| h.chunk.id.starts_with("billing/")));
    }

    #[test]
    fn into_slice_prefixes_topic_with_source() {
        let h = Hit {
            chunk: chunk("api/openapi.yaml", ChunkSource::L1Fact, "x"),
            score: 1.0,
        };
        let s = h.into_slice();
        assert!(s.topic.starts_with("l1/"));
        assert!(s.topic.ends_with("openapi.yaml"));
    }

    #[test]
    fn fit_budget_drops_low_scoring_hits() {
        let hits = vec![
            Hit {
                chunk: chunk("a", ChunkSource::L1Fact, "x".repeat(800).as_str()),
                score: 1.0,
            },
            Hit {
                chunk: chunk("b", ChunkSource::L1Fact, "y".repeat(800).as_str()),
                score: 0.5,
            },
            Hit {
                chunk: chunk("c", ChunkSource::L1Fact, "z".repeat(800).as_str()),
                score: 0.1,
            },
        ];
        let kept = fit_budget(hits, 1500, 2000);
        // First fits (800), second pushes total to 1600 > 1500 → stop.
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].chunk.id, "a");
    }

    #[test]
    fn fit_budget_per_hit_truncation() {
        let hits = vec![Hit {
            chunk: chunk("big", ChunkSource::L1Fact, "x".repeat(5000).as_str()),
            score: 1.0,
        }];
        let kept = fit_budget(hits, 10_000, 1000);
        assert_eq!(kept.len(), 1);
        assert!(kept[0].chunk.content.contains("truncated"));
        // 1000 bytes of payload + the suffix marker
        assert!(kept[0].chunk.content.len() < 1100);
    }
}
