use gardener::worktree_audit::reconcile_worktrees_in;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn run_git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {:?} failed\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_repo() -> TempDir {
    let temp = tempfile::tempdir().expect("tempdir");
    run_git(temp.path(), &["init", "--initial-branch=main"]);
    run_git(temp.path(), &["config", "user.name", "Gardener Tests"]);
    run_git(
        temp.path(),
        &["config", "user.email", "gardener-tests@example.com"],
    );
    std::fs::write(temp.path().join("README.md"), "seed\n").expect("write seed file");
    run_git(temp.path(), &["add", "README.md"]);
    run_git(temp.path(), &["commit", "-m", "seed"]);
    temp
}

#[test]
fn reconcile_worktrees_in_prunes_missing_worktree_paths_in_real_repo() {
    let repo = init_repo();
    let stale_worktree = repo.path().join(".worktrees/task-1");
    std::fs::create_dir_all(stale_worktree.parent().expect("worktree parent"))
        .expect("create worktree root");

    run_git(
        repo.path(),
        &[
            "worktree",
            "add",
            stale_worktree.to_str().expect("utf8 worktree path"),
            "-b",
            "task-1",
        ],
    );

    std::fs::remove_dir_all(&stale_worktree).expect("remove worktree directory");

    let summary = reconcile_worktrees_in(repo.path());

    assert_eq!(summary.stale_found, 1);
    assert_eq!(summary.stale_fixed, 1);

    let output = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo.path())
        .output()
        .expect("list worktrees");
    assert!(output.status.success(), "git worktree list should succeed");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(!stdout.contains(stale_worktree.to_str().expect("utf8 path")));
}

#[test]
fn reconcile_worktrees_in_leaves_clean_repo_unchanged() {
    let repo = init_repo();
    let active_worktree = repo.path().join(".worktrees/task-2");
    std::fs::create_dir_all(active_worktree.parent().expect("worktree parent"))
        .expect("create worktree root");

    run_git(
        repo.path(),
        &[
            "worktree",
            "add",
            active_worktree.to_str().expect("utf8 worktree path"),
            "-b",
            "task-2",
        ],
    );

    let summary = reconcile_worktrees_in(repo.path());

    assert_eq!(summary.stale_found, 0);
    assert_eq!(summary.stale_fixed, 0);
    assert!(active_worktree.exists());
}

#[test]
fn reconcile_worktrees_in_returns_default_for_non_repo_directory() {
    let temp = tempfile::tempdir().expect("tempdir");

    let summary = reconcile_worktrees_in(temp.path());

    assert_eq!(summary.stale_found, 0);
    assert_eq!(summary.stale_fixed, 0);
}
