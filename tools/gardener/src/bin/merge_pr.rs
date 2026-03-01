use gardener::errors::GardenerError;
use gardener::gh::GhClient;
use gardener::git::GitClient;
use gardener::merge_loop::{run_merge_loop, MergeLoopContext};
use gardener::phase_cli::{step, print_agent_event, resolve_worktree_from_pr, PhaseRuntime};

fn main() -> Result<(), GardenerError> {
    gardener::logging::append_run_log("info", "bin.merge_pr.started", serde_json::json!({}));
    let pr: u64 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| {
            eprintln!("Usage: merge-pr --pr <NUMBER>");
            std::process::exit(1);
        });

    let rt = PhaseRuntime::init("merge-pr")?;

    let (worktree_path, branch, pr_number) =
        resolve_worktree_from_pr(&rt.runner, &rt.scope, pr, "merge-pr")?;

    step("merge-pr", "RUN", &format!("pr={pr_number} branch={branch} worktree={}", worktree_path.display()));

    let gh = GhClient::new(&rt.runner, &worktree_path);
    let git = GitClient::new(&rt.runner, &worktree_path);

    let outcome = run_merge_loop(&MergeLoopContext {
        cfg: &rt.cfg,
        process_runner: &rt.runner,
        scope: &rt.scope,
        worktree_path: &worktree_path,
        factory: &rt.factory,
        registry: &rt.registry,
        learning_loop: &rt.learning_loop,
        identity: &rt.identity,
        task_summary: &format!("Merge PR #{pr_number}"),
        attempt_count: 1,
        gh: &gh,
        git: &git,
        branch: &branch,
        pr_number,
        validation_command: &rt.validation_command,
        on_step: Some(&|label, detail| step("merge-pr", label, detail)),
        on_agent_event: Some(&|event| print_agent_event("merge-pr", event)),
    })?;

    match &outcome {
        gardener::merge_loop::MergeLoopOutcome::Merged { sha } => {
            step("merge-pr", "DONE", &format!("merged (sha={sha})"));
        }
        gardener::merge_loop::MergeLoopOutcome::Failed { reason } => {
            step("merge-pr", "FAILED", reason);
            std::process::exit(1);
        }
    }
    Ok(())
}
