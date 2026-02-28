use crate::errors::GardenerError;
use crate::logging::{current_run_id, current_run_log_path};
use crate::hotkeys::{dashboard_controls_legend, report_controls_legend};
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::backend::TestBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Terminal;
use std::cell::RefCell;
use std::io::{self, Stdout};
use std::time::{SystemTime, UNIX_EPOCH};

const WORKER_LIST_ROW_HEIGHT: usize = 3;
const RECENT_COMMAND_STREAM_LIMIT: usize = 4;
const WORKER_FLOW_STATES: [&str; 6] = [
    "understand",
    "doing",
    "gitting",
    "reviewing",
    "merging",
    "complete",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerRow {
    pub worker_id: String,
    pub state: String,
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
    pub p0: usize,
    pub p1: usize,
    pub p2: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BacklogView {
    pub in_progress: Vec<String>,
    pub queued: Vec<String>,
}

const STARTUP_SPINNER_FRAMES: [&str; 6] = ["⠋", "⠙", "⠸", "⠴", "⠦", "⠇"];
const STARTUP_VERBS: [&str; 6] = [
    "Scanning",
    "Seeding",
    "Pruning",
    "Cultivating",
    "Grafting",
    "Harvesting",
];
const STARTUP_SPINNER_TICK_MS: u128 = 150;
const STARTUP_ELLIPSIS_TICK_MS: u128 = 400;
const STARTUP_SPINNER_TICKS: u32 = 30;
const TRIAGE_STAGE_LABELS: [&str; 4] = [
    "Scan repository shape",
    "Detect tools and docs",
    "Build project profile",
    "Seed prioritized backlog",
];
const WIZARD_STEP_LABELS: [&str; 4] = [
    "Parallelism",
    "Validation",
    "Docs",
    "Notes",
];
const WORKER_EQUIPMENT_NAMES: [&str; 9] = [
    "Lawn Mower",
    "Leaf Blower",
    "Hedge Trimmer",
    "Edger",
    "String Trimmer",
    "Wheelbarrow",
    "Seed Spreader",
    "Pruning Shears",
    "Sprinkler",
];

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

impl StartupHeadline {
    fn from_view(source: StartupHeadlineView) -> Self {
        Self {
            spinner_frame: source.spinner_frame,
            verb: source.verb().to_string(),
            startup_active: source.startup_active,
            ellipsis_phase: source.ellipsis_phase,
        }
    }

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
            "failed" => Self::Failed,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BacklogPriority {
    P0,
    P1,
    P2,
}

impl BacklogPriority {
    fn span_style(self) -> Style {
        match self {
            Self::P0 => Style::default().fg(Color::Rgb(255, 122, 122)),
            Self::P1 => Style::default().fg(Color::Rgb(255, 207, 105)),
            Self::P2 => Style::default().fg(Color::Rgb(127, 230, 148)),
        }
    }
}

#[derive(Debug, Clone)]
pub struct BacklogItem {
    pub priority: BacklogPriority,
    pub title: String,
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
    fn from_dashboard_feed(
        workers: &[WorkerRow],
        backlog: &BacklogView,
        startup_headline: StartupHeadline,
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
                        format!("{} ({})", row.tool_line, humanize_breadcrumb(&row.breadcrumb))
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
            selected_worker: WORKERS_VIEWPORT_SELECTED.with(|cell| *cell.borrow()),
            terminal_width: 0,
            terminal_height: 0,
        }
    }

    fn from_triage_feed(
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
            triage_artifacts: artifacts.iter().map(|line| parse_triage_artifact(line)).collect(),
            startup_headline,
            workers: Vec::new(),
            backlog: Vec::new(),
            selected_worker: 0,
            terminal_width: 0,
            terminal_height: 0,
        }
    }
}

fn triage_stage_progress(activity: &[String]) -> usize {
    let mut current_stage = 0usize;
    for entry in activity {
        let lower = entry.to_ascii_lowercase();
        if lower.contains("persisted triage profile") || lower.contains("interview complete") {
            current_stage = 3;
        } else if lower.contains("discovery assessment complete")
            || lower.contains("running repository discovery assessment")
        {
            current_stage = 2;
        } else if lower.contains("collecting human-validated repository context") {
            current_stage = current_stage.max(2);
        } else if lower.contains("agent detection complete")
            || lower.contains("detecting coding agent signals")
        {
            current_stage = current_stage.max(1);
        } else if lower.contains("starting triage session") {
            current_stage = current_stage.max(0);
        }
    }
    current_stage
}

fn triage_stages_with_state(current_stage: usize) -> Vec<TriageStage> {
    TRIAGE_STAGE_LABELS
        .iter()
        .enumerate()
        .map(|(index, label)| TriageStage {
            label: (*label).to_string(),
            state: if index < current_stage {
                StageState::Done
            } else if index == current_stage {
                StageState::Current
            } else {
                StageState::Future
            },
        })
        .collect()
}

fn wizard_step_indicator(current_step: usize) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, label) in WIZARD_STEP_LABELS.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ", Style::default()));
        }
        let (dot, style) = if i < current_step {
            ("● ", Style::default().fg(Color::Rgb(126, 231, 135)))
        } else if i == current_step {
            (
                "● ",
                Style::default()
                    .fg(Color::Rgb(85, 198, 255))
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            ("○ ", Style::default().fg(Color::Rgb(82, 88, 126)))
        };
        spans.push(Span::styled(dot, style));
        spans.push(Span::styled(
            *label,
            if i == current_step {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else if i < current_step {
                Style::default().fg(Color::Rgb(126, 231, 135))
            } else {
                Style::default().fg(Color::Rgb(82, 88, 126))
            },
        ));
    }
    Line::from(spans)
}

fn command_row_with_timestamp(timestamp: &str, command: &str, max_width: usize) -> String {
    let mut command = command.to_string();
    let prefix = format!("{timestamp}  ");
    if max_width <= prefix.len() {
        return prefix;
    }
    let available = max_width.saturating_sub(prefix.len());
    if command.len() > available {
        command = truncate_right(&command, available);
    }
    format!("{prefix}{command}")
}

