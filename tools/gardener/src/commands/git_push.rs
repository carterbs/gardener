use gardener::errors::GardenerError;
use gardener::git_phase::{run_git_push, GitPushContext};
use gardener::logging::append_run_log;
use gardener::phase_cli::{print_agent_event, step, PhaseRuntime};

pub fn run_with_args(args: &[String]) -> Result<i32, GardenerError> {
    append_run_log("info", "bin.git_push.started", serde_json::json!({}));
    let task = get_arg(args, "--task").ok_or_else(|| {
        GardenerError::Cli("Usage: git-push --task <TASK_SUMMARY> --worktree <PATH>".to_string())
    })?;
    let worktree = get_arg(args, "--worktree").ok_or_else(|| {
        GardenerError::Cli("Usage: git-push --task <TASK_SUMMARY> --worktree <PATH>".to_string())
    })?;

    let rt = PhaseRuntime::init("git-push")?;
    let worktree_path = std::path::PathBuf::from(&worktree);
    let branch = format!("gardener/{}", rt.identity.worker_id);

    step(
        "git-push",
        "RUN",
        &format!("task={task} worktree={worktree} branch={branch}"),
    );

    let outcome = run_git_push(&GitPushContext {
        cfg: &rt.cfg,
        process_runner: &rt.runner,
        scope: &rt.scope,
        worktree_path: &worktree_path,
        factory: &rt.factory,
        registry: &rt.registry,
        learning_loop: &rt.learning_loop,
        identity: &rt.identity,
        task_summary: &task,
        attempt_count: 1,
        branch: &branch,
        commit_message: &format!("feat: {task}"),
        skip_initial_commit: false,
        on_step: Some(&|label, detail| step("git-push", label, detail)),
        on_agent_event: Some(&|event| print_agent_event("git-push", event)),
    })?;

    step(
        "git-push",
        "DONE",
        &format!("pr_number={} pr_url={}", outcome.pr_number, outcome.pr_url),
    );
    Ok(0)
}

fn get_arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}
