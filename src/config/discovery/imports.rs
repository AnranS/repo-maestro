use super::*;
use std::path::{Path, PathBuf};

/// File extensions whose relative imports we scan for cross-project edges.
pub(super) const SOURCE_IMPORT_EXTS: &[&str] = &["ts", "tsx", "js", "jsx", "mjs", "cjs", "py"];

/// Bounded scan of a project's source for relative imports (JS/TS path imports
/// and Python `from .`/`..` relative imports), resolving each to an absolute,
/// lexically-cleaned path outside the file's own dir. Go and Rust resolve
/// cross-project deps through manifests (go.mod / Cargo.toml), which discovery
/// already reads, and don't use relative source imports idiomatically.
/// Capped so a huge monorepo subtree never blows up discovery time.
pub(super) fn scan_source_imports(project_dir: &Path, out: &mut Vec<PathBuf>) {
    const MAX_FILES: usize = 200;
    const MAX_BYTES: u64 = 96 * 1024;
    let skip_dir = |name: &str| {
        matches!(
            name,
            "node_modules"
                | ".git"
                | "dist"
                | "build"
                | "out"
                | "target"
                | "coverage"
                | ".maestro"
                | ".next"
                | ".turbo"
                | ".cache"
        )
    };
    let mut stack = vec![project_dir.to_path_buf()];
    let mut scanned = 0usize;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if scanned >= MAX_FILES {
                return;
            }
            let path = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !skip_dir(&name) {
                    stack.push(path);
                }
                continue;
            }
            let is_source = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| SOURCE_IMPORT_EXTS.contains(&e))
                .unwrap_or(false);
            if !is_source {
                continue;
            }
            if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > MAX_BYTES {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            scanned += 1;
            let file_dir = path.parent().unwrap_or(project_dir);
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if ext == "py" {
                out.extend(python_relative_imports(&text, file_dir));
            } else {
                for spec in relative_import_specifiers(&text) {
                    out.push(lexical_clean(&file_dir.join(&spec)));
                }
            }
        }
    }
}

/// Resolve Python relative imports (`from .x import …`, `from ..pkg.mod import
/// …`) to absolute filesystem targets. Leading-dot count = how far up the
/// package tree: one dot is the file's own package (its dir), two is the
/// parent, etc. The dotted module after the dots becomes path segments.
pub(super) fn python_relative_imports(src: &str, file_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for line in src.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("from ") else {
            continue;
        };
        let rest = rest.trim_start();
        if !rest.starts_with('.') {
            continue; // absolute import → resolved via packaging, not paths
        }
        let dots = rest.chars().take_while(|c| *c == '.').count();
        // The dotted module name, if any (skip the `import` keyword for `from .
        // import x`, which targets the current package itself).
        let module = rest[dots..]
            .split_whitespace()
            .next()
            .filter(|t| *t != "import")
            .unwrap_or("");
        let mut base = file_dir.to_path_buf();
        for _ in 0..dots.saturating_sub(1) {
            base = base.parent().map(Path::to_path_buf).unwrap_or(base);
        }
        for seg in module.split('.').filter(|s| !s.is_empty()) {
            base = base.join(seg);
        }
        out.push(lexical_clean(&base));
    }
    out
}

/// Extract relative module specifiers (those starting with `.`) from JS/TS
/// `import`/`export … from`, `require(...)`, and dynamic `import(...)`.
pub(super) fn relative_import_specifiers(src: &str) -> Vec<String> {
    let mut specs = Vec::new();
    for line in src.lines() {
        let line = line.trim();
        if line.starts_with("//") || line.starts_with('*') {
            continue;
        }
        // Only the cheap, common forms — quoted specifiers after `from`,
        // `require(`, or `import(`.
        for marker in ["from ", "require(", "import("] {
            let mut rest = line;
            while let Some(idx) = rest.find(marker) {
                rest = &rest[idx + marker.len()..];
                let rest_trim = rest.trim_start().trim_start_matches('(');
                if let Some(quote) = rest_trim.chars().next().filter(|c| *c == '\'' || *c == '"') {
                    if let Some(end) = rest_trim[1..].find(quote) {
                        let spec = &rest_trim[1..1 + end];
                        if spec.starts_with('.') {
                            specs.push(spec.to_string());
                        }
                    }
                }
            }
        }
    }
    specs
}