fn truncate_right(input: &str, max_width: usize) -> String {
    if input.len() <= max_width {
        return input.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width == 1 {
        return "…".to_string();
    }
    let mut chars = input.chars().collect::<Vec<_>>();
    chars.truncate(max_width - 1);
    let mut output = chars.into_iter().collect::<String>();
    output.push('…');
    output
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WorkerMetrics {
    total: usize,
    doing: usize,
    reviewing: usize,
    idle: usize,
    complete: usize,
    failed: usize,
}

#[derive(Debug, Clone, Copy)]
enum ParsedBacklogPriority {
    P0,
    P1,
    P2,
}

#[derive(Debug, Clone)]
struct ParsedBacklogItem {
    priority: ParsedBacklogPriority,
    title: String,
}

impl WorkerMetrics {
    fn from_app_state(workers: &[WorkerCard]) -> Self {
        let mut metrics = Self {
            total: workers.len(),
            doing: 0,
            reviewing: 0,
            idle: 0,
            complete: 0,
            failed: 0,
        };
        for worker in workers {
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

impl ParsedBacklogPriority {
}

fn parse_backlog_priority(token: &str) -> Option<ParsedBacklogPriority> {
    match token {
        "P0" | "p0" => Some(ParsedBacklogPriority::P0),
        "P1" | "p1" => Some(ParsedBacklogPriority::P1),
        "P2" | "p2" => Some(ParsedBacklogPriority::P2),
        _ => None,
    }
}

fn is_backlog_status_token(token: &str) -> bool {
    matches!(token, "INP" | "inp" | "Q" | "q")
}

fn is_short_task_id(token: &str) -> bool {
    token.len() >= 6
        && token.len() <= 12
        && token
            .chars()
            .all(|ch| ch.is_ascii_hexdigit() || ch.is_ascii_alphanumeric())
}

fn parse_backlog_item(raw: &str) -> Option<ParsedBacklogItem> {
    let tokens = raw.split_whitespace().collect::<Vec<_>>();
    if tokens.is_empty() {
        return None;
    }
    let mut idx = 0;
    if is_backlog_status_token(tokens[idx]) {
        idx += 1;
    }
    if idx >= tokens.len() {
        return None;
    }
    let priority = parse_backlog_priority(tokens[idx])?;
    idx += 1;
    if idx >= tokens.len() {
        return None;
    }
    if tokens.len() >= idx + 2 && is_short_task_id(tokens[idx]) {
        idx += 1;
    }
    let title = tokens[idx..].join(" ");
    if title.is_empty() {
        None
    } else {
        Some(ParsedBacklogItem {
            priority,
            title,
        })
    }
}

fn ordered_backlog_items(in_progress: &[String], queued: &[String]) -> Vec<ParsedBacklogItem> {
    let mut p0 = Vec::new();
    let mut p1 = Vec::new();
    let mut p2 = Vec::new();

    for raw in in_progress.iter().chain(queued.iter()) {
        if let Some(item) = parse_backlog_item(raw) {
            match item.priority {
                ParsedBacklogPriority::P0 => p0.push(item),
                ParsedBacklogPriority::P1 => p1.push(item),
                ParsedBacklogPriority::P2 => p2.push(item),
            }
        }
    }

    let mut ordered = Vec::new();
    ordered.extend(p0);
    ordered.extend(p1);
    ordered.extend(p2);
    ordered
}

fn parse_triage_artifact(line: &str) -> TriageArtifact {
    if let Some((label, value)) = line.split_once(':') {
        TriageArtifact {
            label: label.trim().to_string(),
            value: value.trim().to_string(),
        }
    } else if let Some((label, value)) = line.split_once('=') {
        TriageArtifact {
            label: label.trim().to_string(),
            value: value.trim().to_string(),
        }
    } else {
        TriageArtifact {
            label: "Artifact".to_string(),
            value: line.to_string(),
        }
    }
}

fn now_hhmmss() -> String {
    let timestamp = now_unix_millis() % 86_400_000;
    let secs = (timestamp / 1000) as u64;
    let in_day = secs % 86_400;
    format!(
        "{:02}:{:02}:{:02}",
        in_day / 3600,
        (in_day % 3600) / 60,
        in_day % 60
    )
}

fn run_context_summary() -> (String, String) {
    let run_id = current_run_id().unwrap_or_else(|| "none".to_string());
    let run_log_path = current_run_log_path()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    (truncate_right(&run_id, 28), run_log_path)
}

fn worker_ids_summary(workers: &[WorkerRow]) -> String {
    if workers.is_empty() {
        return "none".to_string();
    }
    workers
        .iter()
        .map(|worker| worker.worker_id.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn equipment_name_for_worker(index: usize, worker_id: &str) -> String {
    if worker_id.is_empty() {
        return WORKER_EQUIPMENT_NAMES[index % WORKER_EQUIPMENT_NAMES.len()].to_string();
    }
    if index < WORKER_EQUIPMENT_NAMES.len() {
        return WORKER_EQUIPMENT_NAMES[index].to_string();
    }
    let mut acc = 0u64;
    for ch in worker_id.bytes() {
        acc = acc.wrapping_mul(31).wrapping_add(ch as u64);
    }
    WORKER_EQUIPMENT_NAMES[(acc as usize) % WORKER_EQUIPMENT_NAMES.len()].to_string()
}

#[derive(Debug, Clone, Copy)]
struct StartupHeadlineView {
    spinner_frame: usize,
    startup_active: bool,
    ellipsis_phase: u8,
    verb_idx: usize,
}

impl StartupHeadlineView {
    fn from_tick(tick: u32, verb_idx: usize) -> Self {
        let max_tick = STARTUP_SPINNER_TICKS.saturating_sub(1);
        let startup_active = tick < STARTUP_SPINNER_TICKS;
        let spinner_tick = if startup_active { tick } else { max_tick };
        Self {
            spinner_frame: (spinner_tick as usize) % STARTUP_SPINNER_FRAMES.len(),
            startup_active,
            ellipsis_phase: ((tick / 3) % 3) as u8,
            verb_idx: verb_idx % STARTUP_VERBS.len(),
        }
    }

    fn from_elapsed_ms(elapsed_ms: u128, verb_idx: usize) -> Self {
        let spinner_tick = (elapsed_ms / STARTUP_SPINNER_TICK_MS) as u32;
        Self {
            ellipsis_phase: ((elapsed_ms / STARTUP_ELLIPSIS_TICK_MS) % 3) as u8,
            ..Self::from_tick(spinner_tick, verb_idx)
        }
    }

    fn spinner(self) -> &'static str {
        STARTUP_SPINNER_FRAMES[self.spinner_frame]
    }

    fn verb(self) -> &'static str {
        STARTUP_VERBS[self.verb_idx]
    }

    fn ellipsis(self) -> &'static str {
        match self.ellipsis_phase {
            0 => ".",
            1 => "..",
            _ => "...",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct LiveStartupHeadlineState {
    started_at_ms: u128,
    verb_idx: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProblemClass {
    Healthy,
    Stalled,
    Zombie,
}

pub fn classify_problem(
    worker: &WorkerRow,
    heartbeat_interval_seconds: u64,
    lease_timeout_seconds: u64,
) -> ProblemClass {
    if worker.session_missing && worker.lease_held {
        return ProblemClass::Zombie;
    }

    if worker.last_heartbeat_secs > lease_timeout_seconds {
        return ProblemClass::Zombie;
    }

    if worker.last_heartbeat_secs > heartbeat_interval_seconds.saturating_mul(2) {
        return ProblemClass::Stalled;
    }

    ProblemClass::Healthy
}

pub fn render_dashboard(
    workers: &[WorkerRow],
    stats: &QueueStats,
    backlog: &BacklogView,
    width: u16,
    height: u16,
) -> String {
    render_dashboard_with_headline(
        workers,
        stats,
        backlog,
        width,
        height,
        StartupHeadlineView::from_tick(0, 0),
    )
}

fn render_dashboard_with_headline(
    workers: &[WorkerRow],
    stats: &QueueStats,
    backlog: &BacklogView,
    width: u16,
    height: u16,
    startup_headline: StartupHeadlineView,
) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| {
            draw_dashboard_frame(frame, workers, stats, backlog, 15, 900, startup_headline)
        })
        .expect("draw");

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

pub fn render_triage(activity: &[String], artifacts: &[String], width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| draw_triage_frame(frame, activity, artifacts))
        .expect("draw");

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
fn render_dashboard_at_tick(
    workers: &[WorkerRow],
    stats: &QueueStats,
    backlog: &BacklogView,
    width: u16,
    height: u16,
    tick: u32,
    verb_idx: usize,
) -> String {
    render_dashboard_with_headline(
        workers,
        stats,
        backlog,
        width,
        height,
        StartupHeadlineView::from_tick(tick, verb_idx),
    )
}

fn draw_dashboard_frame(
    frame: &mut ratatui::Frame<'_>,
    workers: &[WorkerRow],
    stats: &QueueStats,
    backlog: &BacklogView,
    heartbeat_interval_seconds: u64,
    lease_timeout_seconds: u64,
    startup_headline: StartupHeadlineView,
) {
    let mut app_state = AppState::from_dashboard_feed(
        workers,
        backlog,
        StartupHeadline::from_view(startup_headline),
    );
    let viewport = frame.area();
    app_state.terminal_width = viewport.width;
    app_state.terminal_height = viewport.height;
    let human_problems = workers
        .iter()
        .filter_map(|row| {
            let class = classify_problem(row, heartbeat_interval_seconds, lease_timeout_seconds);
            if !requires_human_attention(class) {
                return None;
            }
            Some(describe_problem_for_human(row, class))
        })
        .collect::<Vec<_>>();
    let has_human_problems = !human_problems.is_empty();

    let layout_constraints = if has_human_problems {
        vec![
            Constraint::Length(3),
            Constraint::Min(11),
            Constraint::Length(5),
            Constraint::Length(2),
        ]
    } else {
        vec![
            Constraint::Length(3),
            Constraint::Min(16),
            Constraint::Length(2),
        ]
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(layout_constraints)
        .split(frame.area());
    let body_height = chunks[1].height;
    let now_rows: u16 = 7;
    let remaining = body_height.saturating_sub(now_rows);
    let backlog_reserve = if app_state.terminal_width >= 120 { 6 } else { 8 };
    let requested_backlog_rows = if remaining > backlog_reserve {
        remaining - backlog_reserve
    } else {
        1
    };
    let mut backlog_rows = requested_backlog_rows;
    if app_state.workers.len() >= 3 {
        let minimum_worker_rows = (3 * WORKER_LIST_ROW_HEIGHT) as u16;
        let max_backlog_rows = remaining.saturating_sub(minimum_worker_rows);
        let minimum_backlog_rows = if app_state.backlog.is_empty() {
            0
        } else if remaining > minimum_worker_rows {
            1
        } else {
            0
        };
        backlog_rows = requested_backlog_rows
            .min(max_backlog_rows)
            .max(minimum_backlog_rows);
    }
    let workers_rows = remaining.saturating_sub(backlog_rows).max(1);
    let body = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(now_rows),
            Constraint::Length(workers_rows),
            Constraint::Length(backlog_rows),
        ])
        .split(chunks[1]);

    let summary = Paragraph::new(Line::from(vec![
        Span::styled(
            "GARDENER ",
            Style::default()
                .fg(Color::Rgb(85, 198, 255))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "live queue  ",
            Style::default().fg(Color::Rgb(170, 178, 210)),
        ),
        Span::styled("ready ", Style::default().fg(Color::Cyan)),
        Span::raw(format!("{}  ", stats.ready)),
        Span::styled("active ", Style::default().fg(Color::Yellow)),
        Span::raw(format!("{}  ", stats.active)),
        Span::styled("failed ", Style::default().fg(Color::Red)),
        Span::raw(format!("{}   ", stats.failed)),
        Span::styled(
            "P0",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(" {}  ", stats.p0)),
        Span::styled(
            "P1",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(" {}  ", stats.p1)),
        Span::styled(
            "P2",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(" {}", stats.p2)),
    ]))
    .block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
    );
    frame.render_widget(summary, chunks[0]);

    let metrics = WorkerMetrics::from_app_state(&app_state.workers);
    let (run_id, run_log_path) = run_context_summary();
    let worker_ids = worker_ids_summary(workers);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![Span::styled(
                "Now",
                Style::default().fg(Color::Gray).add_modifier(Modifier::BOLD),
            )]),
            Line::from(vec![
                Span::styled(
                    startup_headline.spinner(),
                    Style::default()
                        .fg(Color::Rgb(85, 198, 255))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(
                    startup_headline.verb(),
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
                ),
                Span::styled(startup_headline.ellipsis(), Style::default().fg(Color::White)),
            ]),
            Line::from(Span::styled(
                "Working the queue in priority order and showing exactly what each worker is doing.",
                Style::default().fg(Color::Gray),
            )),
            Line::from(vec![
                Span::styled(
                    format!("{} ", metrics.total),
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
                ),
                Span::styled("parallel workers  ", Style::default().fg(Color::Gray)),
                Span::styled(
                    format!("{} ", metrics.doing),
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                ),
                Span::styled("doing  ", Style::default().fg(Color::Gray)),
                Span::styled(
                    format!("{} ", metrics.reviewing),
                    Style::default()
                        .fg(Color::Rgb(85, 198, 255))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("reviewing  ", Style::default().fg(Color::Gray)),
                Span::styled(
                    format!("{} ", metrics.idle),
                    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                ),
                Span::styled("idle  ", Style::default().fg(Color::Gray)),
                Span::styled(
                    format!("{} ", metrics.complete),
                    Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD),
                ),
                Span::styled("complete  ", Style::default().fg(Color::Gray)),
                Span::styled(
                    format!("{} ", metrics.failed),
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
                Span::styled("failed", Style::default().fg(Color::Gray)),
            ]),
            Line::from(vec![
                Span::styled("Workers: ", Style::default().fg(Color::Gray)),
                Span::raw(truncate_right(&worker_ids, 52)),
            ]),
            Line::from(vec![
                Span::styled("Run: ", Style::default().fg(Color::Gray)),
                Span::raw(run_id),
                Span::styled(" | Log: ", Style::default().fg(Color::Gray)),
                Span::raw(truncate_right(&run_log_path, 72)),
            ]),
        ])
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(if startup_headline.startup_active {
                    Color::Rgb(85, 198, 255)
                } else {
                    Color::Rgb(82, 88, 126)
                })),
        ),
        body[0],
    );
    let workers_panel = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(body[1]);
    let viewport_height = workers_panel[1]
        .height
        .min(frame.area().height.saturating_sub(12).max(1));
    let worker_row_capacity = (viewport_height as usize / WORKER_LIST_ROW_HEIGHT).max(1);
    let max_worker_offset = app_state.workers.len().saturating_sub(worker_row_capacity);
    WORKERS_VIEWPORT_CAPACITY.with(|cell| {
        *cell.borrow_mut() = worker_row_capacity;
    });
    WORKERS_TOTAL_COUNT.with(|cell| {
        *cell.borrow_mut() = app_state.workers.len();
    });
    let selected_worker = WORKERS_VIEWPORT_SELECTED.with(|cell| {
        let mut selected = cell.borrow_mut();
        if app_state.workers.is_empty() {
            *selected = 0;
        } else {
            *selected = (*selected).min(app_state.workers.len() - 1);
        }
        *selected
    });
    let worker_offset = WORKERS_VIEWPORT_OFFSET.with(|cell| {
        let mut offset = cell.borrow_mut();
        if selected_worker < *offset {
            *offset = selected_worker;
        } else if selected_worker >= offset.saturating_add(worker_row_capacity) {
            *offset = selected_worker + 1 - worker_row_capacity;
        }
        if *offset > max_worker_offset {
            *offset = max_worker_offset;
        }
        *offset
    });
    let worker_end = (worker_offset + worker_row_capacity).min(app_state.workers.len());

    let command_stream_max_width =
        workers_panel[1]
            .width
            .saturating_sub(8 + "Commands: ".len() as u16) as usize;
    let worker_items = app_state
        .workers
        .iter()
        .enumerate()
        .skip(worker_offset)
        .take(worker_row_capacity)
        .map(|(idx, row)| {
            let selected = idx == selected_worker;
            let marker = if selected { ">" } else { " " };
            let worker_style = if selected {
                Style::default()
                    .fg(Color::Rgb(126, 231, 135))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            };
            let flow_line = worker_flow_chain_spans(&row.state);
            let command_stream = worker_command_stream(&row.command_details);
            let command_stream = truncate_right(&command_stream, command_stream_max_width);
            let mut flow_spans = Vec::new();
            flow_spans.push(Span::raw("    "));
            flow_spans.push(Span::styled(
                "Flow: ",
                Style::default().fg(Color::Blue),
            ));
            flow_spans.extend(flow_line);
            let lines = vec![
                Line::from(vec![
                    Span::styled(format!("{} {:<3}", marker, row.name), worker_style),
                    Span::raw(": "),
                    Span::raw(row.task.clone()),
                ]),
                Line::from(flow_spans),
                Line::from(vec![
                    Span::raw("    "),
                    Span::styled("Commands: ", Style::default().fg(Color::Blue)),
                    Span::styled(
                        command_stream,
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::DIM),
                    ),
                ]),
            ];
            ListItem::new(lines)
        })
        .collect::<Vec<_>>();

    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            if app_state.workers.len() > worker_row_capacity {
                format!(
                    "Workers ({:02}-{:02}/{:02})",
                    worker_offset + 1,
                    worker_end,
                    app_state.workers.len()
                )
            } else {
                "Workers".to_string()
            },
            Style::default()
                .fg(Color::Rgb(126, 231, 135))
                .add_modifier(Modifier::BOLD),
        )])),
        workers_panel[0],
    );
    frame.render_widget(
        List::new(worker_items),
        workers_panel[1],
    );

    let ordered_backlog = app_state.backlog;
    let content_capacity = body[2].height.saturating_sub(2) as usize;
    let max_visible = if content_capacity == 0 {
        0
    } else if ordered_backlog.len() > content_capacity {
        content_capacity.saturating_sub(1)
    } else {
        ordered_backlog.len()
    };
    let mut list_items = Vec::new();
    for item in ordered_backlog.iter().take(max_visible) {
        let badge = match item.priority {
            BacklogPriority::P0 => "P0",
            BacklogPriority::P1 => "P1",
            BacklogPriority::P2 => "P2",
        };
        list_items.push(ListItem::new(Line::from(vec![
            Span::styled(format!("{badge: <2}"), item.priority.span_style()),
            Span::raw(" "),
            Span::raw(item.title.clone()),
        ])));
    }
    if ordered_backlog.len() > max_visible && content_capacity > 0 {
        let hidden = ordered_backlog.len().saturating_sub(max_visible);
        list_items.push(ListItem::new(format!("... and {hidden} more")));
    }
    if list_items.is_empty() {
        list_items.push(ListItem::new("No backlog items"));
    }

    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            "BACKLOG (PRIORITY ORDER)",
            Style::default()
                .fg(Color::Rgb(245, 196, 95))
                .add_modifier(Modifier::BOLD),
        )])),
        body[2],
    );
    let backlog_panel = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(body[2]);
    frame.render_widget(List::new(list_items), backlog_panel[1]);

    if has_human_problems {
        frame.render_widget(
            Paragraph::new(human_problems.join("\n")).block(
                Block::default()
                    .title(Line::from(vec![Span::styled(
                        "Problems Requiring Human",
                        Style::default()
                            .fg(Color::Rgb(255, 125, 125))
                            .add_modifier(Modifier::BOLD),
                    )]))
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
            ),
            chunks[2],
        );
    }

    let controls_legend =
        if workers.len() == 1 && workers[0].worker_id == "boot" && workers[0].state == "init" {
            "Controls: startup in progress; hotkeys activate in WORKING stage".to_string()
        } else {
            dashboard_controls_legend()
        };
    let footer = Paragraph::new(controls_legend).block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
    );
    frame.render_widget(footer, chunks[chunks.len() - 1]);
}

