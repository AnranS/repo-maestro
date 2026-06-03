use super::*;
use std::path::{Path, PathBuf};

// F-103 provider detection — wider than detect_contract per design §3.1.
// Marker dir leaf names (case-insensitive).
pub(super) const F103_PROVIDER_MARKER_DIRS: &[&str] = &[
    "idl",
    "idls",
    "proto",
    "protos",
    "openapi",
    "openapis",
    "thrift",
    "thrifts",
    "schema",
    "schemas",
    "contract",
    "contracts",
    "api-spec",
];

// Extensions that count as a contract file when found INSIDE a marker dir.
// Outside a marker dir, `detect_contract` already handles the at-root cases
// (openapi.yaml etc.) so we don't double up.
pub(super) const F103_CONTRACT_EXTS: &[&str] =
    &["proto", "thrift", "yaml", "yml", "json", "graphql"];

// Canonical contract filenames (lowercased). When one of these lives inside
// a marker dir we prefer it as the representative — it's the strongest
// signal the project intends this file to be THE published contract.
pub(super) const F103_CANONICAL_FILENAMES: &[&str] = &[
    "openapi.yaml",
    "openapi.yml",
    "openapi.json",
    "swagger.yaml",
    "swagger.yml",
    "swagger.json",
    "schema.graphql",
];

/// F-103 §3.1: walk up to project-root + 2 looking for a marker directory
/// containing at least one contract file, then return the project-relative
/// path of a representative contract FILE inside it (canonical filename
/// first, then shortest path, then lex). Returning a file path — not a
/// directory — matches the existing `contracts.provides` semantics that
/// downstream code (`scheduler::executor::consumed_contract_section`)
/// already relies on: it `read_to_string`s the resolved path.
///
/// Generated-client subtrees, empty marker dirs, and package-manifest
/// filenames stay filtered out.
pub(super) fn detect_provider_marker(project_root: &Path) -> Option<String> {
    use ignore::WalkBuilder;

    // (canonical_rank, dir_priority, file_depth, rel_path_string)
    let mut hits: Vec<(u8, u8, usize, String)> = Vec::new();
    let walker = WalkBuilder::new(project_root)
        .max_depth(Some(2))
        .follow_links(false)
        .filter_entry(|e| e.depth() == 0 || !should_skip_dir(e.path()))
        .build();
    for entry in walker.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Ok(rel) = path.strip_prefix(project_root) else {
            continue;
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let Some(leaf) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let leaf_lc = leaf.to_ascii_lowercase();
        if !F103_PROVIDER_MARKER_DIRS.contains(&leaf_lc.as_str()) {
            continue;
        }
        let rel_lc = rel.to_string_lossy().to_ascii_lowercase();
        if crate::codegraph::is_generated_client_path(&rel_lc) {
            continue;
        }
        let dir_priority = if matches!(leaf_lc.as_str(), "idl" | "idls") {
            0
        } else {
            1
        };
        collect_marker_contract_files(path, rel, dir_priority, &mut hits);
    }
    hits.sort();
    hits.into_iter().next().map(|(_, _, _, p)| p)
}

/// Walk inside a marker dir and append every contract file to `hits` as a
/// (canonical_rank, dir_priority, file_depth, project-relative path) tuple.
/// Sort order at the call site picks the representative.
pub(super) fn collect_marker_contract_files(
    marker_abs: &Path,
    marker_rel: &Path,
    dir_priority: u8,
    hits: &mut Vec<(u8, u8, usize, String)>,
) {
    use ignore::WalkBuilder;

    let walker = WalkBuilder::new(marker_abs)
        .follow_links(false)
        .filter_entry(|e| e.depth() == 0 || !should_skip_dir(e.path()))
        .build();
    for entry in walker.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Ok(rel_in_marker) = path.strip_prefix(marker_abs) else {
            continue;
        };
        // Skip files that live inside a generated-client subtree
        // (`contracts/bam-idl/...` etc. — those are consumer snapshots,
        // not publishing surfaces).
        let rel_in_marker_lc = rel_in_marker.to_string_lossy().to_ascii_lowercase();
        if crate::codegraph::is_generated_client_path(&rel_in_marker_lc) {
            continue;
        }
        let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
            continue;
        };
        let ext_lc = ext.to_ascii_lowercase();
        if !F103_CONTRACT_EXTS.contains(&ext_lc.as_str()) {
            continue;
        }
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let name_lc = name.to_ascii_lowercase();
        if matches!(
            name_lc.as_str(),
            "package.json"
                | "package-lock.json"
                | "jsconfig.json"
                | "composer.json"
                | "manifest.json"
                | "tsconfig.json"
        ) || name_lc.ends_with(".tsconfig.json")
        {
            continue;
        }
        let canonical_rank = F103_CANONICAL_FILENAMES
            .iter()
            .position(|c| *c == name_lc)
            .map(|p| p as u8)
            .unwrap_or(u8::MAX);
        let full_rel = marker_rel.join(rel_in_marker);
        let depth = full_rel.components().count();
        let rel_str = display_path(Some(full_rel));
        hits.push((canonical_rank, dir_priority, depth, rel_str));
    }
}

