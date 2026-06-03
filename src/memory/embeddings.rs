//! Local semantic embedding for memory recall (opt-in, `--features embeddings`).
//!
//! Uses fastembed (ONNX, BGE-small) entirely on-device — the model downloads
//! once on first use and is then cached locally, preserving maestro's
//! local-first stance. Embeddings are cached in-process by content hash so
//! repeated searches only embed new/changed chunks. Every entry point returns
//! `None` on any failure (no model, offline, etc.) so the caller transparently
//! falls back to TF-IDF.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

use super::retrieval::{Chunk, Hit};

static MODEL: OnceLock<Option<TextEmbedding>> = OnceLock::new();
static CACHE: OnceLock<Mutex<HashMap<u64, Vec<f32>>>> = OnceLock::new();

/// On-disk vector cache so embeddings survive restarts (only changed chunks
/// get re-embedded). Keyed by content hash; best-effort (errors ignored).
fn cache_path() -> Option<std::path::PathBuf> {
    Some(
        crate::paths::maestro_dir()
            .ok()?
            .join("memory")
            .join("embeddings-cache.json"),
    )
}

fn cache() -> &'static Mutex<HashMap<u64, Vec<f32>>> {
    CACHE.get_or_init(|| {
        let loaded = cache_path()
            .and_then(|p| std::fs::read(&p).ok())
            .and_then(|b| serde_json::from_slice::<HashMap<u64, Vec<f32>>>(&b).ok())
            .unwrap_or_default();
        Mutex::new(loaded)
    })
}

fn persist_cache(map: &HashMap<u64, Vec<f32>>) {
    if let Some(p) = cache_path() {
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(bytes) = serde_json::to_vec(map) {
            let _ = std::fs::write(&p, bytes);
        }
    }
}

fn model() -> Option<&'static TextEmbedding> {
    MODEL
        .get_or_init(|| {
            TextEmbedding::try_new(
                InitOptions::new(EmbeddingModel::BGESmallENV15).with_show_download_progress(false),
            )
            .map_err(|e| {
                tracing::warn!("embeddings disabled — model unavailable: {e:#}");
                e
            })
            .ok()
        })
        .as_ref()
}

fn content_hash(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let (mut dot, mut na, mut nb) = (0.0f32, 0.0f32, 0.0f32);
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

/// Rank `chunks` against `query` by cosine similarity of local embeddings.
/// `None` ⇒ embeddings unavailable; caller should fall back to TF-IDF.
pub fn semantic_rank(chunks: &[Chunk], query: &str, k: usize) -> Option<Vec<Hit>> {
    let model = model()?;
    let query_vec = model
        .embed(vec![query.to_string()], None)
        .ok()?
        .into_iter()
        .next()?;

    let cache = cache();
    let keys: Vec<u64> = chunks.iter().map(|c| content_hash(&c.content)).collect();

    // Embed only the chunks we haven't seen before.
    let missing: Vec<(usize, String)> = {
        let c = cache.lock().ok()?;
        keys.iter()
            .enumerate()
            .filter(|(_, key)| !c.contains_key(*key))
            .map(|(i, _)| (i, chunks[i].content.clone()))
            .collect()
    };
    if !missing.is_empty() {
        let texts: Vec<String> = missing.iter().map(|(_, t)| t.clone()).collect();
        let embs = model.embed(texts, None).ok()?;
        let mut c = cache.lock().ok()?;
        for ((i, _), emb) in missing.iter().zip(embs) {
            c.insert(keys[*i], emb);
        }
        // Drop entries no longer in the live corpus: edited/deleted content
        // leaves a stale hash, so without this the on-disk cache grows forever.
        // Safe because callers pass the full corpus (build_index), not a
        // query-specific subset.
        let live: std::collections::HashSet<u64> = keys.iter().copied().collect();
        c.retain(|key, _| live.contains(key));
        persist_cache(&c);
    }

    let mut scored: Vec<Hit> = {
        let c = cache.lock().ok()?;
        chunks
            .iter()
            .enumerate()
            .filter_map(|(i, ch)| {
                c.get(&keys[i]).map(|emb| Hit {
                    chunk: ch.clone(),
                    score: cosine(&query_vec, emb),
                })
            })
            .collect()
    };
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(k);
    Some(scored)
}