fn draw_triage_frame(frame: &mut ratatui::Frame<'_>, activity: &[String], artifacts: &[String]) {
    let mut app_state = AppState::from_triage_feed(
        activity,
        artifacts,
        StartupHeadline {
            spinner_frame: 0,
            verb: "Triage".to_string(),
            startup_active: false,
            ellipsis_phase: 0,
        },
    );
    let viewport = frame.area();
    app_state.terminal_width = viewport.width;
    app_state.terminal_height = viewport.height;
    draw_triage_frame_from_state(frame, &app_state);
}

fn draw_triage_frame_from_state(frame: &mut ratatui::Frame<'_>, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(2),
        ])
        .split(frame.area());
    let triage_narrow = state.terminal_width < 80;
    let body = Layout::default()
        .direction(if triage_narrow {
            Direction::Vertical
        } else {
            Direction::Horizontal
        })
        .constraints(if triage_narrow {
            [Constraint::Min(1), Constraint::Min(1)]
        } else {
            [Constraint::Percentage(62), Constraint::Percentage(38)]
        })
        .split(chunks[1]);

    let summary = Paragraph::new(Line::from(vec![
        Span::styled(
            "GARDENER ",
            Style::default()
                .fg(Color::Rgb(85, 198, 255))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "triage mode",
            Style::default()
                .fg(Color::Rgb(245, 196, 95))
                .add_modifier(Modifier::BOLD),
        ),
    ]))
    .block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
    );
    frame.render_widget(summary, chunks[0]);

    let activity_items = if state.triage_activity.is_empty() {
        vec![ListItem::new("- waiting for triage updates")]
    } else {
        state
            .triage_activity
            .iter()
            .map(|entry| ListItem::new(format!("- {} {}", entry.timestamp, entry.message)))
            .collect::<Vec<_>>()
    };
    frame.render_widget(
        List::new(activity_items).block(
            Block::default()
                .title("Live Activity")
                .borders(Borders::RIGHT),
        ),
        body[0],
    );

    let artifact_items = if state.triage_artifacts.is_empty() {
        vec![ListItem::new("- no triage artifacts yet")]
    } else {
        state
            .triage_artifacts
            .iter()
            .map(|entry| {
                if entry.value.is_empty() {
                    ListItem::new(format!("- {}", entry.label))
                } else {
                    ListItem::new(format!("- {}: {}", entry.label, entry.value))
                }
            })
            .collect::<Vec<_>>()
    };
    frame.render_widget(
        List::new(artifact_items).block(Block::default().title("Triage Artifacts")),
        body[1],
    );

    let footer = Paragraph::new("Controls: triage in progress; follow prompts in terminal").block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
    );
    frame.render_widget(footer, chunks[2]);
}

