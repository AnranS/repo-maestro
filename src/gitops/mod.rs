//! Thin wrapper around the `git` CLI for the bits the executor needs —
//! creating and tearing down per-task worktrees so that parallel tasks on
//! the same project never race on the working directory.
//!
//! We shell out instead of using `git2` because (a) it's a tiny surface,
//! (b) `git2` adds a heavy native dep, and (c) the user's `git` already
//! has their auth/config — using it side-steps a class of "works in tests
//! but not in real repos" bugs.

use anyhow::{Context, Result};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Is `dir` the root of (or contained within) a git working tree?
pub fn is_git_repo(dir: &Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("rev-parse")
        .arg("--is-inside-work-tree")
        .output()
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "true")
        .unwrap_or(false)
}

/// Whether the repo at `dir` has a remote named `remote`.
pub fn has_remote(dir: &Path, remote: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("remote")
        .output()
        .map(|o| {
            o.status.success()
                && String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .any(|l| l.trim() == remote)
        })
        .unwrap_or(false)
}

/// Push `branch` to `remote`, setting it as upstream. Needed before opening a
/// PR — the head branch must exist on the remote. Errors carry git's stderr.
pub fn push_branch(dir: &Path, remote: &str, branch: &str) -> Result<()> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["push", "--set-upstream", remote, branch])
        .output()
        .with_context(|| format!("git push {remote} {branch}"))?;
    if !out.status.success() {
        anyhow::bail!(
            "git push {remote} {branch} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Return the top-level git worktree directory containing `dir`.
pub fn worktree_root(dir: &Path) -> Option<PathBuf> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("rev-parse")
        .arg("--show-toplevel")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let root = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if root.is_empty() {
        None
    } else {
        Some(PathBuf::from(root))
    }
}

/// Return git's shared metadata directory for all worktrees of the same repo.
/// This is stable across the main checkout and linked worktrees, unlike
/// `--show-toplevel`, which returns each worktree's own root.
pub fn git_common_dir(dir: &Path) -> Option<PathBuf> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("rev-parse")
        .arg("--git-common-dir")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if raw.is_empty() {
        return None;
    }
    let path = PathBuf::from(raw);
    let path = if path.is_absolute() {
        path
    } else {
        worktree_root(dir)?.join(path)
    };
    Some(path.canonicalize().unwrap_or(path))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeContext {
    pub repo_root: PathBuf,
    pub project_relative_path: PathBuf,
}

/// Return the repo root and the project subpath to preserve when a project
/// lives inside a monorepo. Git worktrees must be created from the repo root,
/// but tasks should still execute from their configured project directory.
pub fn worktree_context(dir: &Path) -> Option<WorktreeContext> {
    let repo_root = worktree_root(dir)?;
    let repo_root = repo_root.canonicalize().ok()?;
    let project_dir = dir.canonicalize().ok()?;
    let project_relative_path = project_dir.strip_prefix(&repo_root).ok()?.to_path_buf();
    Some(WorktreeContext {
        repo_root,
        project_relative_path,
    })
}

/// Is `dir` itself the root of a git worktree?
pub fn is_git_worktree_root(dir: &Path) -> bool {
    let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    worktree_root(&dir)
        .and_then(|root| root.canonicalize().ok())
        .map(|root| root == dir)
        .unwrap_or(false)
}

/// Return files changed under the project `dir`, as **project-relative**
/// paths. Status-based so it catches untracked/staged/unstaged edits from
/// adapters that don't report artifacts. Delegates to
/// `changed_files_with_status` (one scoped implementation) and drops the
/// status code, deduping + sorting.
pub fn changed_files(dir: &Path) -> Result<Vec<String>> {
    let mut files = BTreeSet::new();
    for (_, path) in changed_files_with_status(dir)? {
        files.insert(path);
    }
    Ok(files.into_iter().collect())
}

/// Strip the project prefix from a repo-root-relative path, yielding a
/// project-relative one. Empty prefix = the path is already
/// project-relative. Returns `None` if the path is outside the project
/// subtree (guards against a stray path the pathspec scope didn't exclude).
fn strip_project_prefix(repo_rel: &str, project_prefix: &Path) -> Option<String> {
    if project_prefix.as_os_str().is_empty() {
        return Some(repo_rel.to_string());
    }
    Path::new(repo_rel)
        .strip_prefix(project_prefix)
        .ok()
        .map(|rel| rel.to_string_lossy().to_string())
}

fn parse_status_path(line: &str) -> Option<String> {
    if line.len() < 4 {
        return None;
    }
    let path = line.get(3..)?.trim();
    if path.is_empty() {
        return None;
    }
    let path = path
        .rsplit_once(" -> ")
        .map(|(_, to)| to)
        .unwrap_or(path)
        .trim();
    (!path.is_empty()).then(|| path.to_string())
}

/// Changed files with their porcelain status code (e.g. `A`, `M`, `D`, `R`),
/// normalized to a single char from the two-column XY, as **project-relative**
/// paths. Skips maestro's own artifacts the same way `changed_files` does.
///
/// F-108: a project may be a subdir of a larger repo (the monorepo layout).
/// `git status --porcelain` is repo-root-relative even with `-C <subdir>`,
/// while maestro's contract paths are project-relative. So we run status
/// from the repo root with a pathspec scoped to the project subtree (which
/// also excludes sibling projects' changes), then strip the project prefix
/// to land project-relative paths. When `dir` isn't inside a git repo we
/// fall back to treating it as the root.
pub fn changed_files_with_status(dir: &Path) -> Result<Vec<(char, String)>> {
    let (run_dir, project_rel) = match worktree_context(dir) {
        Some(ctx) => (ctx.repo_root, ctx.project_relative_path),
        None => (dir.to_path_buf(), PathBuf::new()),
    };
    let pathspec = if project_rel.as_os_str().is_empty() {
        ".".to_string()
    } else {
        project_rel.to_string_lossy().to_string()
    };
    let out = Command::new("git")
        .arg("-C")
        .arg(&run_dir)
        .arg("status")
        .arg("--porcelain=v1")
        .arg("--untracked-files=all")
        .arg("--")
        .arg(&pathspec)
        .output()
        .with_context(|| format!("spawn git status for {run_dir:?}"))?;
    if !out.status.success() {
        anyhow::bail!(
            "git status failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let mut files = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let Some(repo_rel) = parse_status_path(line) else {
            continue;
        };
        // git emits repo-root-relative paths; bring them to project-relative.
        let Some(path) = strip_project_prefix(&repo_rel, &project_rel) else {
            continue; // outside the project subtree
        };
        if path == ".maestro" || path.starts_with(".maestro/") || is_transient_artifact(&path) {
            continue;
        }
        // XY columns: '?' = untracked (treat as added); else first non-space.
        let code = line
            .chars()
            .take(2)
            .find(|c| !c.is_whitespace())
            .unwrap_or('?');
        let code = if code == '?' { 'A' } else { code };
        files.push((code, path));
    }
    Ok(files)
}

/// Human-readable (non-binary) diff of all `worktree` changes against HEAD, for
/// surfacing in an approval card. Stages into the index first so new/untracked
/// files appear too. Truncated to `max_bytes` so a huge diff can't blow the UI.
pub fn worktree_diff_text(worktree: &Path, max_bytes: usize) -> Result<String> {
    let files = changed_files(worktree)?;
    if files.is_empty() {
        return Ok(String::new());
    }
    let _ = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["reset", "-q", "HEAD", "--", "."])
        .output();
    let add = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .arg("add")
        .arg("-A")
        .arg("--")
        .args(&files)
        .output()
        .with_context(|| format!("spawn git add for {worktree:?}"))?;
    if !add.status.success() {
        anyhow::bail!(
            "git add failed: {}",
            String::from_utf8_lossy(&add.stderr).trim()
        );
    }
    let diff = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["diff", "--cached", "--stat", "HEAD"])
        .output()
        .with_context(|| format!("spawn git diff --stat for {worktree:?}"))?;
    let stat = String::from_utf8_lossy(&diff.stdout).to_string();
    let full = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["diff", "--cached", "HEAD"])
        .output()
        .with_context(|| format!("spawn git diff for {worktree:?}"))?;
    let body = String::from_utf8_lossy(&full.stdout);
    let mut text = if stat.trim().is_empty() {
        body.to_string()
    } else {
        format!("{}\n{}", stat.trim_end(), body)
    };
    if text.len() > max_bytes {
        text.truncate(max_bytes);
        text.push_str("\n… (diff truncated)\n");
    }
    Ok(text)
}

