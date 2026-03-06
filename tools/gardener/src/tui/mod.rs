mod backlog;
mod dashboard;
mod formatting;
mod quality;
mod report;
mod seed_review;
mod startup;
mod state;
mod terminal;
mod triage;
mod wizard;

pub use quality::{
    draw_quality_grading_live, draw_quality_intro_live, render_quality_grading,
    render_quality_intro, QUALITY_DIMENSIONS,
};
pub use report::{render_report_view, reset_report_scroll, scroll_report_down, scroll_report_up};
pub use seed_review::{render_seed_review, run_seed_review_wizard, ReviewDecision};
pub use terminal::{
    close_live_terminal, draw_dashboard_live, draw_report_live, draw_seeding_live,
    draw_shutdown_screen_live, draw_triage_live, render_seeding, reset_workers_scroll,
    scroll_workers_down, scroll_workers_up,
};
pub use triage::render_triage;
pub use wizard::{run_repo_health_wizard, RepoHealthWizardAnswers};

pub use state::{
    ActivityEntry, AppState, BacklogView, CommandEntry, QueueStats, StageState, StartupHeadline,
    TriageActivity, TriageArtifact, TriageStage, UiMode, WorkerCard, WorkerRow, WorkerState,
};

pub use dashboard::render_dashboard;
#[cfg(test)]
pub(crate) use dashboard::render_dashboard_at_tick;
pub(crate) use formatting::format_state_label;
pub use formatting::now_hhmmss;
#[cfg(test)]
pub(crate) use formatting::{
    command_stream_window, format_breadcrumb, style_activity_line, wizard_step_indicator,
    worker_command_stream, worker_flow_chain_spans, WIZARD_STEP_LABELS,
};
#[cfg(test)]
pub(crate) use startup::StartupHeadlineView;
#[cfg(test)]
pub(crate) use state::WorkerMetrics;
#[cfg(test)]
pub(crate) use wizard::{WizardAction, WizardState};

use ratatui::backend::TestBackend;
use ratatui::Terminal;

pub(super) fn render_to_string<F>(width: u16, height: u16, draw: F) -> String
where
    F: FnOnce(&mut ratatui::Frame<'_>),
{
    let backend = TestBackend::new(width, height);
    let mut terminal = match Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(err) => panic!("terminal: {err}"),
    };
    terminal
        .draw(draw)
        .unwrap_or_else(|err| panic!("draw: {err}"));

    let mut out = String::new();
    let buffer = terminal.backend().buffer().clone();
    for y in 0..height {
        for x in 0..width {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests;
