use gardener::errors::GardenerError;
use gardener::logging::append_run_log;
use gardener::phase_cli::{step, print_agent_event, PhaseRuntime};
use gardener::do_phase::{run_do, DoContext};

fn main() -> Result<(), GardenerError> {
    append_run_log("info", "bin.do_task.started", serde_json::json!({}));
    let args: Vec<String> = std::env::args().collect();
    let task = get_arg(&args, "--task").unwrap_or_else(|| {
        eprintln!("Usage: do-task --task <TASK_SUMMARY> --worktree <PATH>");
        std::process::exit(1);
    });
    let worktree = get_arg(&args, "--worktree").unwrap_or_else(|| {
        eprintln!("Usage: do-task --task <TASK_SUMMARY> --worktree <PATH>");
        std::process::exit(1);
    });

    let rt = PhaseRuntime::init("do-task")?;
    let worktree_path = std::path::PathBuf::from(&worktree);

    step("do-task", "RUN", &format!("task={task} worktree={worktree}"));

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
    Ok(())
}

fn get_arg(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1).cloned())
}