pub fn render_report_view(path: &str, report: &str, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let lines = report
        .lines()
        .take(height.saturating_sub(8) as usize)
        .collect::<Vec<_>>()
        .join("\n");
    terminal
        .draw(|frame| draw_report_frame(frame, path, &lines))
        .expect("draw");
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

fn draw_report_frame(frame: &mut ratatui::Frame<'_>, path: &str, report_lines: &str) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(3),
        ])
        .split(frame.area());
    frame.render_widget(
        Paragraph::new("Quality report view")
            .block(Block::default().borders(Borders::ALL).title("Report")),
        chunks[0],
    );
    frame.render_widget(
        Paragraph::new(report_lines.to_string())
            .block(Block::default().borders(Borders::ALL).title(path)),
        chunks[1],
    );
    frame.render_widget(
        Paragraph::new(report_controls_legend()).block(Block::default().borders(Borders::ALL)),
        chunks[2],
    );
}

thread_local! {
    static LIVE_TUI: RefCell<Option<Terminal<CrosstermBackend<Stdout>>>> = const { RefCell::new(None) };
    static LIVE_TUI_SIZE: RefCell<Option<(u16, u16)>> = const { RefCell::new(None) };
    static LIVE_STARTUP_HEADLINE: RefCell<Option<LiveStartupHeadlineState>> = const { RefCell::new(None) };
    static WORKERS_VIEWPORT_OFFSET: RefCell<usize> = const { RefCell::new(0) };
    static WORKERS_VIEWPORT_SELECTED: RefCell<usize> = const { RefCell::new(0) };
    static WORKERS_VIEWPORT_CAPACITY: RefCell<usize> = const { RefCell::new(1) };
    static WORKERS_TOTAL_COUNT: RefCell<usize> = const { RefCell::new(0) };
}

fn now_unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn live_startup_headline() -> StartupHeadlineView {
    LIVE_STARTUP_HEADLINE.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            let now = now_unix_millis();
            *slot = Some(LiveStartupHeadlineState {
                started_at_ms: now,
                verb_idx: (now as usize) % STARTUP_VERBS.len(),
            });
        }
        let state = slot.expect("live startup headline initialized");
        let now = now_unix_millis();
        let elapsed = now.saturating_sub(state.started_at_ms);
        StartupHeadlineView::from_elapsed_ms(elapsed, state.verb_idx)
    })
}