pub fn is_transient_artifact(path: &str) -> bool {
    path == ".DS_Store"
        || path.ends_with("/.DS_Store")
        || path.ends_with(".pyc")
        || path.contains("/__pycache__/")
        || path.starts_with("__pycache__/")
}

/// Create a new git worktree of `repo` checked out at `target` on a fresh
/// branch derived from the current HEAD. Returns the target path on
/// success.
///
/// `branch_name` should be globally unique (e.g.
/// `maestro/<run-id>/<task-id>`); git will refuse to create a worktree
/// that reuses an existing branch.
pub fn create_worktree(repo: &Path, target: &Path, branch_name: &str) -> Result<PathBuf> {
    create_worktree_from(repo, target, branch_name, None)
}

/// Create a new git worktree from an explicit start point. When `start_point`
/// is `None`, git uses the repo's current `HEAD`.
pub fn create_worktree_from(
    repo: &Path,
    target: &Path,
    branch_name: &str,
    start_point: Option<&str>,
) -> Result<PathBuf> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("mkdir {parent:?}"))?;
    }
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(repo)
        .arg("worktree")
        .arg("add")
        .arg("-b")
        .arg(branch_name)
        .arg(target);
    if let Some(start_point) = start_point {
        cmd.arg(start_point);
    }
    let out = cmd
        .output()
        .with_context(|| format!("spawn git worktree add for {repo:?}"))?;
    if !out.status.success() {
        anyhow::bail!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(target.to_path_buf())
}

