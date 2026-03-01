use gardener::errors::GardenerError;
use gardener::logging::append_run_log;
use gardener::phase_cli::{print_agent_event, step, PhaseRuntime};
use gardener::understand_phase::{run_understand, UnderstandContext};

fn main() -> Result<(), GardenerError> {
    append_run_log("info", "bin.understand.started", serde_json::json!({}));
    let task = std::env::args().nth(2).unwrap_or_else(|| {
        eprintln!("Usage: understand --task <TASK_SUMMARY>");
        std::process::exit(1);
    });

    let rt = PhaseRuntime::init("understand")?;

    step("understand", "RUN", &format!("task={task}"));

    let worktree_path = rt.scope.working_dir.clone();

    let outcome = run_understand(&UnderstandContext {
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
        on_step: Some(&|label, detail| step("understand", label, detail)),
        on_agent_event: Some(&|event| print_agent_event("understand", event)),
    })?;

    step(
        "understand",
        "DONE",
        &format!(
            "category={:?} reasoning={}",
            outcome.category, outcome.reasoning
        ),
    );
    Ok(())
}