pub fn scroll_workers_down() -> bool {
    let total = WORKERS_TOTAL_COUNT.with(|cell| *cell.borrow());
    if total == 0 {
        return false;
    }
    let capacity = WORKERS_VIEWPORT_CAPACITY.with(|cell| (*cell.borrow()).max(1));
    let moved = WORKERS_VIEWPORT_SELECTED.with(|cell| {
        let mut selected = cell.borrow_mut();
        let old = *selected;
        *selected = (*selected).min(total - 1).saturating_add(1).min(total - 1);
        *selected != old
    });
    if moved {
        let selected = WORKERS_VIEWPORT_SELECTED.with(|cell| *cell.borrow());
        WORKERS_VIEWPORT_OFFSET.with(|cell| {
            let mut offset = cell.borrow_mut();
            let max_offset = total.saturating_sub(capacity);
            if selected >= offset.saturating_add(capacity) {
                *offset = selected + 1 - capacity;
            }
            if *offset > max_offset {
                *offset = max_offset;
            }
        });
    }
    moved
}

pub fn scroll_workers_up() -> bool {
    let moved = WORKERS_VIEWPORT_SELECTED.with(|cell| {
        let mut selected = cell.borrow_mut();
        let old = *selected;
        *selected = selected.saturating_sub(1);
        *selected != old
    });
    if moved {
        let selected = WORKERS_VIEWPORT_SELECTED.with(|cell| *cell.borrow());
        WORKERS_VIEWPORT_OFFSET.with(|cell| {
            let mut offset = cell.borrow_mut();
            if selected < *offset {
                *offset = selected;
            }
        });
    }
    moved
}

pub fn reset_workers_scroll() {
    WORKERS_VIEWPORT_OFFSET.with(|cell| {
        *cell.borrow_mut() = 0;
    });
    WORKERS_VIEWPORT_SELECTED.with(|cell| {
        *cell.borrow_mut() = 0;
    });
    WORKERS_VIEWPORT_CAPACITY.with(|cell| {
        *cell.borrow_mut() = 1;
    });
    WORKERS_TOTAL_COUNT.with(|cell| {
        *cell.borrow_mut() = 0;
    });
}

pub fn draw_dashboard_live(
    workers: &[WorkerRow],
    stats: &QueueStats,
    backlog: &BacklogView,
    heartbeat_interval_seconds: u64,
    lease_timeout_seconds: u64,
) -> Result<(), GardenerError> {
    let startup_headline = live_startup_headline();
    with_live_terminal(|terminal| {
        terminal
            .draw(|frame| {
                draw_dashboard_frame(
                    frame,
                    workers,
                    stats,
                    backlog,
                    heartbeat_interval_seconds,
                    lease_timeout_seconds,
                    startup_headline,
                )
            })
            .map(|_| ())
            .map_err(|e| GardenerError::Io(e.to_string()))
    })
}

pub fn draw_report_live(path: &str, report: &str) -> Result<(), GardenerError> {
    with_live_terminal(|terminal| {
        let available_lines = terminal
            .size()
            .map_err(|e| GardenerError::Io(e.to_string()))?
            .height
            .saturating_sub(8) as usize;
        let lines = report.lines().take(available_lines).collect::<Vec<_>>().join("\n");
        terminal
            .draw(|frame| draw_report_frame(frame, path, &lines))
            .map(|_| ())
            .map_err(|e| GardenerError::Io(e.to_string()))
    })
}

pub fn draw_triage_live(activity: &[String], artifacts: &[String]) -> Result<(), GardenerError> {
    with_live_terminal(|terminal| {
        terminal
            .draw(|frame| draw_triage_frame(frame, activity, artifacts))
            .map(|_| ())
            .map_err(|e| GardenerError::Io(e.to_string()))
    })
}

pub fn draw_shutdown_screen_live(title: &str, message: &str) -> Result<(), GardenerError> {
    with_live_terminal(|terminal| {
        terminal
            .draw(|frame| draw_shutdown_frame(frame, title, message))
            .map(|_| ())
            .map_err(|e| GardenerError::Io(e.to_string()))
    })
}

fn draw_shutdown_frame(frame: &mut ratatui::Frame<'_>, title: &str, message: &str) {
    let is_error =
        title.to_ascii_lowercase().contains("error") || title.to_ascii_lowercase().contains("fail");
    let accent = if is_error {
        Color::Red
    } else {
        Color::Rgb(85, 198, 255)
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(2),
        ])
        .split(frame.area());

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "GARDENER ",
                Style::default()
                    .fg(Color::Rgb(85, 198, 255))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                title,
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ),
        ]))
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
        ),
        chunks[0],
    );

    frame.render_widget(
        Paragraph::new(message.to_string()).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(accent)),
        ),
        chunks[1],
    );

    frame.render_widget(
        Paragraph::new("Press Ctrl+C to exit").block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
        ),
        chunks[2],
    );
}

pub fn close_live_terminal() -> Result<(), GardenerError> {
    LIVE_TUI.with(|cell| -> Result<(), GardenerError> {
        let mut slot = cell.borrow_mut();
        if let Some(mut terminal) = slot.take() {
            disable_raw_mode().map_err(|e| GardenerError::Io(e.to_string()))?;
            execute!(terminal.backend_mut(), LeaveAlternateScreen)
                .map_err(|e| GardenerError::Io(e.to_string()))?;
            terminal
                .show_cursor()
                .map_err(|e| GardenerError::Io(e.to_string()))?;
        }
        Ok(())
    })?;
    LIVE_TUI_SIZE.with(|cell| {
        *cell.borrow_mut() = None;
    });
    LIVE_STARTUP_HEADLINE.with(|cell| {
        *cell.borrow_mut() = None;
    });
    Ok(())
}

fn with_live_terminal<F>(f: F) -> Result<(), GardenerError>
where
    F: FnOnce(&mut Terminal<CrosstermBackend<Stdout>>) -> Result<(), GardenerError>,
{
    LIVE_TUI.with(|cell| -> Result<(), GardenerError> {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            let mut stdout = io::stdout();
            enable_raw_mode().map_err(|e| GardenerError::Io(e.to_string()))?;
            execute!(stdout, EnterAlternateScreen).map_err(|e| GardenerError::Io(e.to_string()))?;
            let backend = CrosstermBackend::new(stdout);
            let terminal = Terminal::new(backend).map_err(|e| GardenerError::Io(e.to_string()))?;
            *slot = Some(terminal);
            let size = crossterm::terminal::size().map_err(|e| GardenerError::Io(e.to_string()))?;
            LIVE_TUI_SIZE.with(|size_cell| {
                *size_cell.borrow_mut() = Some(size);
            });
        }
        let size = crossterm::terminal::size().map_err(|e| GardenerError::Io(e.to_string()))?;
        let resized = LIVE_TUI_SIZE.with(|size_cell| {
            let mut current = size_cell.borrow_mut();
            let changed = current.map(|existing| existing != size).unwrap_or(true);
            *current = Some(size);
            changed
        });
        let terminal = slot.as_mut().expect("live terminal initialized");
        if resized {
            terminal
                .autoresize()
                .map_err(|e| GardenerError::Io(e.to_string()))?;
            terminal
                .clear()
                .map_err(|e| GardenerError::Io(e.to_string()))?;
        }
        f(terminal)
    })
}

fn requires_human_attention(class: ProblemClass) -> bool {
    matches!(class, ProblemClass::Zombie)
}

fn describe_problem_for_human(row: &WorkerRow, class: ProblemClass) -> String {
    match class {
        ProblemClass::Zombie => format!(
            "{} needs intervention: worker lease/session is inconsistent (last action: {}).",
            row.worker_id,
            humanize_action(&row.tool_line)
        ),
        ProblemClass::Stalled | ProblemClass::Healthy => String::new(),
    }
}