/// Stage all working-tree changes in `worktree` and write a binary-safe patch
/// against `HEAD` to `patch_path`. Returns `false` when there is no diff.
pub fn write_index_patch(worktree: &Path, patch_path: &Path) -> Result<bool> {
    if let Some(parent) = patch_path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("mkdir {parent:?}"))?;
    }

    let files = changed_files(worktree)?;
    if files.is_empty() {
        return Ok(false);
    }

    let reset = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .arg("reset")
        .arg("-q")
        .arg("HEAD")
        .arg("--")
        .arg(".")
        .output()
        .with_context(|| format!("spawn git reset for {worktree:?}"))?;
    if !reset.status.success() {
        anyhow::bail!(
            "git reset failed: {}",
            String::from_utf8_lossy(&reset.stderr).trim()
        );
    }

    let add = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .arg("add")
        .arg("-A")
        .arg("--")
        .args(&files)
        .output()
        .with_context(|| format!("spawn git add for {worktree:?}"))?;
    if !add.status.success() {
        anyhow::bail!(
            "git add failed: {}",
            String::from_utf8_lossy(&add.stderr).trim()
        );
    }

    let diff = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .arg("diff")
        .arg("--binary")
        .arg("--cached")
        .arg("HEAD")
        .output()
        .with_context(|| format!("spawn git diff for {worktree:?}"))?;
    if !diff.status.success() {
        anyhow::bail!(
            "git diff failed: {}",
            String::from_utf8_lossy(&diff.stderr).trim()
        );
    }
    if diff.stdout.is_empty() {
        return Ok(false);
    }
    std::fs::write(patch_path, &diff.stdout)
        .with_context(|| format!("write {}", patch_path.display()))?;
    Ok(true)
}

/// A patch could not be integrated because it overlaps changes already
/// applied to the integration worktree (the classic monorepo case: two
/// parallel tasks edit the same region of a shared file). Carries the
/// conflicting files so the caller can attribute the clash to a prior task
/// and tell the user how to resolve it.
#[derive(Debug, Clone, thiserror::Error)]
#[error("patch conflicts with already-integrated changes in: {}", files.join(", "))]
pub struct PatchConflict {
    pub files: Vec<String>,
}

