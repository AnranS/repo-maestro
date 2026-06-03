//! Pure leaf helpers split out of the executor: code-context assembly
//! (codegraph CLI + native scan + scope filtering) and review-verdict parsing.
//! These are stateless functions with no scheduler state — extracted so
//! `executor.rs` stays focused on the run loop. Pure move, no behavior change.

/// Walk up from `start` (max 6 levels) for a directory holding a `.codegraph`
/// index, so a monorepo subpackage finds the index at the repo root.
pub(crate) fn find_codegraph_root(start: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut cur = Some(start);
    for _ in 0..6 {
        let c = cur?;
        if c.join(".codegraph").is_dir() {
            return Some(c.to_path_buf());
        }
        cur = c.parent();
    }
    None
}

/// Build a "relevant code" prompt section from codegraph for `query`, scoped to
/// the repo owning `repo_path`. Returns `None` (silently) when there's no
/// codegraph index or the CLI isn't available — codegraph is an optional,
/// auto-detected enhancement, never a hard dependency.
pub(crate) fn codegraph_cli_context(
    repo_path: &std::path::Path,
    query: &str,
    keep_paths: &[String],
) -> Option<String> {
    let root = find_codegraph_root(repo_path)?;
    let q: String = query.chars().take(600).collect();
    let out = std::process::Command::new(crate::codegraph::codegraph_bin())
        .arg("context")
        .arg(&q)
        .arg("--path")
        .arg(&root)
        .arg("--max-nodes")
        .arg("40")
        .arg("--no-code")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let md = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if md.is_empty() {
        return None;
    }

    // Scope to the project's own subtree when it's a subpackage of a larger
    // (mono)repo: the codegraph index covers the whole repo, but an agent
    // working on `packages/web` shouldn't be fed `packages/api`'s symbols —
    // it bloats the prompt and invites cross-package edits. Declared contract
    // files are kept regardless so a consumer still sees its interface.
    let subtree = repo_path
        .strip_prefix(&root)
        .ok()
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
        .filter(|s| !s.is_empty() && s != ".")
        .map(|s| format!("{}/", s.trim_end_matches('/')));
    let scoped = match subtree {
        Some(prefix) => scope_context_markdown(&md, &prefix, keep_paths)?,
        None => md, // project is the repo root — nothing to scope
    };

    Some(format!(
        "## Relevant code (codegraph)\n\nGrounding from this project's code knowledge graph — \
         use it to locate the right symbols before changing things.\n\n{scoped}"
    ))
}

/// Zero-dependency grounding: the built-in native indexer, scoped to the
/// project subtree (walks `repo_path` directly), ranked by query relevance.
/// Used when no external codegraph index exists so agents are never flying
/// blind on the zero-config default.
pub(crate) fn native_code_context(repo_path: &std::path::Path, query: &str) -> Option<String> {
    let hits = crate::codegraph::native_context(repo_path, query, 12);
    if hits.is_empty() {
        return None;
    }
    let mut body = String::new();
    for (path, syms) in &hits {
        let names: Vec<&str> = syms.iter().take(8).map(|s| s.name.as_str()).collect();
        if names.is_empty() {
            body.push_str(&format!("- `{path}`\n"));
        } else {
            body.push_str(&format!("- `{path}`: {}\n", names.join(", ")));
        }
    }
    Some(format!(
        "## Relevant code (native scan)\n\nA quick local scan of this project's files most \
         relevant to the task — locate the right symbols before changing things.\n\n{}",
        body.trim_end()
    ))
}

/// Keep only the code-context bullets that reference files under `prefix`
/// (the project's subtree, e.g. `packages/web/`) or one of `keep_paths`
/// (declared contract files). Section headers and the query line are always
/// kept; a bullet's indented continuation lines follow their bullet. Returns
/// `None` if nothing in scope survived (don't inject sibling-package noise).
pub(crate) fn scope_context_markdown(
    md: &str,
    prefix: &str,
    keep_paths: &[String],
) -> Option<String> {
    // Pull the relative file paths a line references. Tokens look like
    // `pkg/web/src/x.ts:12` (entry points) or `pkg/web/src/x.ts:` (related
    // symbols); we take the part before the first `:` and require it to be an
    // actual relpath (contains `/`), ignoring symbol names and line numbers.
    let line_paths = |line: &str| -> Vec<String> {
        line.split([' ', ',', '`', '*', '(', ')'])
            .filter_map(|tok| {
                let path = tok.split(':').next().unwrap_or("");
                (path.contains('/') && !path.contains(".maestro/")).then(|| path.to_string())
            })
            .collect()
    };
    let in_scope = |line: &str| -> bool {
        line_paths(line)
            .iter()
            .any(|p| p.starts_with(prefix) || keep_paths.iter().any(|k| !k.is_empty() && p == k))
    };
    let mut out: Vec<&str> = Vec::new();
    let mut kept_any_bullet = false;
    let mut keep_continuation = false;
    for line in md.lines() {
        let trimmed = line.trim_start();
        let is_bullet = trimmed.starts_with("- ");
        let is_indented_cont = line.starts_with(char::is_whitespace) && !trimmed.is_empty();
        if is_bullet && line.starts_with("- ") {
            // Top-level bullet: decide by scope.
            keep_continuation = in_scope(line);
            if keep_continuation {
                out.push(line);
                kept_any_bullet = true;
            }
        } else if is_indented_cont || (is_bullet && !line.starts_with("- ")) {
            // Continuation (signature, nested bullet) — follow the parent.
            if keep_continuation {
                out.push(line);
            }
        } else {
            // Header, blank line, or prose — always keep, resets bullet scope.
            out.push(line);
            keep_continuation = false;
        }
    }
    if !kept_any_bullet {
        return None;
    }
    // Collapse the runs of blank lines the filtering may have opened up.
    let mut compact: Vec<&str> = Vec::with_capacity(out.len());
    let mut prev_blank = false;
    for line in out {
        let blank = line.trim().is_empty();
        if blank && prev_blank {
            continue;
        }
        prev_blank = blank;
        compact.push(line);
    }
    Some(compact.join("\n").trim().to_string())
}

/// Parse a reviewer agent's reply for its verdict. Scans from the end (the
/// verdict line should be last) for `VERDICT: pass|fail`. Anything else —
/// including no verdict at all — counts as a pass, so a reviewer that ignores
/// the format never wrongly blocks a task. Returns `true` for pass.
/// Normalize an error message to a stable signature for "is this the same
/// failure as last time?" — lowercased, first few lines only, digits dropped
/// (timestamps, counts, line numbers vary run-to-run), whitespace collapsed.
pub(crate) fn error_signature(msg: &str) -> String {
    let core: String = msg
        .lines()
        .take(8)
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
        .chars()
        .filter(|c| !c.is_ascii_digit())
        .collect();
    core.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn parse_verdict(text: &str) -> bool {
    for line in text.lines().rev() {
        let lower = line.trim().to_lowercase();
        if let Some(rest) = lower.strip_prefix("verdict:") {
            let v = rest.trim();
            if v.starts_with("fail") {
                return false;
            }
            if v.starts_with("pass") {
                return true;
            }
        }
    }
    true
}
