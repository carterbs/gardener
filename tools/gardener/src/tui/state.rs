pub(crate) mod seed_review;
pub(crate) mod wizard;

use super::backlog::{ordered_backlog_items, BacklogItem, BacklogPriority, ParsedBacklogPriority};
use super::formatting::{equipment_name_for_worker, format_breadcrumb, now_hhmmss};
use super::triage::{triage_stage_progress, triage_stages_with_state};
use super::formatting::parse_triage_artifact;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerRow {
    pub worker_id: String,
    pub state: String,
    pub task_id: Option<String>,
    pub last_state_line: usize,
    pub task_title: String,
    pub tool_line: String,
    pub breadcrumb: String,
    pub last_heartbeat_secs: u64,
    pub session_age_secs: u64,
    pub lease_held: bool,
    pub session_missing: bool,
    pub command_details: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueStats {
    pub ready: usize,
    pub active: usize,
    pub failed: usize,
    pub unresolved: usize,
    pub merge_pending: usize,
    pub p0: usize,
    pub p1: usize,
    pub p2: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BacklogView {
    pub in_progress: Vec<String>,
    pub queued: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiMode {
    Triage,
    Work,
}

#[derive(Debug, Clone)]
pub struct TriageStage {
    pub label: String,
    pub state: StageState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageState {
    Done,
    Current,
    Future,
}

#[derive(Debug, Clone)]
pub struct TriageActivity {
    pub timestamp: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct TriageArtifact {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct StartupHeadline {
    pub spinner_frame: usize,
    pub verb: String,
    pub startup_active: bool,
    pub ellipsis_phase: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerState {
    Doing,
    Reviewing,
    Complete,
    Failed,
    Idle,
}

impl WorkerState {
    fn from_str(state: &str) -> Self {
        match state {
            "reviewing" => Self::Reviewing,
            "complete" => Self::Complete,
            "failed" | "unresolved" => Self::Failed,
            "idle" => Self::Idle,
            _ => Self::Doing,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ActivityEntry {
    pub timestamp: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct CommandEntry {
    pub timestamp: String,
    pub command: String,
}

#[derive(Debug, Clone)]
pub struct WorkerCard {
    pub name: String,
    pub state: String,
    pub task: String,
    pub tool_line: String,
    pub breadcrumb: String,
    pub activity: Vec<ActivityEntry>,
    pub command_details: Vec<CommandEntry>,
    pub state_bucket: WorkerState,
    pub last_heartbeat_secs: u64,
    pub lease_held: bool,
    pub session_missing: bool,
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub ui_mode: UiMode,
    pub triage_stages: Vec<TriageStage>,
    pub triage_activity: Vec<TriageActivity>,
    pub triage_artifacts: Vec<TriageArtifact>,
    pub startup_headline: StartupHeadline,
    pub workers: Vec<WorkerCard>,
    pub backlog: Vec<BacklogItem>,
    pub selected_worker: usize,
    pub terminal_width: u16,
    pub terminal_height: u16,
}

impl AppState {
    pub(crate) fn from_dashboard_feed(
        workers: &[WorkerRow],
        backlog: &BacklogView,
        startup_headline: StartupHeadline,
        selected_worker: usize,
    ) -> Self {
        let triage_stages = triage_stages_with_state(0);

        let mapped_workers = workers
            .iter()
            .enumerate()
            .map(|(index, row)| WorkerCard {
                name: equipment_name_for_worker(index, &row.worker_id),
                state: row.state.clone(),
                task: row.task_title.clone(),
                tool_line: row.tool_line.clone(),
                breadcrumb: row.breadcrumb.clone(),
                activity: vec![ActivityEntry {
                    timestamp: now_hhmmss(),
                    message: if row.breadcrumb.is_empty() {
                        row.tool_line.clone()
                    } else {
                        format!("{} ({})", row.tool_line, format_breadcrumb(&row.breadcrumb))
                    },
                }],
                command_details: row
                    .command_details
                    .iter()
                    .map(|(timestamp, command)| CommandEntry {
                        timestamp: timestamp.clone(),
                        command: command.clone(),
                    })
                    .collect(),
                state_bucket: WorkerState::from_str(&row.state),
                last_heartbeat_secs: row.last_heartbeat_secs,
                lease_held: row.lease_held,
                session_missing: row.session_missing,
            })
            .collect();

        let mapped_backlog = ordered_backlog_items(&backlog.in_progress, &backlog.queued)
            .into_iter()
            .map(|item| BacklogItem {
                priority: match item.priority {
                    ParsedBacklogPriority::P0 => BacklogPriority::P0,
                    ParsedBacklogPriority::P1 => BacklogPriority::P1,
                    ParsedBacklogPriority::P2 => BacklogPriority::P2,
                },
                title: item.title,
            })
            .collect();

        Self {
            ui_mode: UiMode::Work,
            triage_stages,
            triage_activity: Vec::new(),
            triage_artifacts: Vec::new(),
            startup_headline,
            workers: mapped_workers,
            backlog: mapped_backlog,
            selected_worker,
            terminal_width: 0,
            terminal_height: 0,
        }
    }

    pub(crate) fn from_triage_feed(
        activity: &[String],
        artifacts: &[String],
        startup_headline: StartupHeadline,
    ) -> Self {
        let current_triage_stage = triage_stage_progress(activity);
        let triage_stages = triage_stages_with_state(current_triage_stage);
        Self {
            ui_mode: UiMode::Triage,
            triage_stages,
            triage_activity: activity
                .iter()
                .map(|line| TriageActivity {
                    timestamp: now_hhmmss(),
                    message: line.clone(),
                })
                .collect(),
            triage_artifacts: artifacts
                .iter()
                .map(|line| parse_triage_artifact(line))
                .collect(),
            startup_headline,
            workers: Vec::new(),
            backlog: Vec::new(),
            selected_worker: 0,
            terminal_width: 0,
            terminal_height: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WorkerMetrics {
    pub(crate) total: usize,
    pub(crate) doing: usize,
    pub(crate) reviewing: usize,
    pub(crate) idle: usize,
    pub(crate) complete: usize,
    pub(crate) failed: usize,
}

impl WorkerMetrics {
    pub(crate) fn from_app_state<'a, I>(workers: I) -> Self
    where
        I: IntoIterator<Item = &'a WorkerCard>,
    {
        let mut metrics = Self {
            total: 0,
            doing: 0,
            reviewing: 0,
            idle: 0,
            complete: 0,
            failed: 0,
        };
        for worker in workers {
            metrics.total += 1;
            match worker.state_bucket {
                WorkerState::Doing => metrics.doing += 1,
                WorkerState::Reviewing => metrics.reviewing += 1,
                WorkerState::Idle => metrics.idle += 1,
                WorkerState::Complete => metrics.complete += 1,
                WorkerState::Failed => metrics.failed += 1,
            }
        }
        metrics
    }
}