/// Files left unmerged in `worktree`'s index (conflict markers written).
fn unmerged_files(worktree: &Path) -> Vec<String> {
    Command::new("git")
        .arg("-C")
        .arg(worktree)
        .arg("diff")
        .arg("--name-only")
        .arg("--diff-filter=U")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Best-effort parse of the file paths `git apply` reported as failing, from
/// its stderr (`error: patch failed: <path>:N` / `error: <path>: patch does
/// not apply`).
fn parse_apply_failure_files(stderr: &str) -> Vec<String> {
    let mut files = BTreeSet::new();
    for line in stderr.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("error: patch failed: ") {
            let path = rest.rsplit_once(':').map(|(p, _)| p).unwrap_or(rest);
            files.insert(path.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("error: ") {
            if let Some(path) = rest.strip_suffix(": patch does not apply") {
                files.insert(path.trim().to_string());
            }
        }
    }
    files.into_iter().collect()
}

/// Apply a previously generated patch to `worktree`'s index and commit it.
/// Returns `false` when the patch file is empty.
///
/// Uses a 3-way merge so that parallel tasks editing *different* regions of
/// the same file integrate cleanly instead of failing on strict context
/// matching. A true overlap (same region edited twice) yields a
/// [`PatchConflict`] naming the files, and the worktree is restored to a
/// clean state so the run can continue / report precisely.
pub fn apply_patch_file_and_commit(
    worktree: &Path,
    patch_path: &Path,
    message: &str,
) -> Result<bool> {
    if std::fs::metadata(patch_path).map(|m| m.len()).unwrap_or(0) == 0 {
        return Ok(false);
    }
    let apply = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .arg("apply")
        .arg("--index")
        .arg("--3way")
        .arg("--binary")
        .arg(patch_path)
        .output()
        .with_context(|| format!("spawn git apply for {worktree:?}"))?;
    if !apply.status.success() {
        let stderr = String::from_utf8_lossy(&apply.stderr).to_string();
        let mut files = unmerged_files(worktree);
        if files.is_empty() {
            files = parse_apply_failure_files(&stderr);
        }
        // Restore the worktree: drop conflict markers and any half-applied
        // state so the integration branch stays clean for the next task.
        let _ = Command::new("git")
            .arg("-C")
            .arg(worktree)
            .args(["reset", "-q", "--hard", "HEAD"])
            .status();
        let _ = Command::new("git")
            .arg("-C")
            .arg(worktree)
            .args(["clean", "-fdq"])
            .status();
        if !files.is_empty() {
            return Err(PatchConflict { files }.into());
        }
        anyhow::bail!("git apply failed: {}", stderr.trim());
    }

    let has_staged = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .arg("diff")
        .arg("--cached")
        .arg("--quiet")
        .status()
        .with_context(|| format!("spawn git diff --cached --quiet for {worktree:?}"))?;
    if has_staged.success() {
        return Ok(false);
    }

    let commit = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .arg("-c")
        .arg("user.name=maestro")
        .arg("-c")
        .arg("user.email=maestro@example.invalid")
        .arg("commit")
        .arg("-q")
        .arg("-m")
        .arg(message)
        .output()
        .with_context(|| format!("spawn git commit for {worktree:?}"))?;
    if !commit.status.success() {
        anyhow::bail!(
            "git commit failed: {}",
            String::from_utf8_lossy(&commit.stderr).trim()
        );
    }
    Ok(true)
}

/// Remove a previously created worktree. Errors are swallowed (worktrees
/// are best-effort cleanup; orphaned ones can be reclaimed with `git
/// worktree prune` if anything ever goes wrong).
pub fn remove_worktree(repo: &Path, target: &Path) {
    let _ = Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("worktree")
        .arg("remove")
        .arg("--force")
        .arg(target)
        .output();
    // Defensive: if `git worktree remove` left the dir, nuke it.
    if target.exists() {
        let _ = std::fs::remove_dir_all(target);
    }
}

/// RAII guard that creates a worktree on construction and removes it on
/// drop. The executor holds one of these per task that's running on a
/// git-backed project, so worktrees are always cleaned up even if the
/// async task panics.
pub struct WorktreeGuard {
    repo: PathBuf,
    target: PathBuf,
    branch: String,
    armed: bool,
}

impl WorktreeGuard {
    pub fn create(repo: PathBuf, target: PathBuf, branch: String) -> Result<Self> {
        Self::create_from(repo, target, branch, None)
    }

    pub fn create_from(
        repo: PathBuf,
        target: PathBuf,
        branch: String,
        start_point: Option<&str>,
    ) -> Result<Self> {
        create_worktree_from(&repo, &target, &branch, start_point)?;
        Ok(Self {
            repo,
            target,
            branch,
            armed: true,
        })
    }

    pub fn path(&self) -> &Path {
        &self.target
    }

    /// The branch name git created for this worktree. Useful for telling
    /// the user where their work landed.
    pub fn branch(&self) -> &str {
        &self.branch
    }

    /// Disarm the guard. Used when the caller wants to keep the worktree
    /// alive past the guard's lifetime (e.g. on failure, so the user can
    /// inspect what was changed).
    pub fn keep(mut self) -> PathBuf {
        self.armed = false;
        self.target.clone()
    }
}

impl Drop for WorktreeGuard {
    fn drop(&mut self) {
        if self.armed {
            remove_worktree(&self.repo, &self.target);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Spawn `git` with a few retries. The full test suite fires hundreds of
    /// git subprocesses in parallel and the OS can transiently refuse a
    /// fork/exec (EAGAIN) under that load — a plain `.output().unwrap()` then
    /// panics and the test flakes. Retrying the *spawn* (not git's own exit
    /// status) absorbs that without masking real failures.
    fn git_output(args: &[&str], cwd: &Path) -> std::process::Output {
        for attempt in 0..5 {
            match Command::new("git").args(args).current_dir(cwd).output() {
                Ok(out) => return out,
                Err(_) if attempt < 4 => {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(e) => panic!("git {args:?} failed to spawn: {e}"),
            }
        }
        unreachable!()
    }

    fn make_repo() -> TempDir {
        let dir = TempDir::new().unwrap();
        let path = dir.path();
        let run = |args: &[&str]| git_output(args, path);
        run(&["init", "-q", "-b", "main"]);
        // git refuses to create worktrees from a repo with no commits, so
        // make one empty commit to anchor HEAD.
        run(&["config", "user.email", "test@example.invalid"]);
        run(&["config", "user.name", "test"]);
        run(&["commit", "--allow-empty", "-q", "-m", "init"]);
        dir
    }

    #[test]
    fn push_branch_publishes_to_a_remote() {
        let repo = make_repo();
        let path = repo.path();
        // a feature branch with a commit
        git_output(&["checkout", "-q", "-b", "feat/x"], path);
        git_output(&["commit", "--allow-empty", "-q", "-m", "work"], path);

        // no remote yet
        assert!(!has_remote(path, "origin"));
        assert!(push_branch(path, "origin", "feat/x").is_err());

        // wire a local bare remote and push
        let bare = TempDir::new().unwrap();
        git_output(&["init", "-q", "--bare"], bare.path());
        git_output(
            &["remote", "add", "origin", &bare.path().to_string_lossy()],
            path,
        );
        assert!(has_remote(path, "origin"));
        push_branch(path, "origin", "feat/x").expect("push should succeed");

        // the branch now exists in the bare remote
        let out = git_output(&["branch", "--list", "feat/x"], bare.path());
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("feat/x"),
            "pushed branch should appear in the bare remote"
        );
    }

    #[test]
    fn is_git_repo_recognises_initialized_dir() {
        let repo = make_repo();
        assert!(is_git_repo(repo.path()));
        assert!(is_git_worktree_root(repo.path()));

        let plain = TempDir::new().unwrap();
        assert!(!is_git_repo(plain.path()));
        assert!(!is_git_worktree_root(plain.path()));
    }

    #[test]
    fn git_worktree_root_rejects_nested_project_dirs() {
        let repo = make_repo();
        let nested = repo.path().join("packages").join("api");
        std::fs::create_dir_all(&nested).unwrap();
        assert!(is_git_repo(&nested));
        assert!(!is_git_worktree_root(&nested));
    }

    // ── F-108: change-tracking when a project is a subdir of a repo ──
    // git status --porcelain is repo-root-relative even with `-C <subdir>`,
    // but maestro's contract paths are project-relative. These pin that
    // changed_files / changed_files_with_status return PROJECT-relative
    // paths so the risk classifier (refute + gate_on_high_risk) matches.

    #[test]
    fn changed_files_in_repo_subdir_returns_project_relative_paths() {
        let repo = make_repo();
        let proj = repo.path().join("shared-lib");
        std::fs::create_dir_all(proj.join("idl")).unwrap();
        std::fs::write(proj.join("idl/user.proto"), "syntax=\"proto3\";\n").unwrap();

        let changed = changed_files(&proj).unwrap();
        assert_eq!(
            changed,
            vec!["idl/user.proto".to_string()],
            "subdir project must yield project-relative paths, got {changed:?}"
        );
    }

    #[test]
    fn changed_files_with_status_in_repo_subdir_returns_project_relative_paths() {
        let repo = make_repo();
        let proj = repo.path().join("shared-lib");
        std::fs::create_dir_all(proj.join("idl")).unwrap();
        std::fs::write(proj.join("idl/user.proto"), "syntax=\"proto3\";\n").unwrap();

        let changed = changed_files_with_status(&proj).unwrap();
        assert_eq!(
            changed,
            vec![('A', "idl/user.proto".to_string())],
            "status version must also be project-relative, got {changed:?}"
        );
    }

    #[test]
    fn changed_files_excludes_sibling_project_changes() {
        let repo = make_repo();
        let a = repo.path().join("shared-lib");
        let b = repo.path().join("app-alpha");
        std::fs::create_dir_all(a.join("idl")).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("idl/user.proto"), "x\n").unwrap();
        std::fs::write(b.join("main.rs"), "fn main(){}\n").unwrap();

        let changed = changed_files(&a).unwrap();
        assert_eq!(
            changed,
            vec!["idl/user.proto".to_string()],
            "a sibling project's change must not leak into this project, got {changed:?}"
        );
    }

    #[test]
    fn changed_files_single_repo_layout_unchanged() {
        // project IS the repo root — paths are already project-relative,
        // and this must keep working exactly as before.
        let repo = make_repo();
        std::fs::create_dir_all(repo.path().join("idl")).unwrap();
        std::fs::write(repo.path().join("idl/user.proto"), "x\n").unwrap();

        let changed = changed_files(repo.path()).unwrap();
        assert_eq!(changed, vec!["idl/user.proto".to_string()]);
    }

    #[test]
    fn subdir_project_change_classifies_high_risk_end_to_end() {
        // The F-108 payoff (dali's integration ask): a contract change in a
        // subdir-of-repo project now produces project-relative files that the
        // risk classifier matches against the project-relative contract path
        // → high risk. Before the fix, changed_files returned
        // `shared-lib/idl/user.proto`, which never matched the contract path
        // `idl/user.proto`, so risk stayed low and refute/gate silently
        // no-op'd. No adapter/agent needed — real git + the classifier.
        let repo = make_repo();
        let proj = repo.path().join("shared-lib");
        std::fs::create_dir_all(proj.join("idl")).unwrap();
        std::fs::write(proj.join("idl/user.proto"), "x\n").unwrap();

        let with_status: Vec<(char, String)> = changed_files(&proj)
            .unwrap()
            .into_iter()
            .map(|f| ('M', f))
            .collect();
        let contracts = vec!["idl/user.proto".to_string()];
        let risk = crate::scheduler::risk::classify_change_risk(&with_status, &contracts);
        assert_eq!(
            risk.level, "high",
            "subdir contract change must classify high; got {risk:?} from {with_status:?}"
        );
    }

    #[test]
    fn worktree_context_preserves_nested_project_path() {
        let repo = make_repo();
        let nested = repo.path().join("packages").join("api");
        std::fs::create_dir_all(&nested).unwrap();

        let context = worktree_context(&nested).unwrap();
        assert_eq!(context.repo_root, repo.path().canonicalize().unwrap());
        assert_eq!(
            context.project_relative_path,
            PathBuf::from("packages").join("api")
        );
    }

    #[test]
    fn git_common_dir_is_stable_across_linked_worktrees() {
        let repo = make_repo();
        let tmp = TempDir::new().unwrap();
        let target =
            create_worktree(repo.path(), &tmp.path().join("wt"), "maestro/test/common").unwrap();

        assert_eq!(git_common_dir(repo.path()), git_common_dir(&target));
        remove_worktree(repo.path(), &target);
    }

    #[test]
    fn changed_files_reports_git_status_paths() {
        let repo = make_repo();
        std::fs::create_dir_all(repo.path().join("src")).unwrap();
        std::fs::write(repo.path().join("src").join("lib.rs"), "pub fn old() {}\n").unwrap();
        git_output(&["add", "."], repo.path());
        git_output(&["commit", "-q", "-m", "add lib"], repo.path());

        std::fs::write(repo.path().join("src").join("lib.rs"), "pub fn new() {}\n").unwrap();
        std::fs::write(repo.path().join("README.md"), "hello\n").unwrap();
        std::fs::create_dir_all(repo.path().join(".maestro").join("runs")).unwrap();
        std::fs::write(
            repo.path().join(".maestro").join("runs").join("state"),
            "{}",
        )
        .unwrap();
        std::fs::create_dir_all(repo.path().join("src").join("__pycache__")).unwrap();
        std::fs::write(
            repo.path()
                .join("src")
                .join("__pycache__")
                .join("lib.cpython-314.pyc"),
            b"bytecode",
        )
        .unwrap();
        std::fs::write(repo.path().join(".DS_Store"), b"metadata").unwrap();

        let files = changed_files(repo.path()).unwrap();
        assert_eq!(files, vec!["README.md", "src/lib.rs"]);
    }

    #[test]
    fn patch_round_trip_applies_and_commits_to_another_worktree() {
        let repo = make_repo();
        std::fs::write(repo.path().join("base.txt"), "base\n").unwrap();
        git_output(&["add", "."], repo.path());
        git_output(&["commit", "-q", "-m", "base"], repo.path());

        let tmp = TempDir::new().unwrap();
        let source = create_worktree(
            repo.path(),
            &tmp.path().join("source"),
            "maestro/test/patch-source",
        )
        .unwrap();
        let target = create_worktree(
            repo.path(),
            &tmp.path().join("target"),
            "maestro/test/patch-target",
        )
        .unwrap();

        std::fs::write(source.join("base.txt"), "changed\n").unwrap();
        std::fs::write(source.join("new.txt"), "new\n").unwrap();
        std::fs::create_dir_all(source.join("__pycache__")).unwrap();
        std::fs::write(
            source.join("__pycache__").join("test.cpython-314.pyc"),
            b"pyc",
        )
        .unwrap();
        let patch = tmp.path().join("task.patch");
        assert!(write_index_patch(&source, &patch).unwrap());
        let patch_text = std::fs::read_to_string(&patch).unwrap();
        assert!(
            !patch_text.contains("__pycache__"),
            "transient bytecode cache should not be staged into integration patches"
        );
        assert!(apply_patch_file_and_commit(&target, &patch, "integrate task").unwrap());

        assert_eq!(
            std::fs::read_to_string(target.join("base.txt")).unwrap(),
            "changed\n"
        );
        assert_eq!(
            std::fs::read_to_string(target.join("new.txt")).unwrap(),
            "new\n"
        );
        assert!(!target.join("__pycache__").exists());
        remove_worktree(repo.path(), &source);
        remove_worktree(repo.path(), &target);
    }

    #[test]
    fn three_way_merge_integrates_non_overlapping_edits_to_same_file() {
        // Two tasks edit different regions of the same shared file in parallel.
        // Strict `git apply` would reject the second; the 3-way merge lands both.
        let repo = make_repo();
        std::fs::write(
            repo.path().join("shared.txt"),
            "line1\nline2\nline3\nline4\nline5\n",
        )
        .unwrap();
        git_output(&["add", "."], repo.path());
        git_output(&["commit", "-q", "-m", "shared"], repo.path());

        let tmp = TempDir::new().unwrap();
        let a = create_worktree(repo.path(), &tmp.path().join("a"), "maestro/test/3way-a").unwrap();
        let b = create_worktree(repo.path(), &tmp.path().join("b"), "maestro/test/3way-b").unwrap();
        let integ = create_worktree(
            repo.path(),
            &tmp.path().join("integ"),
            "maestro/test/3way-integ",
        )
        .unwrap();

        // A edits the top, B edits the bottom — disjoint regions.
        std::fs::write(
            a.join("shared.txt"),
            "LINE1-by-a\nline2\nline3\nline4\nline5\n",
        )
        .unwrap();
        std::fs::write(
            b.join("shared.txt"),
            "line1\nline2\nline3\nline4\nLINE5-by-b\n",
        )
        .unwrap();

        let pa = tmp.path().join("a.patch");
        let pb = tmp.path().join("b.patch");
        assert!(write_index_patch(&a, &pa).unwrap());
        assert!(write_index_patch(&b, &pb).unwrap());

        assert!(apply_patch_file_and_commit(&integ, &pa, "integrate a").unwrap());
        assert!(
            apply_patch_file_and_commit(&integ, &pb, "integrate b").unwrap(),
            "non-overlapping edit to the same file should 3-way merge cleanly"
        );

        let merged = std::fs::read_to_string(integ.join("shared.txt")).unwrap();
        assert_eq!(merged, "LINE1-by-a\nline2\nline3\nline4\nLINE5-by-b\n");

        remove_worktree(repo.path(), &a);
        remove_worktree(repo.path(), &b);
        remove_worktree(repo.path(), &integ);
    }

    #[test]
    fn overlapping_edits_yield_patch_conflict_naming_the_file() {
        // Two tasks edit the *same* region — a true conflict. The second apply
        // returns a typed PatchConflict naming the file, and the worktree is
        // restored to a clean state (no leftover conflict markers).
        let repo = make_repo();
        std::fs::write(repo.path().join("shared.txt"), "alpha\nbeta\ngamma\n").unwrap();
        git_output(&["add", "."], repo.path());
        git_output(&["commit", "-q", "-m", "shared"], repo.path());

        let tmp = TempDir::new().unwrap();
        let a = create_worktree(repo.path(), &tmp.path().join("a"), "maestro/test/conf-a").unwrap();
        let b = create_worktree(repo.path(), &tmp.path().join("b"), "maestro/test/conf-b").unwrap();
        let integ = create_worktree(
            repo.path(),
            &tmp.path().join("integ"),
            "maestro/test/conf-integ",
        )
        .unwrap();

        std::fs::write(a.join("shared.txt"), "alpha-A\nbeta\ngamma\n").unwrap();
        std::fs::write(b.join("shared.txt"), "alpha-B\nbeta\ngamma\n").unwrap();

        let pa = tmp.path().join("a.patch");
        let pb = tmp.path().join("b.patch");
        assert!(write_index_patch(&a, &pa).unwrap());
        assert!(write_index_patch(&b, &pb).unwrap());

        assert!(apply_patch_file_and_commit(&integ, &pa, "integrate a").unwrap());
        let err = apply_patch_file_and_commit(&integ, &pb, "integrate b").unwrap_err();
        let conflict = err
            .downcast_ref::<PatchConflict>()
            .expect("overlapping edit should surface a PatchConflict");
        assert!(conflict.files.iter().any(|f| f == "shared.txt"));

        // Worktree restored: HEAD content intact, no conflict markers, clean tree.
        assert_eq!(
            std::fs::read_to_string(integ.join("shared.txt")).unwrap(),
            "alpha-A\nbeta\ngamma\n"
        );
        assert!(changed_files(&integ).unwrap().is_empty());

        remove_worktree(repo.path(), &a);
        remove_worktree(repo.path(), &b);
        remove_worktree(repo.path(), &integ);
    }

    #[test]
    fn worktree_round_trip_creates_and_removes_files() {
        let repo = make_repo();
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("wt");

        let path = create_worktree(repo.path(), &target, "maestro/test/wt-1").unwrap();
        assert_eq!(path, target);
        assert!(target.exists(), "worktree dir should exist");
        assert!(
            target.join(".git").exists(),
            "worktree should be a real checkout"
        );

        remove_worktree(repo.path(), &target);
        assert!(!target.exists(), "worktree dir should be gone");
    }

    #[test]
    fn guard_cleans_up_on_drop() {
        let repo = make_repo();
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("wt");

        {
            let _g = WorktreeGuard::create(
                repo.path().to_path_buf(),
                target.clone(),
                "maestro/test/guarded".into(),
            )
            .unwrap();
            assert!(target.exists());
        }
        assert!(
            !target.exists(),
            "guard should have removed worktree on drop"
        );
    }

    #[test]
    fn guard_keep_preserves_worktree() {
        let repo = make_repo();
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("wt-keep");

        let g = WorktreeGuard::create(
            repo.path().to_path_buf(),
            target.clone(),
            "maestro/test/kept".into(),
        )
        .unwrap();
        let kept = g.keep();
        assert!(kept.exists(), "kept worktree should still exist");
        // Clean up manually since we disarmed the guard.
        remove_worktree(repo.path(), &kept);
    }
}
