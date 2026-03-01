use gardener::errors::GardenerError;
use gardener::logging::append_run_log;
use gardener::phase_cli::{print_agent_event, resolve_worktree_from_pr, step, PhaseRuntime};
use gardener::review_phase::{run_review, ReviewContext};

fn main() -> Result<(), GardenerError> {
    append_run_log("info", "bin.review_pr.started", serde_json::json!({}));
    let pr: u64 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| {
            eprintln!("Usage: review-pr --pr <NUMBER>");
            std::process::exit(1);
        });

    let rt = PhaseRuntime::init("review-pr")?;

    let (worktree_path, branch, pr_number) =
        resolve_worktree_from_pr(&rt.runner, &rt.scope, pr, "review-pr")?;

    step(
        "review-pr",
        "RUN",
        &format!(
            "pr={pr_number} branch={branch} worktree={}",
            worktree_path.display()
        ),
    );

    let outcome = run_review(&ReviewContext {
        cfg: &rt.cfg,
        process_runner: &rt.runner,
        scope: &rt.scope,
        worktree_path: &worktree_path,
        factory: &rt.factory,
        registry: &rt.registry,
        learning_loop: &rt.learning_loop,
        identity: &rt.identity,
        task_summary: &format!("Review PR #{pr_number}"),
        attempt_count: 1,
        pr_number,
        branch: &branch,
        task_id: &format!("pr-{pr_number}"),
        on_step: Some(&|label, detail| step("review-pr", label, detail)),
        on_agent_event: Some(&|event| print_agent_event("review-pr", event)),
    })?;

    step(
        "review-pr",
        "DONE",
        &format!(
            "verdict={:?} suggestions={}",
            outcome.verdict,
            outcome.suggestions.len()
        ),
    );
    Ok(())
}
