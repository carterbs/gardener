use gardener::errors::GardenerError;
use gardener::logging::append_run_log;
use gardener::tui::{
    close_live_terminal, draw_quality_grading_live, draw_quality_intro_live, draw_report_live,
    draw_seeding_live, draw_shutdown_screen_live, draw_triage_live, run_repo_health_wizard,
    run_seed_review_wizard, BacklogView, QueueStats, ReviewDecision, WorkerRow,
};
use gardener::seed_runner::SeedTask;
use serde_json::json;

pub fn run_with_args(args: &[String]) -> Result<i32, GardenerError> {
    let mode = args
        .get(1)
        .cloned()
        .ok_or_else(|| GardenerError::Cli("missing mode".to_string()))?;
    append_run_log("debug", "bin.tui_live_smoke.run", json!({ "mode": mode }));

    match mode.as_str() {
        "dashboard" => gardener::tui::draw_dashboard_live(
            &[WorkerRow {
                worker_id: "worker-1".to_string(),
                state: "doing".to_string(),
                task_id: None,
                last_state_line: 0,
                task_title: "Exercise live dashboard".to_string(),
                tool_line: "git status".to_string(),
                breadcrumb: "understand>doing".to_string(),
                last_heartbeat_secs: 1,
                session_age_secs: 3,
                lease_held: true,
                session_missing: false,
                command_details: Vec::new(),
            }],
            &QueueStats {
                ready: 1,
                active: 1,
                failed: 0,
                unresolved: 0,
                merge_pending: 0,
                p0: 1,
                p1: 0,
                p2: 0,
            },
            &BacklogView::default(),
            5,
            30,
        )?,
        "report" => draw_report_live("report.md", "# Report\n\nline 1\nline 2")?,
        "seeding" => draw_seeding_live(&[
            "scan repository".to_string(),
            "index docs".to_string(),
        ])?,
        "triage" => draw_triage_live(
            &["discover runtime".to_string(), "scan logs".to_string()],
            &["artifact: profile.json".to_string()],
        )?,
        "shutdown" => {
            draw_shutdown_screen_live("Complete", "Tasks completed: 1\nTotal runtime: 1s")?
        }
        "quality-grading" => draw_quality_grading_live(&[
            "score docs".to_string(),
            "measure tests".to_string(),
        ])?,
        "quality-intro" => draw_quality_intro_live()?,
        "wizard" => {
            let answers = run_repo_health_wizard("./scripts/run-validate")?;
            if answers.validation_command.is_empty() {
                return Err(GardenerError::Cli("wizard returned empty validation".to_string()));
            }
        }
        "seed-review" => {
            let decisions = run_seed_review_wizard(
                &[
                    SeedTask {
                        title: "Improve docs".to_string(),
                        details: "Clarify the validation workflow".to_string(),
                        rationale: "Agents need a stable command path".to_string(),
                        domain: "docs".to_string(),
                        priority: "P1".to_string(),
                    },
                    SeedTask {
                        title: "Harden TUI smoke tests".to_string(),
                        details: "Exercise live terminal paths under a PTY".to_string(),
                        rationale: "Prevents regressions in coverage gates".to_string(),
                        domain: "runtime".to_string(),
                        priority: "P0".to_string(),
                    },
                ],
                0,
            )?;
            if decisions.len() != 2 || matches!(decisions[0], ReviewDecision::Discard(_)) {
                return Err(GardenerError::Cli(
                    "seed review returned unexpected decisions".to_string(),
                ));
            }
        }
        other => {
            return Err(GardenerError::Cli(format!(
                "unsupported mode: {other}"
            )))
        }
    }

    close_live_terminal()?;
    Ok(0)
}