/// F-103 §3.2 consumer-name matching, tightened after dali's N1-2 review:
/// - Normalized exact match is always allowed.
/// - Substring (`contains`) is only allowed when the producer's normalized
///   name is long enough (>=7 chars) OR has multi-word evidence
///   (kebab/snake/camel). This stops `data` ⊂ `Metadata` /
///   `core` ⊂ `Score...` false positives that would silently feed
///   `wire_contract_dependencies`.
pub(super) fn token_matches_producer(token_norm: &str, pname: &str, pname_norm: &str) -> bool {
    if pname_norm.len() < 4 {
        return false;
    }
    if token_norm == pname_norm {
        return true;
    }
    let multi_word = pname.contains('-') || pname.contains('_') || has_camel_word_boundary(pname);
    if pname_norm.len() >= 7 || multi_word {
        return token_norm.contains(pname_norm);
    }
    false
}

pub(super) fn has_camel_word_boundary(name: &str) -> bool {
    let chars: Vec<char> = name.chars().collect();
    chars
        .windows(2)
        .any(|w| w[0].is_ascii_lowercase() && w[1].is_ascii_uppercase())
}

/// Walk `consumer.path` for a generated-client subdir whose immediate child
/// names a registered producer that already has `provides`. Returns the
/// chosen producer + evidence list, or None.
pub(super) fn find_generated_client_match(
    consumer: &DiscoveredProject,
    provider_index: &std::collections::BTreeMap<String, (PathBuf, String)>,
) -> Option<ProducerMatch> {
    use ignore::WalkBuilder;

    // Per-producer hit count + first evidence entry. We pick the heaviest
    // producer at the end so an ambiguous workspace (consumer pulls from
    // two producers under one bam-idl/) goes with the loudest signal.
    let mut hit_count: std::collections::BTreeMap<String, usize> =
        std::collections::BTreeMap::new();
    let mut first_evidence: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    // F-103 design §8.2: near-miss subdirs we saw inside a generated-client
    // dir but couldn't match to any registered producer. Surfaced behind
    // RUST_LOG=debug only — never in default stdout.
    let mut near_misses: Vec<String> = Vec::new();

    let walker = WalkBuilder::new(&consumer.path)
        .max_depth(Some(8))
        .follow_links(false)
        .filter_entry(|e| e.depth() == 0 || !should_skip_dir(e.path()))
        .build();
    for entry in walker.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Ok(rel) = path.strip_prefix(&consumer.path) else {
            continue;
        };
        let rel_lc = rel.to_string_lossy().to_ascii_lowercase();
        // Only descend INTO generated-client dirs to enumerate subdirs.
        // is_generated_client_path is component-aware (F-103 §3.1 fix).
        if !crate::codegraph::is_generated_client_path(&rel_lc) {
            continue;
        }
        // Inside a generated-client dir. Each immediate subdir name is a
        // candidate producer-name token.
        let Ok(children) = std::fs::read_dir(path) else {
            continue;
        };
        for child in children.flatten() {
            let child_path = child.path();
            if !child_path.is_dir() {
                continue;
            }
            let Some(subdir) = child_path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let token_lc = subdir.to_ascii_lowercase();
            // Normalised form: lowercase + strip every non-alphanumeric so a
            // PascalCase generated-client dir name like `BillingPortalService`
            // collapses to `billingportalservice` and a registered project
            // name like `billing-portal` collapses to `billingportal` —
            // then a simple `contains` lights up the match. This handles
            // both kebab/snake/Pascal naming conventions in one rule
            // without coupling to codegraph's path_contains_token (which
            // needs word boundaries and so misses PascalCase).
            let token_norm: String = token_lc
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect();
            let mut matched_this_subdir = false;
            for pname in provider_index.keys() {
                if pname == &consumer.name {
                    continue;
                }
                let pname_norm: String = pname
                    .to_ascii_lowercase()
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric())
                    .collect();
                if !token_matches_producer(&token_norm, pname, &pname_norm) {
                    continue;
                }
                matched_this_subdir = true;
                // N2: weight by in-tree file count under the matched
                // generated-client subdir, not by subdir count. A producer
                // whose snapshot is one large tree should outrank one with
                // many tiny shells.
                let weight = count_files_in_tree(&child_path).max(1);
                *hit_count.entry(pname.clone()).or_default() += weight;
                let evidence_line = child_path
                    .strip_prefix(&consumer.path)
                    .map(|p| display_path(Some(p.to_path_buf())))
                    .unwrap_or_else(|_| child_path.to_string_lossy().to_string());
                first_evidence
                    .entry(pname.clone())
                    .or_insert_with(|| vec![evidence_line]);
            }
            if !matched_this_subdir {
                near_misses.push(subdir.to_string());
            }
        }
    }
    if hit_count.is_empty() {
        if !near_misses.is_empty() {
            tracing::debug!(
                consumer = %consumer.name,
                near_misses = ?near_misses,
                "contract promotion: saw generated-client subdirs but no registered producer matched",
            );
        }
        return None;
    }
    // Pick heaviest (by file count); ties broken lexicographically by name
    // (smaller name wins) for determinism.
    let (winner, _) = hit_count
        .iter()
        .max_by(|(an, ac), (bn, bc)| ac.cmp(bc).then_with(|| bn.cmp(an)))
        .unwrap();
    let evidence = first_evidence.get(winner).cloned().unwrap_or_default();
    Some(ProducerMatch {
        producer: winner.clone(),
        evidence,
    })
}

