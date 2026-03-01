use gardener::errors::GardenerError;
use gardener::logging::append_run_log;
use gardener::phase_cli::{step, print_agent_event, PhaseRuntime};
use gardener::plan_phase::{run_plan, PlanContext};

fn main() -> Result<(), GardenerError> {
    append_run_log("info", "bin.plan.started", serde_json::json!({}));
    let args: Vec<String> = std::env::args().collect();
    let task = get_arg(&args, "--task").unwrap_or_else(|| {
        eprintln!("Usage: plan --task <TASK_SUMMARY> --worktree <PATH>");
        std::process::exit(1);
    });
    let worktree = get_arg(&args, "--worktree").unwrap_or_else(|| {
        eprintln!("Usage: plan --task <TASK_SUMMARY> --worktree <PATH>");
        std::process::exit(1);
    });

    let rt = PhaseRuntime::init("plan")?;
    let worktree_path = std::path::PathBuf::from(&worktree);

    step("plan", "RUN", &format!("task={task} worktree={worktree}"));

    let _outcome = run_plan(&PlanContext {
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
        on_step: Some(&|label, detail| step("plan", label, detail)),
        on_agent_event: Some(&|event| print_agent_event("plan", event)),
    })?;

    step("plan", "DONE", "planning phase complete");
    Ok(())
}

fn get_arg(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1).cloned())
}
