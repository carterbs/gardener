use gardener::errors::GardenerError;
use gardener::logging::append_run_log;
use gardener::phase_cli::{step, print_agent_event, PhaseRuntime};
use gardener::git_phase::{run_git_push, GitPushContext};

fn main() -> Result<(), GardenerError> {
    append_run_log("info", "bin.git_push.started", serde_json::json!({}));
    let args: Vec<String> = std::env::args().collect();
    let task = get_arg(&args, "--task").unwrap_or_else(|| {
        eprintln!("Usage: git-push --task <TASK_SUMMARY> --worktree <PATH>");
        std::process::exit(1);
    });
    let worktree = get_arg(&args, "--worktree").unwrap_or_else(|| {
        eprintln!("Usage: git-push --task <TASK_SUMMARY> --worktree <PATH>");
        std::process::exit(1);
    });

    let rt = PhaseRuntime::init("git-push")?;
    let worktree_path = std::path::PathBuf::from(&worktree);
    let branch = format!("gardener/{}", rt.identity.worker_id);

    step("git-push", "RUN", &format!("task={task} worktree={worktree} branch={branch}"));

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
        on_step: Some(&|label, detail| step("git-push", label, detail)),
        on_agent_event: Some(&|event| print_agent_event("git-push", event)),
    })?;

    step("git-push", "DONE", &format!("pr_number={} pr_url={}", outcome.pr_number, outcome.pr_url));
    Ok(())
}

fn get_arg(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1).cloned())
}
