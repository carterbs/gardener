use gardener::do_phase::{run_do, DoContext};
use gardener::errors::GardenerError;
use gardener::logging::append_run_log;
use gardener::phase_cli::{print_agent_event, step, PhaseRuntime};

pub fn run_with_args(args: &[String]) -> Result<i32, GardenerError> {
    append_run_log("info", "bin.do_task.started", serde_json::json!({}));
    let task = get_arg(args, "--task").ok_or_else(|| {
        GardenerError::Cli("Usage: do-task --task <TASK_SUMMARY> --worktree <PATH>".to_string())
    })?;
    let worktree = get_arg(args, "--worktree").ok_or_else(|| {
        GardenerError::Cli("Usage: do-task --task <TASK_SUMMARY> --worktree <PATH>".to_string())
    })?;

    let rt = PhaseRuntime::init("do-task")?;
    let worktree_path = std::path::PathBuf::from(&worktree);

    step(
        "do-task",
        "RUN",
        &format!("task={task} worktree={worktree}"),
    );

    let outcome = run_do(&DoContext {
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
        on_step: Some(&|label, detail| step("do-task", label, detail)),
        on_agent_event: Some(&|event| print_agent_event("do-task", event)),
    })?;

    step("do-task", "DONE", &format!("summary={}", outcome.summary));
    Ok(0)
}

fn get_arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}