fn humanize_state(state: &str) -> String {
    match state {
        "init" => "Startup".to_string(),
        "backlog_sync" => "Backlog Sync".to_string(),
        "understand" => "Understand".to_string(),
        "planning" => "Planning".to_string(),
        "doing" => "Doing".to_string(),
        "gitting" => "Gitting".to_string(),
        "reviewing" => "Reviewing".to_string(),
        "merging" => "Merging".to_string(),
        "complete" => "Complete".to_string(),
        "failed" => "Failed".to_string(),
        "working" => "Working".to_string(),
        "idle" => "Idle".to_string(),
        _ => title_case_words(state),
    }
}

fn humanize_action(action: &str) -> String {
    if action.eq_ignore_ascii_case("waiting for claim") {
        return "Waiting for work".to_string();
    }
    if action.eq_ignore_ascii_case("claimed") {
        return "Task claimed".to_string();
    }
    if action.eq_ignore_ascii_case("orchestrator") {
        return "Coordinator".to_string();
    }
    if let Some(rest) = action.strip_prefix("failed: ") {
        let rest = if rest.is_empty() {
            "unknown issue".to_string()
        } else {
            rest.to_string()
        };
        return format!("Failed: {}", truncate_right(&rest, 12));
    }
    if action.starts_with("failed ") || action.starts_with("completed ") || action.starts_with("completed:") {
        return if action.starts_with("failed ") || action.starts_with("failed") {
            "Task failed".to_string()
        } else {
            "Task complete".to_string()
        };
    }
    if let Some(prompt) = action.strip_prefix("prompt ") {
        return format!("Prompt {}", prompt);
    }
    title_case_words(action)
}

fn worker_flow_chain_spans(state: &str) -> Vec<Span<'static>> {
    let current = normalize_worker_state(state);
    if current == "idle" {
        return vec![Span::styled(
            "Idle",
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM),
        )];
    }
    if current == "unknown" {
        return vec![Span::styled(
            humanize_state(state),
            Style::default().fg(Color::DarkGray),
        )];
    }

    let mut chain: Vec<&'static str> = WORKER_FLOW_STATES.to_vec();
    if current == "failed" {
        chain.push("failed");
    }
    let current_index = chain.iter().position(|step| *step == current);

    chain
        .into_iter()
        .enumerate()
        .flat_map(|(index, step)| {
            let is_current = current_index == Some(index);
            let is_after_current = if let Some(current) = current_index {
                index > current
            } else {
                false
            };
            let style = if is_current {
                if step == "failed" {
                    Style::default()
                        .fg(Color::Red)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD)
                }
            } else if is_after_current {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default().fg(Color::Gray)
            };
            let mut spans = Vec::with_capacity(2);
            if index > 0 {
                spans.push(Span::raw(" → "));
            }
            spans.push(Span::styled(humanize_state(step), style));
            spans
        })
        .collect()
}

fn worker_command_stream(commands: &[CommandEntry]) -> String {
    let recent = commands
        .iter()
        .rev()
        .take(RECENT_COMMAND_STREAM_LIMIT)
        .collect::<Vec<_>>();
    if recent.is_empty() {
        return "no recent commands".to_string();
    }

    recent
        .into_iter()
        .rev()
        .map(|entry| format!("{}  {}", entry.timestamp, entry.command))
        .collect::<Vec<_>>()
        .join("  |  ")
}

fn normalize_worker_state<'a>(state: &'a str) -> &'a str {
    match state {
        "init" | "boot" | "planning" | "backlog_sync" | "working" | "seeding" => "understand",
        "doing" | "gitting" | "reviewing" | "merging" | "complete" | "failed" | "idle" => state,
        _ => "unknown",
    }
}

fn worker_flow_summary(state: &str, tool_line: &str, breadcrumb: &str) -> String {
    if state == "failed" {
        if let Some(reason) = tool_line.strip_prefix("failed: ") {
            let details = if reason.is_empty() {
                "unknown issue".to_string()
            } else {
                truncate_right(reason, 52)
            };
            return format!("Task failed ({})", details);
        }
        if let Some(task) = tool_line.strip_prefix("failed ") {
            return format!("Task failed on {}", task);
        }
        return "Task failed".to_string();
    }

    if state == "complete" {
        return "Task complete".to_string();
    }

    if state == "reviewing" {
        return "Reviewing patch and results".to_string();
    }

    if state == "merging" {
        if let Some(prompt) = tool_line.strip_prefix("prompt ") {
            return format!("Merging changes (prompt {})", prompt);
        }
        return "Merging changes".to_string();
    }

    let base = if breadcrumb.is_empty() {
        humanize_state(state)
    } else {
        humanize_breadcrumb(breadcrumb)
    };
    if base.is_empty() {
        humanize_state(state)
    } else if let Some(prompt) = tool_line.strip_prefix("prompt ") {
        format!("{base} (prompt {prompt})")
    } else if base.len() > 56 {
        truncate_right(&base, 56)
    } else {
        base
    }
}

fn humanize_breadcrumb(path: &str) -> String {
    let parts = path
        .split('>')
        .map(str::trim)
        .filter(|step| !step.is_empty())
        .filter(|step| !step.eq_ignore_ascii_case("state"))
        .map(humanize_breadcrumb_step)
        .collect::<Vec<_>>()
        .join(" > ");
    if !parts.is_empty() {
        return parts;
    }
    if path.is_empty() {
        String::new()
    } else {
        title_case_words(path)
    }
}

fn humanize_breadcrumb_step(step: &str) -> String {
    match step {
        "claim" => "Claiming".to_string(),
        "understand" => "Understanding".to_string(),
        "planning" => "Planning".to_string(),
        "doing" => "Doing".to_string(),
        "gitting" => "Gitting".to_string(),
        "reviewing" => "Reviewing".to_string(),
        "merging" => "Merging".to_string(),
        "working" => "Working".to_string(),
        "backlog_sync" => "Backlog Sync".to_string(),
        "boot" => "Boot".to_string(),
        _ => title_case_words(step),
    }
}