/// Count regular files under `root`. Used to weight competing producers in
/// the consumer-promotion ambiguity rule (largest in-tree file count wins).
pub(super) fn count_files_in_tree(root: &Path) -> usize {
    use ignore::WalkBuilder;

    WalkBuilder::new(root)
        .follow_links(false)
        .filter_entry(|e| e.depth() == 0 || !should_skip_dir(e.path()))
        .build()
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .count()
}

pub(super) struct ProducerMatch {
    pub(super) producer: String,
    pub(super) evidence: Vec<String>,
}

pub(super) fn detect_contract(dir: &Path) -> Option<String> {
    let mut candidates = Vec::<PathBuf>::new();
    for rel in [
        "openapi.yaml",
        "openapi.yml",
        "openapi.json",
        "swagger.yaml",
        "swagger.yml",
        "swagger.json",
        "schema.graphql",
        "contract/openapi.yaml",
        "contract/openapi.yml",
        "contract/openapi.json",
        "contracts/openapi.yaml",
        "contracts/openapi.yml",
        "contracts/openapi.json",
    ] {
        let p = dir.join(rel);
        if p.is_file() {
            candidates.push(PathBuf::from(rel));
        }
    }
    for subdir in [
        "contract",
        "contracts",
        "schema",
        "schemas",
        "proto",
        "protos",
        "idl",
        "idls",
        "thrift",
    ] {
        let p = dir.join(subdir);
        if p.is_dir() {
            collect_contract_files(dir, &p, &mut candidates);
        }
    }
    candidates.sort();
    candidates.into_iter().next().map(|p| display_path(Some(p)))
}

pub(super) fn collect_contract_files(base: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
    // F-103 guard: never let a generated-client directory contribute
    // candidate files to the PROVIDER side. A consumer project may carry a
    // tree like `src/bam-idl/X/foo.proto` — that's a snapshot of someone
    // else's contract, not this project's publishing surface. The
    // component-aware check in codegraph::is_generated_client_path catches
    // both nested and root-level marker dirs.
    let rel_to_base = dir.strip_prefix(base).unwrap_or(dir);
    let rel_lc = rel_to_base.to_string_lossy().to_ascii_lowercase();
    if crate::codegraph::is_generated_client_path(&rel_lc) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_contract_files(base, &path, out);
            continue;
        }
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let lower = name.to_ascii_lowercase();
        // Package/tooling manifests live inside package dirs (incl. ones named
        // `proto`/`schema`) but are not API contracts — don't let the greedy
        // `.json` rule slurp them, or a grouping dir gets a phantom project.
        if matches!(
            lower.as_str(),
            "package.json"
                | "package-lock.json"
                | "jsconfig.json"
                | "composer.json"
                | "manifest.json"
                | "tsconfig.json"
        ) || lower.ends_with(".tsconfig.json")
        {
            continue;
        }
        let is_contract = lower.ends_with(".proto")
            || lower.ends_with(".thrift")
            || lower.ends_with(".graphql")
            || lower.ends_with(".graphqls")
            || lower.ends_with(".json")
            || lower.ends_with(".schema.json")
            || lower == "openapi.yaml"
            || lower == "openapi.yml"
            || lower == "openapi.json"
            || lower == "swagger.yaml"
            || lower == "swagger.yml"
            || lower == "swagger.json";
        if is_contract {
            out.push(path.strip_prefix(base).unwrap_or(&path).to_path_buf());
        }
    }
}
