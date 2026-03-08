use gardener::errors::GardenerError;
use gardener::logging::append_run_log;
use gardener::phase_cli::{print_agent_event, step, PhaseRuntime};
use gardener::plan_phase::{run_plan, PlanContext};

pub fn run_with_args(args: &[String]) -> Result<i32, GardenerError> {
    append_run_log("info", "bin.plan.started", serde_json::json!({}));
    let task = get_arg(args, "--task")
        .ok_or_else(|| GardenerError::Cli("Usage: plan --task <TASK_SUMMARY> --worktree <PATH>".to_string()))?;
    let worktree = get_arg(args, "--worktree")
        .ok_or_else(|| {
            GardenerError::Cli("Usage: plan --task <TASK_SUMMARY> --worktree <PATH>".to_string())
        })?;

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
    Ok(0)
}

fn get_arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}