fn title_case_words(raw: &str) -> String {
    let mut out = String::new();
    let mut in_word = false;
    let mut capitalize = true;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            if !in_word {
                if !out.is_empty() {
                    out.push(' ');
                }
                in_word = true;
                capitalize = true;
            }
            if capitalize {
                out.push(ch.to_ascii_uppercase());
                capitalize = false;
            } else {
                out.push(ch.to_ascii_lowercase());
            }
        } else if in_word {
            in_word = false;
        }
    }

    if out.is_empty() {
        raw.to_string()
    } else {
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoHealthWizardAnswers {
    pub preferred_parallelism: u32,
    pub validation_command: String,
    pub external_docs_accessible: bool,
    pub additional_context: String,
}

pub fn run_repo_health_wizard(
    default_validation_command: &str,
) -> Result<RepoHealthWizardAnswers, GardenerError> {
    let mut stdout = io::stdout();
    enable_raw_mode().map_err(|e| GardenerError::Io(e.to_string()))?;
    execute!(stdout, EnterAlternateScreen).map_err(|e| GardenerError::Io(e.to_string()))?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).map_err(|e| GardenerError::Io(e.to_string()))?;

    let mut step = 0usize;
    let mut parallelism_input = "3".to_string();
    let mut validation = default_validation_command.to_string();
    let mut docs_accessible = true;
    let mut notes = String::new();

    loop {
        terminal
            .draw(|frame| {
                let area = frame.area();
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3),
                        Constraint::Length(1),
                        Constraint::Min(6),
                        Constraint::Length(2),
                    ])
                    .split(area);

                let header = Paragraph::new(Line::from(vec![
                    Span::styled(
                        "GARDENER ",
                        Style::default()
                            .fg(Color::Rgb(85, 198, 255))
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        "setup wizard",
                        Style::default()
                            .fg(Color::Rgb(245, 196, 95))
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("    "),
                    Span::styled(
                        "Esc = keep defaults",
                        Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
                    ),
                ]))
                .block(
                    Block::default()
                        .borders(Borders::BOTTOM)
                        .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
                );
                frame.render_widget(header, chunks[0]);

                let steps = Paragraph::new(wizard_step_indicator(step));
                frame.render_widget(steps, chunks[1]);

                let (label, help, value) = match step {
                    0 => (
                        "Worker parallelism",
                        "How many parallel workers should gardener run? Range: 1-32.",
                        format!("> {}█", parallelism_input),
                    ),
                    1 => (
                        "Validation command",
                        "Command to verify code changes. Edit or keep the default.",
                        format!("> {}█", validation),
                    ),
                    2 => (
                        "Architecture docs available?",
                        "Are architecture/quality docs accessible in the repo? Press y/n.",
                        format!("> {}", if docs_accessible { "yes" } else { "no" }),
                    ),
                    _ => (
                        "Additional constraints (optional)",
                        "Any extra context for workers? Leave empty to skip.",
                        format!("> {}█", notes),
                    ),
                };
                let body = Paragraph::new(vec![
                    Line::from(Span::styled(
                        label,
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    )),
                    Line::from(""),
                    Line::from(Span::styled(
                        help,
                        Style::default()
                            .fg(Color::Gray)
                            .add_modifier(Modifier::DIM),
                    )),
                    Line::from(""),
                    Line::from(Span::styled(
                        value,
                        Style::default().fg(Color::Rgb(85, 198, 255)),
                    )),
                ])
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
                );
                frame.render_widget(body, chunks[2]);

                let footer = Paragraph::new(Line::from(vec![
                    Span::styled(
                        format!("Step {} of 4", step + 1),
                        Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        if step < 3 { "Enter →" } else { "Enter to finish" },
                        Style::default().fg(Color::Rgb(170, 178, 210)),
                    ),
                ]))
                .block(
                    Block::default()
                        .borders(Borders::TOP)
                        .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
                );
                frame.render_widget(footer, chunks[3]);
            })
            .map_err(|e| GardenerError::Io(e.to_string()))?;

        if let Event::Key(key) = event::read().map_err(|e| GardenerError::Io(e.to_string()))? {
            if key.code == KeyCode::Esc {
                break;
            }
            match step {
                0 => match key.code {
                    KeyCode::Enter => step = 1,
                    KeyCode::Backspace => {
                        parallelism_input.pop();
                    }
                    KeyCode::Char(c)
                        if !key.modifiers.contains(KeyModifiers::CONTROL) && c.is_ascii_digit() =>
                    {
                        parallelism_input.push(c)
                    }
                    _ => {}
                },
                1 => match key.code {
                    KeyCode::Enter => step = 2,
                    KeyCode::Backspace => {
                        validation.pop();
                    }
                    KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                        validation.push(c)
                    }
                    _ => {}
                },
                2 => match key.code {
                    KeyCode::Enter => step = 3,
                    KeyCode::Char('y') | KeyCode::Char('Y') => docs_accessible = true,
                    KeyCode::Char('n') | KeyCode::Char('N') => docs_accessible = false,
                    _ => {}
                },
                _ => match key.code {
                    KeyCode::Enter => break,
                    KeyCode::Backspace => {
                        notes.pop();
                    }
                    KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                        notes.push(c)
                    }
                    _ => {}
                },
            }
        }
    }

    teardown_terminal(terminal)?;

    let preferred_parallelism = parallelism_input
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0 && *value <= 32)
        .unwrap_or(3);

    Ok(RepoHealthWizardAnswers {
        preferred_parallelism,
        validation_command: if validation.trim().is_empty() {
            default_validation_command.to_string()
        } else {
            validation
        },
        external_docs_accessible: docs_accessible,
        additional_context: notes,
    })
}

