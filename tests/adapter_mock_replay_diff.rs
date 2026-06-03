use maestro::adapter::mock::{MockAdapter, MockBehavior};
use maestro::adapter::{AgentAdapter, AgentTask, ExecutionMode, MemorySlice, TrajectoryContext};
use maestro::modes::AllowedTools;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use tempfile::TempDir;

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git command");
    assert!(
        output.status.success(),
        "git {:?} failed\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn git_stdout(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git command");
    assert!(
        output.status.success(),
        "git {:?} failed\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn init_repo() -> TempDir {
    let repo = TempDir::new().expect("repo");
    git(repo.path(), &["init", "-q"]);
    fs::write(repo.path().join("README.md"), "hello\n").expect("readme");
    git(repo.path(), &["add", "README.md"]);
    git(
        repo.path(),
        &[
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "user.name=Test User",
            "commit",
            "-q",
            "-m",
            "initial",
        ],
    );
    repo
}

fn task(repo: &Path, log_path: &Path) -> AgentTask {
    AgentTask {
        task_id: "T_mock".to_string(),
        workspace: repo.to_path_buf(),
        prompt: "Apply replay diff".to_string(),
        context: Vec::<MemorySlice>::new(),
        timeout: Duration::from_secs(30),
        mode: ExecutionMode::Apply,
        resume_chat_id: None,
        log_path: log_path.to_path_buf(),
        trajectory: None::<TrajectoryContext>,
        model: None,
        role_prelude: None,
        allowed_tools: AllowedTools::default(),
    }
}

#[tokio::test]
async fn replay_diff_applies_patch_and_reports_changed_files() {
    let repo = init_repo();
    fs::write(repo.path().join("README.md"), "hello\nworld\n").expect("mutate readme");
    let patch = git_stdout(repo.path(), &["diff"]);
    git(repo.path(), &["checkout", "--", "README.md"]);
    let diff_dir = TempDir::new().expect("diff dir");
    fs::write(diff_dir.path().join("change.diff"), patch).expect("patch file");
    let log_path = repo.path().join("mock.log");
    let adapter = MockAdapter {
        sleep: Duration::ZERO,
        behavior: MockBehavior::ReplayDiff {
            from: diff_dir.path().to_path_buf(),
        },
    };

    let result = adapter
        .run(task(repo.path(), &log_path))
        .await
        .expect("mock replay diff");

    assert_eq!(
        fs::read_to_string(repo.path().join("README.md")).expect("readme"),
        "hello\nworld\n"
    );
    assert_eq!(result.artifacts.files_changed, vec!["README.md"]);
    assert_eq!(result.transcript_summary, "applied diff: 1 files");
}

#[tokio::test]
async fn replay_diff_rejects_invalid_patch_without_mutating_workspace() {
    let repo = init_repo();
    let diff_dir = TempDir::new().expect("diff dir");
    fs::write(diff_dir.path().join("broken.diff"), "not a patch\n").expect("patch file");
    let adapter = MockAdapter {
        sleep: Duration::ZERO,
        behavior: MockBehavior::ReplayDiff {
            from: diff_dir.path().to_path_buf(),
        },
    };

    let error = adapter
        .run(task(repo.path(), &repo.path().join("mock.log")))
        .await
        .expect_err("invalid patch");

    assert!(format!("{error:#}").contains("git apply --check failed"));
    assert_eq!(
        fs::read_to_string(repo.path().join("README.md")).expect("readme"),
        "hello\n"
    );
}