fn teardown_terminal(
    mut terminal: Terminal<CrosstermBackend<Stdout>>,
) -> Result<(), GardenerError> {
    disable_raw_mode().map_err(|e| GardenerError::Io(e.to_string()))?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)
        .map_err(|e| GardenerError::Io(e.to_string()))?;
    terminal
        .show_cursor()
        .map_err(|e| GardenerError::Io(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{
        classify_problem, humanize_action, humanize_breadcrumb, humanize_state, render_dashboard,
        render_dashboard_at_tick, render_triage, reset_workers_scroll, scroll_workers_down,
        scroll_workers_up, AppState, ProblemClass, StageState,
        BacklogView,
        QueueStats, StartupHeadlineView, WorkerCard, WorkerMetrics, WorkerRow, WorkerState,
    };

    fn worker(heartbeat: u64, missing: bool) -> WorkerRow {
        WorkerRow {
            worker_id: "w1".to_string(),
            state: "doing".to_string(),
            task_title: "task: demo".to_string(),
            tool_line: "rg --files".to_string(),
            breadcrumb: "understand>doing".to_string(),
            last_heartbeat_secs: heartbeat,
            session_age_secs: 33,
            lease_held: true,
            session_missing: missing,
            command_details: Vec::new(),
        }
    }

    #[test]
    fn problem_classification_thresholds_are_deterministic() {
        assert_eq!(
            classify_problem(&worker(10, false), 15, 900),
            ProblemClass::Healthy
        );
        assert_eq!(
            classify_problem(&worker(31, false), 15, 900),
            ProblemClass::Stalled
        );
        assert_eq!(
            classify_problem(&worker(901, false), 15, 900),
            ProblemClass::Zombie
        );
        assert_eq!(
            classify_problem(&worker(1, true), 15, 900),
            ProblemClass::Zombie
        );
    }

    #[test]
    fn render_and_key_handling_cover_ui_branches() {
        let frame = render_dashboard(
            &[worker(10, false)],
            &QueueStats {
                ready: 1,
                active: 1,
                failed: 0,
                p0: 1,
                p1: 0,
                p2: 0,
            },
            &BacklogView {
                in_progress: vec!["P1 abc123 fix queue".to_string()],
                queued: vec!["P2 def456 tune logs".to_string()],
            },
            80,
            40,
        );
        assert!(frame.contains("GARDENER"));
        assert!(frame.contains("Now"));
        assert!(frame.contains("Scanning"));
        assert!(frame.contains("parallel workers"));
        assert!(frame.contains("Workers"));
        assert!(!frame.contains("Problems"));
        assert!(frame.contains("Flow:"));
        assert!(!frame.contains("Action:"));
        assert!(frame.contains("P0"));
        assert!(frame.contains("P2"));
        assert!(!frame.contains("status="));
        assert!(!frame.contains("action="));

        // handle_key removed — real dispatch uses hotkeys::action_for_key
    }

    #[test]
    fn backlog_rendering_is_priority_ordered() {
        let frame = render_dashboard(
            &[worker(10, false)],
            &QueueStats {
                ready: 1,
                active: 1,
                failed: 0,
                p0: 1,
                p1: 2,
                p2: 2,
            },
            &BacklogView {
                in_progress: vec![
                    "INP P1 alpha task".to_string(),
                    "INP P0 bravo task".to_string(),
                    "INP P2 charlie task".to_string(),
                ],
                queued: vec![],
            },
            80,
            40,
        );
        let backlog_section_start = frame.find("BACKLOG (PRIORITY ORDER)").expect("backlog heading");
        let backlog_section = &frame[backlog_section_start..];
        let p0 = backlog_section.find("P0").expect("p0 row");
        let p1 = backlog_section.find("P1").expect("p1 row");
        let p2 = backlog_section.find("P2").expect("p2 row");
        assert!(
            p0 < p1,
            "P0 rows should render before P1 rows in Backlog panel"
        );
        assert!(
            p1 < p2,
            "P1 rows should render before P2 rows in Backlog panel"
        );
    }

    #[test]
    fn work_now_card_freezes_spinner_after_startup() {
        let active_frame = render_dashboard_at_tick(
            &[worker(10, false)],
            &QueueStats {
                ready: 1,
                active: 1,
                failed: 0,
                p0: 0,
                p1: 1,
                p2: 0,
            },
            &BacklogView::default(),
            90,
            22,
            5,
            2,
        );
        let frozen_frame = render_dashboard_at_tick(
            &[worker(10, false)],
            &QueueStats {
                ready: 1,
                active: 1,
                failed: 0,
                p0: 0,
                p1: 1,
                p2: 0,
            },
            &BacklogView::default(),
            90,
            22,
            35,
            2,
        );
        assert!(active_frame.contains("Pruning"));
        assert!(frozen_frame.contains("Pruning"));
        assert!(frozen_frame.contains("..."));
        assert!(active_frame.contains("⠇"));
        assert!(frozen_frame.contains("⠇"));
    }

    #[test]
    fn shows_problems_only_when_human_intervention_is_needed() {
        let frame = render_dashboard(
            &[worker(901, false)],
            &QueueStats {
                ready: 0,
                active: 1,
                failed: 0,
                p0: 1,
                p1: 0,
                p2: 0,
            },
            &BacklogView::default(),
            80,
            20,
        );
        assert!(frame.contains("Problems Requiring Human"));
        assert!(frame.contains("needs intervention"));
    }

    #[test]
    fn humanized_labels_are_readable() {
        assert_eq!(humanize_state("backlog_sync"), "Backlog Sync");
        assert_eq!(humanize_action("orchestrator"), "Coordinator");
        assert_eq!(
            humanize_breadcrumb("boot>backlog_sync"),
            "Boot > Backlog Sync"
        );
        assert_eq!(humanize_breadcrumb("state>merging"), "Merging");
        assert_eq!(
            super::worker_flow_summary("merging", "prompt 42", "state>merging"),
            "Merging changes (prompt 42)"
        );
        assert_eq!(
            super::worker_flow_summary("failed", "failed: conflict applying patch", ""),
            "Task failed (conflict applying patch)"
        );
    }

    #[test]
    fn triage_mode_renders_activity_and_artifact_cards() {
        let frame = render_triage(
            &["Detecting coding agent signals".to_string()],
            &["repo-intelligence.toml (pending)".to_string()],
            80,
            20,
        );
        assert!(frame.contains("triage mode"));
        assert!(frame.contains("Live Activity"));
        assert!(frame.contains("Triage Artifacts"));
        assert!(frame.contains("Detecting coding agent signals"));
    }

    #[test]
    fn triage_stage_progress_comes_from_activity_stream() {
        let state = AppState::from_triage_feed(
            &[
                "Starting triage session".to_string(),
                "Detecting coding agent signals".to_string(),
                "Interview complete".to_string(),
            ],
            &[],
            super::StartupHeadline {
                spinner_frame: 0,
                verb: "Triage".to_string(),
                startup_active: false,
                ellipsis_phase: 0,
            },
        );
        assert_eq!(state.triage_stages[0].state, StageState::Done);
        assert_eq!(state.triage_stages[1].state, StageState::Done);
        assert_eq!(state.triage_stages[2].state, StageState::Done);
        assert_eq!(state.triage_stages[3].state, StageState::Current);
    }

    #[test]
    fn command_stream_is_rendered_per_worker() {
        let frame = render_dashboard(
            &[
                WorkerRow {
                    worker_id: "w1".to_string(),
                    state: "doing".to_string(),
                    task_title: "task one".to_string(),
                    tool_line: "tool".to_string(),
                    breadcrumb: "understand>doing".to_string(),
                    last_heartbeat_secs: 0,
                    session_age_secs: 0,
                    lease_held: true,
                    session_missing: false,
                    command_details: vec![("12:34:56".to_string(), "echo first".to_string())],
                },
                WorkerRow {
                    worker_id: "w2".to_string(),
                    state: "reviewing".to_string(),
                    task_title: "task two".to_string(),
                    tool_line: "tool".to_string(),
                    breadcrumb: "reviewing".to_string(),
                    last_heartbeat_secs: 0,
                    session_age_secs: 0,
                    lease_held: true,
                    session_missing: false,
                    command_details: vec![("23:45:01".to_string(), "echo second".to_string())],
                },
            ],
            &QueueStats {
                ready: 0,
                active: 2,
                failed: 0,
                p0: 0,
                p1: 2,
                p2: 0,
            },
            &BacklogView::default(),
            120,
            24,
        );
        assert!(frame.contains("Flow:"));
        assert!(frame.contains("12:34:56  echo first"));
        assert!(frame.contains("23:45:01  echo second"));
    }

    #[test]
    fn worker_metrics_are_derived_from_states() {
        let workers = vec![
            WorkerCard {
                name: "w1".to_string(),
                state: "doing".to_string(),
                task: String::new(),
                tool_line: String::new(),
                breadcrumb: String::new(),
                activity: Vec::new(),
                command_details: Vec::new(),
                state_bucket: WorkerState::Doing,
                last_heartbeat_secs: 0,
                lease_held: false,
                session_missing: false,
            },
            WorkerCard {
                name: "w2".to_string(),
                state: "reviewing".to_string(),
                task: String::new(),
                tool_line: String::new(),
                breadcrumb: String::new(),
                activity: Vec::new(),
                command_details: Vec::new(),
                state_bucket: WorkerState::Reviewing,
                last_heartbeat_secs: 0,
                lease_held: false,
                session_missing: false,
            },
            WorkerCard {
                name: "w3".to_string(),
                state: "idle".to_string(),
                task: String::new(),
                tool_line: String::new(),
                breadcrumb: String::new(),
                activity: Vec::new(),
                command_details: Vec::new(),
                state_bucket: WorkerState::Idle,
                last_heartbeat_secs: 0,
                lease_held: false,
                session_missing: false,
            },
        ];
        let metrics = WorkerMetrics::from_app_state(&workers);
        assert_eq!(metrics.total, 3);
        assert_eq!(metrics.doing, 1);
        assert_eq!(metrics.reviewing, 1);
        assert_eq!(metrics.idle, 1);
    }

    #[test]
    fn startup_headline_stops_after_30_ticks() {
        let running = StartupHeadlineView::from_tick(29, 0);
        let frozen = StartupHeadlineView::from_tick(35, 0);
        assert!(running.startup_active);
        assert!(!frozen.startup_active);
        assert_eq!(running.spinner(), frozen.spinner());
    }

    #[test]
    fn workers_panel_uses_scrollable_viewport() {
        reset_workers_scroll();
        let workers = (1..=9)
            .map(|idx| WorkerRow {
                worker_id: format!("w{idx}"),
                state: "doing".to_string(),
                task_title: format!("task {idx}"),
                tool_line: "tool".to_string(),
                breadcrumb: "understand>doing".to_string(),
                last_heartbeat_secs: 0,
                session_age_secs: 0,
                lease_held: true,
                session_missing: false,
                command_details: Vec::new(),
            })
            .collect::<Vec<_>>();
        let stats = QueueStats {
            ready: 0,
            active: workers.len(),
            failed: 0,
            p0: 0,
            p1: workers.len(),
            p2: 0,
        };
        let backlog = BacklogView::default();

        let initial = render_dashboard(&workers, &stats, &backlog, 80, 24);
        let strip_workers_summary = |frame: &str| {
            frame
                .lines()
                .filter(|line| !line.contains("Workers:"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let initial_without_summary = strip_workers_summary(&initial);
        assert!(initial.contains("> Lawn Mower"));
        assert!(!initial_without_summary.contains("w9 "));

        for _ in 0..10 {
            let _ = scroll_workers_down();
        }
        let scrolled = render_dashboard(&workers, &stats, &backlog, 80, 24);
        assert!(!scrolled.contains("Lawn Mower"));
        assert!(!scroll_workers_down());

        for _ in 0..10 {
            let _ = scroll_workers_up();
        }
        let reset = render_dashboard(&workers, &stats, &backlog, 80, 24);
        assert!(reset.contains("> Lawn Mower"));
        assert!(!scroll_workers_up());
    }
}
