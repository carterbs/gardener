use crate::errors::GardenerError;
use crate::hotkeys::{dashboard_controls_legend, report_controls_legend};
use crate::logging::{current_run_id, current_run_log_path};
use crate::seed_runner::SeedTask;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
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

/// Canonical quality dimension descriptions shown in the intro screen.
/// The linter in `tests/quality_dimension_linter.rs` enforces that these IDs
/// match the dimension keys used in `quality_assessment_runner.rs`.
pub const QUALITY_DIMENSIONS: &[(&str, &str)] = &[
    (
        "test_coverage",
        "Measures what fraction of source files have corresponding tests, weighting integration and e2e tests more heavily.",
    ),
    (
        "test_quality",
        "Evaluates how thorough tests are: assertion density, edge-case coverage, test isolation, and meaningful naming.",
    ),
    (
        "risk_exposure",
        "Assesses how exposed each domain is to bugs and regressions based on complexity metrics and untested code.",
    ),
    (
        "convention_adherence",
        "Checks how consistently the codebase follows its own stated conventions for style, naming, and linting.",
    ),
    (
        "agent_steering",
        "Rates the quality of AGENTS.md/CLAUDE.md steering docs: specificity, architecture pointers, and build commands.",
    ),
    (
        "mechanical_guardrails",
        "Evaluates breadth and enforcement of automated checks: linters, formatters, type checkers, and CI gates.",
    ),
    (
        "local_feedback_loop",
        "Measures how quickly developers can validate changes locally via test runners, Makefiles, and watch modes.",
    ),
    (
        "coverage_infrastructure",
        "Checks whether code coverage is measured, reported, and enforced with thresholds and CI gates.",
    ),
    (
        "documentation_quality",
        "Assesses README quality, API docs, architectural docs, inline doc density, and doc generation setup.",
    ),
];

const WORKER_LIST_ROW_HEIGHT: usize = 3;
const COMPACT_WORKER_LIST_ROW_HEIGHT: usize = 2;
const RECENT_COMMAND_STREAM_LIMIT: usize = 4;
const WORKER_FLOW_STATES: [&str; 7] = [
    "understand",
    "planning",
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
const WIZARD_STEP_LABELS: [&str; 5] = ["Parallelism", "Validation", "Docs", "Backlog", "Notes"];
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
    fn from_app_state<'a, I>(workers: I) -> Self
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

impl ParsedBacklogPriority {}

fn parse_backlog_priority(token: &str) -> Option<ParsedBacklogPriority> {
    match token {
        "P0" | "p0" => Some(ParsedBacklogPriority::P0),
        "P1" | "p1" => Some(ParsedBacklogPriority::P1),
        "P2" | "p2" => Some(ParsedBacklogPriority::P2),
        _ => None,
    }
}

fn dashboard_worker_rows_for_width(width: u16) -> u16 {
    match width {
        0..=79 => 6,
        80..=119 => 8,
        _ => 10,
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
        Some(ParsedBacklogItem { priority, title })
    }
}

fn is_in_progress_backlog_item(raw: &str) -> bool {
    raw.split_whitespace()
        .next()
        .map(|token| matches!(token, "INP" | "inp"))
        .unwrap_or(false)
}

fn parse_merge_queue_item(raw: &str) -> Option<ParsedBacklogItem> {
    let tokens = raw.split_whitespace().collect::<Vec<_>>();
    if tokens.is_empty() || tokens[0] != "MRG" {
        return None;
    }
    let mut idx = 1;
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
        Some(ParsedBacklogItem { priority, title })
    }
}

fn ordered_backlog_items(in_progress: &[String], queued: &[String]) -> Vec<ParsedBacklogItem> {
    let mut p0 = Vec::new();
    let mut p1 = Vec::new();
    let mut p2 = Vec::new();

    for raw in in_progress.iter().chain(queued.iter()) {
        if is_in_progress_backlog_item(raw) {
            continue;
        }
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

fn ordered_merge_queue_items(in_progress: &[String]) -> Vec<ParsedBacklogItem> {
    let mut p0 = Vec::new();
    let mut p1 = Vec::new();
    let mut p2 = Vec::new();

    for raw in in_progress {
        if let Some(item) = parse_merge_queue_item(raw) {
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

fn backlog_items_with_capacity(
    items: &[BacklogItem],
    content_capacity: usize,
    empty_label: &'static str,
) -> Vec<ListItem<'static>> {
    let mut rendered_items = Vec::new();
    let max_visible = if content_capacity == 0 {
        0
    } else if items.len() > content_capacity {
        content_capacity.saturating_sub(1)
    } else {
        items.len()
    };

    for item in items.iter().take(max_visible) {
        let badge = match item.priority {
            BacklogPriority::P0 => "P0",
            BacklogPriority::P1 => "P1",
            BacklogPriority::P2 => "P2",
        };
        rendered_items.push(ListItem::new(Line::from(vec![
            Span::styled(format!("{badge: <2}"), item.priority.span_style()),
            Span::raw(" "),
            Span::raw(item.title.clone()),
        ])));
    }
    if items.len() > max_visible && content_capacity > 0 {
        let hidden = items.len().saturating_sub(max_visible);
        rendered_items.push(ListItem::new(format!("... and {hidden} more")));
    }
    if rendered_items.is_empty() {
        rendered_items.push(ListItem::new(empty_label));
    }
    rendered_items
}

fn merge_worker_card_item(
    row: Option<&WorkerRow>,
    compact: bool,
    command_stream_max_width: usize,
) -> ListItem<'static> {
    let (state, task, tool_line, command_details) = row
        .map(|row| {
            (
                row.state.clone(),
                row.task_title.clone(),
                row.tool_line.clone(),
                row.command_details
                    .iter()
                    .map(|(timestamp, command)| CommandEntry {
                        timestamp: timestamp.clone(),
                        command: command.clone(),
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .unwrap_or((
            "idle".to_string(),
            "idle".to_string(),
            "idle".to_string(),
            Vec::new(),
        ));
    let flow_line = worker_flow_chain_spans(&state);
    let mut flow_spans = Vec::new();
    flow_spans.push(Span::raw("    "));
    flow_spans.push(Span::styled("Flow: ", Style::default().fg(Color::Blue)));
    flow_spans.extend(flow_line);

    let command_stream = worker_command_stream(&command_details);
    let command_stream = command_stream_window(&command_stream, command_stream_max_width);

    let worker_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let lines = if compact {
        vec![
            Line::from(vec![
                Span::styled("Merge Worker", worker_style),
                Span::raw(": "),
                Span::raw(task),
            ]),
            Line::from(vec![
                Span::styled("Action: ", Style::default().fg(Color::Blue)),
                Span::raw(tool_line),
            ]),
            Line::from(flow_spans),
        ]
    } else {
        vec![
            Line::from(vec![
                Span::styled("Merge Worker", worker_style),
                Span::raw(": "),
                Span::raw(task),
            ]),
            Line::from(vec![
                Span::styled("Action: ", Style::default().fg(Color::Blue)),
                Span::raw(tool_line),
            ]),
            Line::from(flow_spans),
            Line::from(vec![
                Span::raw("    "),
                Span::styled("Commands: ", Style::default().fg(Color::Blue)),
                Span::styled(command_stream, Style::default().fg(Color::Gray)),
            ]),
        ]
    };
    ListItem::new(lines)
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

pub fn now_hhmmss() -> String {
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

fn equipment_name_for_worker(index: usize, _worker_id: &str) -> String {
    format!("Worker {}", index + 1)
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
    let mut terminal = match Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(err) => panic!("terminal: {err}"),
    };
    terminal
        .draw(|frame| {
            draw_dashboard_frame(frame, workers, stats, backlog, 15, 900, startup_headline)
        })
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

pub fn render_triage(activity: &[String], artifacts: &[String], width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = match Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(err) => panic!("terminal: {err}"),
    };
    terminal
        .draw(|frame| draw_triage_frame(frame, activity, artifacts))
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
    _heartbeat_interval_seconds: u64,
    _lease_timeout_seconds: u64,
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
    let compact_view = app_state.terminal_width <= 80 && app_state.terminal_height <= 19;
    let compact_worker_row = app_state.terminal_width <= 80 || compact_view;
    let worker_row_height_for_layout = if compact_worker_row {
        COMPACT_WORKER_LIST_ROW_HEIGHT
    } else {
        WORKER_LIST_ROW_HEIGHT
    };
    let visible_worker_indices = workers
        .iter()
        .enumerate()
        .filter_map(|(idx, row)| {
            if row.worker_id == "merge-worker" {
                None
            } else {
                Some(idx)
            }
        })
        .collect::<Vec<_>>();
    let visible_worker_rows = visible_worker_indices
        .iter()
        .filter_map(|&idx| workers.get(idx))
        .collect::<Vec<_>>();
    let visible_worker_cards = visible_worker_indices
        .iter()
        .filter_map(|&idx| app_state.workers.get(idx))
        .collect::<Vec<_>>();
    let visible_worker_count = visible_worker_rows.len();
    let layout_constraints = vec![
        Constraint::Length(3),
        Constraint::Min(16),
        Constraint::Length(2),
    ];
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(layout_constraints)
        .split(frame.area());
    let body_height = chunks[1].height;
    let now_rows: u16 = if app_state.terminal_height <= 12 {
        3
    } else if compact_view {
        5
    } else {
        7
    };
    let remaining = body_height.saturating_sub(now_rows);
    let backlog_reserve = dashboard_worker_rows_for_width(app_state.terminal_width);
    let requested_backlog_rows = if remaining > backlog_reserve {
        remaining - backlog_reserve
    } else {
        1
    };
    let mut backlog_rows = requested_backlog_rows;
    if visible_worker_count >= 3 {
        let minimum_worker_rows =
            (visible_worker_count.min(3) * worker_row_height_for_layout + 1) as u16;
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
    } else if visible_worker_count == 0 {
        let max_backlog_rows = remaining.saturating_sub(1);
        backlog_rows = requested_backlog_rows.min(max_backlog_rows);
    }
    let backlog_half_cap = remaining / 2;
    backlog_rows = backlog_rows.min(backlog_half_cap.max(1));
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
            "unresolved ",
            Style::default().fg(Color::Rgb(214, 112, 214)),
        ),
        Span::raw(format!("{}   ", stats.unresolved)),
        Span::styled("merging ", Style::default().fg(Color::Rgb(100, 180, 255))),
        Span::raw(format!("{}   ", stats.merge_pending)),
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

    let metrics = WorkerMetrics::from_app_state(visible_worker_cards.iter().copied());
    let (run_id, run_log_path) = run_context_summary();
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
    let workers_panel = body[1];
    let viewport_cap = if compact_view {
        frame.area().height.saturating_sub(11)
    } else {
        frame.area().height.saturating_sub(12)
    };
    let viewport_height = workers_panel.height.min(viewport_cap.max(1));
    let worker_row_height = worker_row_height_for_layout;
    let worker_row_capacity = (viewport_height as usize / worker_row_height).max(1);
    let max_worker_offset = visible_worker_count.saturating_sub(worker_row_capacity);
    WORKERS_VIEWPORT_CAPACITY.with(|cell| {
        *cell.borrow_mut() = worker_row_capacity;
    });
    WORKERS_TOTAL_COUNT.with(|cell| {
        *cell.borrow_mut() = visible_worker_count;
    });
    let selected_worker = WORKERS_VIEWPORT_SELECTED.with(|cell| {
        let mut selected = cell.borrow_mut();
        if visible_worker_count == 0 {
            *selected = 0;
        } else {
            *selected = (*selected).min(visible_worker_count - 1);
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
    let command_stream_max_width = workers_panel
        .width
        .saturating_sub(8 + "Commands: ".len() as u16) as usize;
    let worker_items = visible_worker_cards
        .iter()
        .enumerate()
        .skip(worker_offset)
        .take(worker_row_capacity)
        .map(|(idx, row)| {
            let selected = idx == selected_worker;
            let marker = if selected { ">" } else { " " };
            let current_state_line = format_current_state_line(&row.state);
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
            let command_stream = command_stream_window(&command_stream, command_stream_max_width);
            let mut flow_spans = vec![
                Span::raw("    "),
                Span::styled(
                    current_state_line,
                    Style::default()
                        .fg(Color::Rgb(85, 198, 255))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled("Flow: ", Style::default().fg(Color::Blue)),
            ];
            flow_spans.extend(flow_line);
            let lines = if compact_view || compact_worker_row {
                vec![
                    Line::from(vec![
                        Span::styled(format!("{} {:<3}", marker, row.name), worker_style),
                        Span::raw(": "),
                        Span::raw(row.task.clone()),
                    ]),
                    Line::from(flow_spans),
                ]
            } else {
                vec![
                    Line::from(vec![
                        Span::styled(format!("{} {:<3}", marker, row.name), worker_style),
                        Span::raw(": "),
                        Span::raw(row.task.clone()),
                    ]),
                    Line::from(flow_spans),
                    Line::from(vec![
                        Span::raw("    "),
                        Span::styled("Commands: ", Style::default().fg(Color::Blue)),
                        Span::styled(command_stream, Style::default().fg(Color::Gray)),
                    ]),
                ]
            };
            ListItem::new(lines)
        })
        .collect::<Vec<_>>();

    frame.render_widget(List::new(worker_items), workers_panel);

    let ordered_backlog = app_state.backlog;
    let ordered_merge_queue = ordered_merge_queue_items(&backlog.in_progress)
        .into_iter()
        .map(|item| BacklogItem {
            priority: match item.priority {
                ParsedBacklogPriority::P0 => BacklogPriority::P0,
                ParsedBacklogPriority::P1 => BacklogPriority::P1,
                ParsedBacklogPriority::P2 => BacklogPriority::P2,
            },
            title: item.title,
        })
        .collect::<Vec<_>>();
    let merge_queue_panel = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(body[2]);
    let backlog_panel_frame = Block::default()
        .borders(Borders::ALL)
        .title("Backlog")
        .border_style(Style::default().fg(Color::Rgb(245, 196, 95)));
    frame.render_widget(backlog_panel_frame.clone(), merge_queue_panel[0]);
    let backlog_panel_area = backlog_panel_frame.inner(merge_queue_panel[0]);
    let backlog_list_capacity = backlog_panel_area.height.saturating_sub(2) as usize;
    let merge_row = workers.iter().find(|row| row.worker_id == "merge-worker");
    let merge_command_stream_max_width = merge_queue_panel[1]
        .width
        .saturating_sub(8 + "Commands: ".len() as u16)
        as usize;

    let backlog_items =
        backlog_items_with_capacity(&ordered_backlog, backlog_list_capacity, "No backlog items");
    let backlog_panel = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(backlog_panel_area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            "BACKLOG (PRIORITY ORDER)",
            Style::default()
                .fg(Color::Rgb(245, 196, 95))
                .add_modifier(Modifier::BOLD),
        )])),
        backlog_panel[0],
    );
    frame.render_widget(List::new(backlog_items), backlog_panel[1]);

    let merge_queue_border = Block::default()
        .borders(Borders::ALL)
        .title("Merge Queue")
        .border_style(Style::default().fg(Color::Rgb(85, 198, 255)));
    frame.render_widget(merge_queue_border.clone(), merge_queue_panel[1]);
    let merge_queue_panel_area = merge_queue_border.inner(merge_queue_panel[1]);
    let merge_right_panel = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(4),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .split(merge_queue_panel_area);
    let merge_queue_list_capacity = merge_right_panel[3].height.saturating_sub(2) as usize;
    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            "MERGE WORKER",
            Style::default()
                .fg(Color::Rgb(85, 198, 255))
                .add_modifier(Modifier::BOLD),
        )])),
        merge_right_panel[0],
    );
    let merge_worker_items = vec![merge_worker_card_item(
        merge_row,
        compact_view || compact_worker_row,
        merge_command_stream_max_width,
    )];
    let merge_worker_card = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(merge_right_panel[1]);
    frame.render_widget(List::new(merge_worker_items), merge_worker_card[1]);

    let merge_queue_items = backlog_items_with_capacity(
        &ordered_merge_queue,
        merge_queue_list_capacity,
        "No merge queue items",
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            "MERGE QUEUE",
            Style::default()
                .fg(Color::Rgb(85, 198, 255))
                .add_modifier(Modifier::BOLD),
        )])),
        merge_right_panel[2],
    );
    let merge_queue_panel_content = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(merge_right_panel[3]);
    frame.render_widget(List::new(merge_queue_items), merge_queue_panel_content[1]);

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
    let mut terminal = match Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(err) => panic!("terminal: {err}"),
    };
    terminal
        .draw(|frame| draw_report_frame(frame, path, report))
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

fn draw_report_frame(frame: &mut ratatui::Frame<'_>, path: &str, report_raw: &str) {
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

    let styled_lines = markdown_to_lines(report_raw);
    let total = styled_lines.len();
    let viewport_height = chunks[1].height.saturating_sub(2) as usize; // borders
    REPORT_TOTAL_LINES.with(|cell| {
        *cell.borrow_mut() = total;
    });
    let offset = REPORT_SCROLL_OFFSET.with(|cell| *cell.borrow());
    let visible: Vec<Line<'_>> = styled_lines
        .into_iter()
        .skip(offset)
        .take(viewport_height)
        .collect();
    frame.render_widget(
        Paragraph::new(visible).block(Block::default().borders(Borders::ALL).title(path)),
        chunks[1],
    );

    let scroll_info = if total > viewport_height {
        let end = (offset + viewport_height).min(total);
        format!(" [{}-{}/{}]", offset + 1, end, total)
    } else {
        String::new()
    };
    let legend = format!("{}{scroll_info}", report_controls_legend());
    frame.render_widget(
        Paragraph::new(legend).block(Block::default().borders(Borders::ALL)),
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
    static REPORT_SCROLL_OFFSET: RefCell<usize> = const { RefCell::new(0) };
    static REPORT_TOTAL_LINES: RefCell<usize> = const { RefCell::new(0) };
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

pub fn scroll_report_down(viewport_height: usize) -> bool {
    let total = REPORT_TOTAL_LINES.with(|cell| *cell.borrow());
    if total <= viewport_height {
        return false;
    }
    let max_offset = total.saturating_sub(viewport_height);
    REPORT_SCROLL_OFFSET.with(|cell| {
        let mut offset = cell.borrow_mut();
        if *offset >= max_offset {
            return false;
        }
        *offset += 1;
        true
    })
}

pub fn scroll_report_up() -> bool {
    REPORT_SCROLL_OFFSET.with(|cell| {
        let mut offset = cell.borrow_mut();
        if *offset == 0 {
            return false;
        }
        *offset -= 1;
        true
    })
}

pub fn reset_report_scroll() {
    REPORT_SCROLL_OFFSET.with(|cell| {
        *cell.borrow_mut() = 0;
    });
    REPORT_TOTAL_LINES.with(|cell| {
        *cell.borrow_mut() = 0;
    });
}

fn markdown_to_lines(raw: &str) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for raw_line in raw.lines() {
        if let Some(heading) = raw_line.strip_prefix("### ") {
            lines.push(Line::from(Span::styled(
                heading.to_string(),
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::DIM),
            )));
        } else if let Some(heading) = raw_line.strip_prefix("## ") {
            lines.push(Line::from(Span::styled(
                heading.to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            )));
        } else if let Some(heading) = raw_line.strip_prefix("# ") {
            lines.push(Line::from(Span::styled(
                heading.to_string(),
                Style::default()
                    .fg(Color::Rgb(85, 198, 255))
                    .add_modifier(Modifier::BOLD),
            )));
        } else if raw_line.trim() == "---" || raw_line.trim() == "***" {
            lines.push(Line::from(Span::styled(
                "─".repeat(60),
                Style::default().add_modifier(Modifier::DIM),
            )));
        } else if raw_line.trim().is_empty() {
            lines.push(Line::from(""));
        } else if raw_line.starts_with("- ") || raw_line.starts_with("* ") {
            let content = &raw_line[2..];
            let mut spans = vec![Span::raw("  ")];
            spans.push(Span::styled(
                "• ",
                Style::default().fg(Color::Rgb(82, 88, 126)),
            ));
            spans.extend(parse_inline_spans(content));
            lines.push(Line::from(spans));
        } else {
            lines.push(Line::from(parse_inline_spans(raw_line)));
        }
    }
    lines
}

fn parse_inline_spans(text: &str) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut remaining = text;
    while !remaining.is_empty() {
        if let Some(bold_start) = remaining.find("**") {
            if bold_start > 0 {
                let before = &remaining[..bold_start];
                spans.extend(parse_code_spans(before));
            }
            let after_open = &remaining[bold_start + 2..];
            if let Some(bold_end) = after_open.find("**") {
                let bold_text = &after_open[..bold_end];
                spans.push(Span::styled(
                    bold_text.to_string(),
                    Style::default().add_modifier(Modifier::BOLD),
                ));
                remaining = &after_open[bold_end + 2..];
            } else {
                spans.extend(parse_code_spans(remaining));
                break;
            }
        } else {
            spans.extend(parse_code_spans(remaining));
            break;
        }
    }
    spans
}

fn parse_code_spans(text: &str) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut remaining = text;
    while !remaining.is_empty() {
        if let Some(code_start) = remaining.find('`') {
            if code_start > 0 {
                spans.push(Span::raw(remaining[..code_start].to_string()));
            }
            let after_open = &remaining[code_start + 1..];
            if let Some(code_end) = after_open.find('`') {
                let code_text = &after_open[..code_end];
                spans.push(Span::styled(
                    code_text.to_string(),
                    Style::default().fg(Color::Rgb(200, 160, 255)),
                ));
                remaining = &after_open[code_end + 1..];
            } else {
                spans.push(Span::raw(remaining.to_string()));
                break;
            }
        } else {
            spans.push(Span::raw(remaining.to_string()));
            break;
        }
    }
    spans
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
        terminal
            .draw(|frame| draw_report_frame(frame, path, report))
            .map(|_| ())
            .map_err(|e| GardenerError::Io(e.to_string()))
    })
}

pub fn draw_quality_grading_live(activity: &[String]) -> Result<(), GardenerError> {
    with_live_terminal(|terminal| {
        terminal
            .draw(|frame| draw_quality_grading_frame(frame, activity))
            .map(|_| ())
            .map_err(|e| GardenerError::Io(e.to_string()))
    })
}

fn draw_quality_grading_frame(frame: &mut ratatui::Frame<'_>, activity: &[String]) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(2),
        ])
        .split(frame.area());

    let header = Paragraph::new(Line::from(vec![
        Span::styled(
            "GARDENER ",
            Style::default()
                .fg(Color::Rgb(85, 198, 255))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "grading your repository",
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
    frame.render_widget(header, chunks[0]);

    let activity_items = if activity.is_empty() {
        vec![ListItem::new("- waiting for quality grading updates")]
    } else {
        activity
            .iter()
            .map(|line| ListItem::new(style_activity_line(line)))
            .collect::<Vec<_>>()
    };
    frame.render_widget(
        List::new(activity_items).block(
            Block::default()
                .title("Quality Grading Activity")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
        ),
        chunks[1],
    );

    let footer =
        Paragraph::new("Quality grading in progress \u{2014} agents are assessing your repository")
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
            );
    frame.render_widget(footer, chunks[2]);
}

pub fn render_quality_grading(activity: &[String], width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = match Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(err) => panic!("terminal: {err}"),
    };
    terminal
        .draw(|frame| draw_quality_grading_frame(frame, activity))
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

/// Style an activity line for TUI display.
///
/// - Timestamp portion ("- HH:MM:SS ") is rendered in `DarkGray`.
/// - For lines containing "Agent activity:", the command portion (after the
///   last `: `) is rendered in light purple italic.
/// - Other lines use default styling for the message body.
fn style_activity_line(line: &str) -> Line<'_> {
    let timestamp = now_hhmmss();
    let body = line;

    if body.contains("Agent activity:") {
        if let Some(last_colon_pos) = body.rfind(": ") {
            let prefix = &body[..last_colon_pos + 2]; // includes ": "
            let command = &body[last_colon_pos + 2..];
            return Line::from(vec![
                Span::styled(
                    format!("- {timestamp} "),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw(prefix.to_string()),
                Span::styled(
                    command.to_string(),
                    Style::default()
                        .fg(Color::Rgb(180, 180, 220))
                        .add_modifier(Modifier::ITALIC),
                ),
            ]);
        }
    }

    Line::from(vec![
        Span::styled(
            format!("- {timestamp} "),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw(body.to_string()),
    ])
}

pub fn draw_seeding_live(activity: &[String]) -> Result<(), GardenerError> {
    with_live_terminal(|terminal| {
        terminal
            .draw(|frame| draw_seeding_frame(frame, activity))
            .map(|_| ())
            .map_err(|e| GardenerError::Io(e.to_string()))
    })
}

fn draw_seeding_frame(frame: &mut ratatui::Frame<'_>, activity: &[String]) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(2),
        ])
        .split(frame.area());

    let header = Paragraph::new(Line::from(vec![
        Span::styled(
            "GARDENER ",
            Style::default()
                .fg(Color::Rgb(85, 198, 255))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "seeding your backlog",
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
    frame.render_widget(header, chunks[0]);

    let activity_items = if activity.is_empty() {
        vec![ListItem::new("- waiting for seeding updates")]
    } else {
        activity
            .iter()
            .map(|line| ListItem::new(style_activity_line(line)))
            .collect::<Vec<_>>()
    };
    frame.render_widget(
        List::new(activity_items).block(
            Block::default()
                .title("Live Activity")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
        ),
        chunks[1],
    );

    let footer = Paragraph::new("Seeding in progress \u{2014} agent is exploring your repository")
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
        );
    frame.render_widget(footer, chunks[2]);
}

pub fn render_seeding(activity: &[String], width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = match Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(err) => panic!("terminal: {err}"),
    };
    terminal
        .draw(|frame| draw_seeding_frame(frame, activity))
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

pub fn draw_quality_intro_live() -> Result<(), GardenerError> {
    with_live_terminal(|terminal| {
        terminal
            .draw(draw_quality_intro_frame)
            .map(|_| ())
            .map_err(|e| GardenerError::Io(e.to_string()))
    })
}

fn draw_quality_intro_frame(frame: &mut ratatui::Frame<'_>) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(2),
        ])
        .split(frame.area());

    let header = Paragraph::new(Line::from(vec![
        Span::styled(
            "GARDENER ",
            Style::default()
                .fg(Color::Rgb(85, 198, 255))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "grading your repository",
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
    frame.render_widget(header, chunks[0]);

    let dimension_items: Vec<ListItem<'_>> = QUALITY_DIMENSIONS
        .iter()
        .map(|(name, desc)| {
            ListItem::new(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    *name,
                    Style::default()
                        .fg(Color::Rgb(85, 198, 255))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("  \u{2014}  ", Style::default().fg(Color::Gray)),
                Span::styled(*desc, Style::default().fg(Color::Gray)),
            ]))
        })
        .collect();

    frame.render_widget(
        List::new(dimension_items).block(
            Block::default()
                .title("Quality Dimensions")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
        ),
        chunks[1],
    );

    let footer = Paragraph::new("Agents are starting up \u{2014} assessing 9 quality dimensions")
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
        );
    frame.render_widget(footer, chunks[2]);
}

pub fn render_quality_intro(width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = match Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(err) => panic!("terminal: {err}"),
    };
    terminal
        .draw(draw_quality_intro_frame)
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

pub fn render_seed_review(
    task: &SeedTask,
    index: usize,
    total: usize,
    width: u16,
    height: u16,
) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = match Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(err) => panic!("terminal: {err}"),
    };
    terminal
        .draw(|frame| draw_seed_review_frame(frame, task, index, total, 0, None, ""))
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

fn draw_seed_review_frame(
    frame: &mut ratatui::Frame<'_>,
    task: &SeedTask,
    index: usize,
    total: usize,
    round: usize,
    input_mode: Option<&InputMode>,
    input_text: &str,
) {
    let footer_height = if input_mode.is_some() { 4 } else { 2 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(footer_height),
        ])
        .split(frame.area());

    let counter = if round == 0 {
        format!("review backlog  ({}/{})", index + 1, total)
    } else {
        format!(
            "review backlog  round {}  ({}/{})",
            round + 1,
            index + 1,
            total
        )
    };
    let header = Paragraph::new(Line::from(vec![
        Span::styled(
            "GARDENER ",
            Style::default()
                .fg(Color::Rgb(85, 198, 255))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            counter,
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
    frame.render_widget(header, chunks[0]);

    let priority_style = match task.priority.as_str() {
        "P0" => Style::default()
            .fg(Color::Rgb(255, 122, 122))
            .add_modifier(Modifier::BOLD),
        "P1" => Style::default()
            .fg(Color::Rgb(255, 207, 105))
            .add_modifier(Modifier::BOLD),
        _ => Style::default()
            .fg(Color::Rgb(127, 230, 148))
            .add_modifier(Modifier::BOLD),
    };

    let body_lines = vec![
        Line::from(Span::styled(
            task.title.clone(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            task.details.clone(),
            Style::default().fg(Color::Gray),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "Why this helps agents: ",
                Style::default()
                    .fg(Color::Rgb(85, 198, 255))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                task.rationale.clone(),
                Style::default().fg(Color::Rgb(85, 198, 255)),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Priority: ", Style::default().fg(Color::Gray)),
            Span::styled(task.priority.clone(), priority_style),
            Span::styled(
                format!("    Domain: {}", task.domain),
                Style::default().fg(Color::Gray),
            ),
        ]),
    ];

    frame.render_widget(
        Paragraph::new(body_lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
        ),
        chunks[1],
    );

    if let Some(mode) = input_mode {
        let (label, hint) = match mode {
            InputMode::DiscardReason => (
                "Why discard? (optional — Enter to skip, Esc to cancel)",
                Color::Rgb(255, 122, 122),
            ),
            InputMode::RefineFeedback => (
                "How should this task change? (Enter to submit, Esc to cancel)",
                Color::Rgb(245, 196, 95),
            ),
        };
        let footer = Paragraph::new(vec![
            Line::from(Span::styled(
                label,
                Style::default().fg(hint).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                format!("> {input_text}\u{2588}"),
                Style::default().fg(Color::White),
            )),
        ])
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
        );
        frame.render_widget(footer, chunks[2]);
    } else {
        let footer = Paragraph::new(Line::from(vec![
            Span::styled(
                "[k] Keep",
                Style::default()
                    .fg(Color::Rgb(126, 231, 135))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("    "),
            Span::styled(
                "[d] Discard",
                Style::default()
                    .fg(Color::Rgb(255, 122, 122))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("    "),
            Span::styled(
                "[r] Refine",
                Style::default()
                    .fg(Color::Rgb(245, 196, 95))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("    "),
            Span::styled(
                "[q] Discard remaining & finish",
                Style::default().fg(Color::Gray),
            ),
        ]))
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
        );
        frame.render_widget(footer, chunks[2]);
    }
}

/// Decision made for a single seed task during interactive review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewDecision {
    Keep,
    Discard(Option<String>),
    Refine(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputMode {
    DiscardReason,
    RefineFeedback,
}

pub fn run_seed_review_wizard(
    tasks: &[SeedTask],
    round: usize,
) -> Result<Vec<ReviewDecision>, GardenerError> {
    let mut stdout = io::stdout();
    enable_raw_mode().map_err(|e| GardenerError::Io(e.to_string()))?;
    execute!(stdout, EnterAlternateScreen).map_err(|e| GardenerError::Io(e.to_string()))?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).map_err(|e| GardenerError::Io(e.to_string()))?;

    let total = tasks.len();
    let mut decisions: Vec<Option<ReviewDecision>> = vec![None; total];
    let mut current = 0usize;
    let mut input_mode: Option<InputMode> = None;
    let mut input_buffer = String::new();

    loop {
        if current >= total {
            break;
        }
        terminal
            .draw(|frame| {
                draw_seed_review_frame(
                    frame,
                    &tasks[current],
                    current,
                    total,
                    round,
                    input_mode.as_ref(),
                    &input_buffer,
                )
            })
            .map_err(|e| GardenerError::Io(e.to_string()))?;

        if let Event::Key(key) = event::read().map_err(|e| GardenerError::Io(e.to_string()))? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            if let Some(mode) = input_mode {
                match key.code {
                    KeyCode::Enter => {
                        match mode {
                            InputMode::DiscardReason => {
                                let reason = if input_buffer.trim().is_empty() {
                                    None
                                } else {
                                    Some(input_buffer.clone())
                                };
                                decisions[current] = Some(ReviewDecision::Discard(reason));
                                current += 1;
                                input_mode = None;
                                input_buffer.clear();
                            }
                            InputMode::RefineFeedback => {
                                if input_buffer.trim().is_empty() {
                                    // Feedback required — stay in input mode
                                } else {
                                    decisions[current] =
                                        Some(ReviewDecision::Refine(input_buffer.clone()));
                                    current += 1;
                                    input_mode = None;
                                    input_buffer.clear();
                                }
                            }
                        }
                    }
                    KeyCode::Esc => {
                        input_mode = None;
                        input_buffer.clear();
                    }
                    KeyCode::Backspace => {
                        input_buffer.pop();
                    }
                    KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                        input_buffer.push(c);
                    }
                    _ => {}
                }
            } else {
                match key.code {
                    KeyCode::Char('k') | KeyCode::Char('K') => {
                        decisions[current] = Some(ReviewDecision::Keep);
                        current += 1;
                    }
                    KeyCode::Char('d') | KeyCode::Char('D') => {
                        input_mode = Some(InputMode::DiscardReason);
                        input_buffer.clear();
                    }
                    KeyCode::Char('r') | KeyCode::Char('R') => {
                        input_mode = Some(InputMode::RefineFeedback);
                        input_buffer.clear();
                    }
                    KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => {
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    teardown_terminal(terminal)?;

    // Convert None decisions (from early exit) to Discard(None)
    Ok(decisions
        .into_iter()
        .map(|d| d.unwrap_or(ReviewDecision::Discard(None)))
        .collect())
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

    let body_lines: Vec<Line> = message
        .lines()
        .map(|line| {
            if line.is_empty() {
                Line::from("")
            } else if line.starts_with("Tasks merged") || line.starts_with("Total runtime") {
                Line::from(Span::styled(
                    line.to_string(),
                    Style::default().fg(Color::Rgb(180, 180, 180)),
                ))
            } else if line.starts_with("Tasks failed") {
                Line::from(Span::styled(
                    line.to_string(),
                    Style::default().fg(Color::Rgb(255, 150, 100)),
                ))
            } else if line.starts_with("Tasks completed") {
                Line::from(Span::styled(
                    line.to_string(),
                    Style::default().fg(accent).add_modifier(Modifier::BOLD),
                ))
            } else {
                Line::from(line.to_string())
            }
        })
        .collect();
    frame.render_widget(
        Paragraph::new(body_lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(accent)),
        ),
        chunks[1],
    );

    frame.render_widget(
        Paragraph::new(if is_error {
            "Press Ctrl+C or c to copy the error message, then any key to exit"
        } else {
            "Press any key to exit"
        })
        .block(
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
        let terminal = slot
            .as_mut()
            .ok_or_else(|| GardenerError::Cli("live terminal initialized".to_string()))?;
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

pub(crate) fn format_state_label(state: &str) -> String {
    match state {
        "init" => "Startup".to_string(),
        "backlog_sync" => "Backlog Sync".to_string(),
        "understand" => "Understand".to_string(),
        "planning" => "Planning".to_string(),
        "claimed" => "Claimed".to_string(),
        "starting" => "Starting".to_string(),
        "worktree_preparing" => "Worktree Prep".to_string(),
        "worktree_ready" => "Worktree Ready".to_string(),
        "doing" => "Doing".to_string(),
        "commit" => "Commit".to_string(),
        "gitting" => "Gitting".to_string(),
        "gitting_remediation" => "Gitting Remediation".to_string(),
        "pr_creating" => "PR Creating".to_string(),
        "handoff" => "Merging".to_string(),
        "reviewing" => "Reviewing".to_string(),
        "merging" => "Merging".to_string(),
        "merge_lock_waiting" => "Merge Lock Wait".to_string(),
        "merge_lock_held" => "Merge Lock Held".to_string(),
        "merge_polling" => "Checking mergeability".to_string(),
        "merge_from_main" => "Updating branch with main".to_string(),
        "merge_remediation" => "Merge Remediation".to_string(),
        "post_merge_validation" => "Running post-merge checks".to_string(),
        "teardown" => "Teardown".to_string(),
        "complete" => "Complete".to_string(),
        "failed" => "Failed".to_string(),
        "parked" => "Parked".to_string(),
        "working" => "Working".to_string(),
        "idle" => "Idle".to_string(),
        _ => to_title_case_words(state),
    }
}

fn worker_flow_chain_spans(state: &str) -> Vec<Span<'static>> {
    let current = normalize_worker_state(state);
    if current == "idle" {
        return vec![Span::styled(
            "Idle",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        )];
    }
    if current == "unknown" {
        return vec![Span::styled(
            format_state_label(state),
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
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
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
            spans.push(Span::styled(format_state_label(step), style));
            spans
        })
        .collect()
}

fn format_current_state_line(state: &str) -> String {
    format!("State: {}", format_state_label(state))
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
        .map(|entry| format!("{}  {}", entry.timestamp, entry.command))
        .collect::<Vec<_>>()
        .join("  |  ")
}

fn command_stream_window(stream: &str, width: usize) -> String {
    truncate_right(stream, width)
}

fn normalize_worker_state(state: &str) -> &str {
    let normalized_state = state.trim().to_ascii_lowercase();
    let normalized_state = normalized_state
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .rfind(|part| !part.is_empty())
        .unwrap_or(normalized_state.as_str());

    match normalized_state {
        "init" | "boot" | "backlog_sync" | "working" | "seeding" => "understand",
        "claimed" | "starting" | "worktree_preparing" | "worktree_ready" => "understand",
        "commit" | "gitting_remediation" | "pr_creating" => "gitting",
        "merge_lock_waiting"
        | "ci_failure_remediation"
        | "merge_from_main"
        | "merge_lock_held"
        | "merge_polling"
        | "handoff"
        | "merge_remediation"
        | "post_merge_validation"
        | "teardown" => "merging",
        "understand" => "understand",
        "planning" => "planning",
        "doing" => "doing",
        "gitting" => "gitting",
        "reviewing" => "reviewing",
        "merging" => "merging",
        "complete" => "complete",
        "failed" => "failed",
        "unresolved" => "unresolved",
        "idle" => "idle",
        "parked" => "parked",
        _ => "unknown",
    }
}

fn format_breadcrumb(path: &str) -> String {
    let parts = path
        .split('>')
        .map(str::trim)
        .filter(|step| !step.is_empty())
        .filter(|step| !step.eq_ignore_ascii_case("state"))
        .map(format_breadcrumb_step)
        .collect::<Vec<_>>()
        .join(" > ");
    if !parts.is_empty() {
        return parts;
    }
    if path.is_empty() {
        String::new()
    } else {
        to_title_case_words(path)
    }
}

fn format_breadcrumb_step(step: &str) -> String {
    match step {
        "claim" => "Claiming".to_string(),
        "claimed" => "Claimed".to_string(),
        "starting" => "Starting".to_string(),
        "worktree_preparing" => "Preparing Worktree".to_string(),
        "worktree_ready" => "Worktree Ready".to_string(),
        "understand" => "Understanding".to_string(),
        "planning" => "Planning".to_string(),
        "doing" => "Doing".to_string(),
        "commit" => "Committing".to_string(),
        "gitting" => "Gitting".to_string(),
        "gitting_remediation" => "Gitting Remediation".to_string(),
        "pr_creating" => "Creating PR".to_string(),
        "reviewing" => "Reviewing".to_string(),
        "merging" => "Merging".to_string(),
        "merge_lock_waiting" => "Waiting For Merge Lock".to_string(),
        "merge_lock_held" => "Merge Lock Held".to_string(),
        "merge_polling" => "Polling Mergeability".to_string(),
        "merge_remediation" => "Merge Remediation".to_string(),
        "post_merge_validation" => "Post-Merge Validation".to_string(),
        "teardown" => "Teardown".to_string(),
        "parked" => "Parked".to_string(),
        "working" => "Working".to_string(),
        "backlog_sync" => "Backlog Sync".to_string(),
        "boot" => "Boot".to_string(),
        _ => to_title_case_words(step),
    }
}

fn to_title_case_words(raw: &str) -> String {
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
    pub backlog_approval: bool,
    pub additional_context: String,
}

/// Return value from wizard key handling: what the loop should do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WizardAction {
    Continue,
    Finish,
}

/// Mutable wizard state extracted for testability.
#[derive(Debug, Clone)]
struct WizardState {
    step: usize,
    parallelism_input: String,
    validation: String,
    docs_accessible: bool,
    backlog_approval: bool,
    notes: String,
}

impl WizardState {
    fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> WizardAction {
        if code == KeyCode::Esc {
            return WizardAction::Finish;
        }
        match self.step {
            0 => match code {
                KeyCode::Enter => self.step = 1,
                KeyCode::Backspace => {
                    self.parallelism_input.pop();
                }
                KeyCode::Char(c)
                    if !modifiers.contains(KeyModifiers::CONTROL) && c.is_ascii_digit() =>
                {
                    self.parallelism_input.push(c)
                }
                _ => {}
            },
            1 => match code {
                KeyCode::Enter => self.step = 2,
                KeyCode::Backspace => {
                    self.validation.pop();
                }
                KeyCode::Char(c) if !modifiers.contains(KeyModifiers::CONTROL) => {
                    self.validation.push(c)
                }
                _ => {}
            },
            2 => match code {
                KeyCode::Enter => self.step = 3,
                KeyCode::Char('y') | KeyCode::Char('Y') => self.docs_accessible = true,
                KeyCode::Char('n') | KeyCode::Char('N') => self.docs_accessible = false,
                _ => {}
            },
            3 => match code {
                KeyCode::Enter => self.step = 4,
                KeyCode::Char('a') | KeyCode::Char('A') => {
                    self.backlog_approval = false;
                    self.step = 4;
                }
                KeyCode::Char('r') | KeyCode::Char('R') => {
                    self.backlog_approval = true;
                    self.step = 4;
                }
                KeyCode::Up | KeyCode::Down | KeyCode::Tab => {
                    self.backlog_approval = !self.backlog_approval
                }
                _ => {}
            },
            _ => match code {
                KeyCode::Enter => return WizardAction::Finish,
                KeyCode::Backspace => {
                    self.notes.pop();
                }
                KeyCode::Char(c) if !modifiers.contains(KeyModifiers::CONTROL) => {
                    self.notes.push(c)
                }
                _ => {}
            },
        }
        WizardAction::Continue
    }
}

pub fn run_repo_health_wizard(
    default_validation_command: &str,
) -> Result<RepoHealthWizardAnswers, GardenerError> {
    let mut stdout = io::stdout();
    enable_raw_mode().map_err(|e| GardenerError::Io(e.to_string()))?;
    execute!(stdout, EnterAlternateScreen).map_err(|e| GardenerError::Io(e.to_string()))?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).map_err(|e| GardenerError::Io(e.to_string()))?;

    let mut ws = WizardState {
        step: 0,
        parallelism_input: "3".to_string(),
        validation: default_validation_command.to_string(),
        docs_accessible: true,
        backlog_approval: true,
        notes: String::new(),
    };

    loop {
        let step = ws.step;
        let parallelism_input = &ws.parallelism_input;
        let validation = &ws.validation;
        let docs_accessible = ws.docs_accessible;
        let backlog_approval = ws.backlog_approval;
        let notes = &ws.notes;
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
                    3 => (
                        "Backlog seeding",
                        "Gardener seeds a backlog of tasks that make your repo more hospitable to coding agents.",
                        String::new(), // options rendered separately below
                    ),
                    _ => (
                        "Additional constraints (optional)",
                        "Any extra context for workers? Leave empty to skip.",
                        format!("> {}█", notes),
                    ),
                };
                let selected_style = Style::default()
                    .fg(Color::Rgb(85, 198, 255))
                    .add_modifier(Modifier::BOLD);
                let unselected_style = Style::default()
                    .fg(Color::DarkGray);
                let mut body_lines = vec![
                    Line::from(Span::styled(
                        label,
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    )),
                    Line::from(""),
                    Line::from(Span::styled(
                        help,
                        Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
                    )),
                    Line::from(""),
                ];
                if step == 3 {
                    let (review_dot, review_style) = if backlog_approval {
                        ("● ", selected_style)
                    } else {
                        ("○ ", unselected_style)
                    };
                    let (auto_dot, auto_style) = if !backlog_approval {
                        ("● ", selected_style)
                    } else {
                        ("○ ", unselected_style)
                    };
                    body_lines.push(Line::from(vec![
                        Span::styled(review_dot, review_style),
                        Span::styled("review tasks", review_style),
                        Span::styled("  (r)", Style::default().fg(Color::DarkGray)),
                    ]));
                    body_lines.push(Line::from(""));
                    body_lines.push(Line::from(vec![
                        Span::styled(auto_dot, auto_style),
                        Span::styled("auto-seed", auto_style),
                        Span::styled("  (a)", Style::default().fg(Color::DarkGray)),
                    ]));
                } else {
                    body_lines.push(Line::from(Span::styled(
                        value,
                        Style::default().fg(Color::Rgb(85, 198, 255)),
                    )));
                }
                let body = Paragraph::new(body_lines)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
                );
                frame.render_widget(body, chunks[2]);

                let footer = Paragraph::new(Line::from(vec![
                    Span::styled(
                        format!("Step {} of 5", step + 1),
                        Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        if step < 4 {
                            "Enter →"
                        } else {
                            "Enter to finish"
                        },
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
            if key.kind != KeyEventKind::Press {
                continue;
            }
            if ws.handle_key(key.code, key.modifiers) == WizardAction::Finish {
                break;
            }
        }
    }

    teardown_terminal(terminal)?;

    let preferred_parallelism = ws
        .parallelism_input
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0 && *value <= 32)
        .unwrap_or(3);

    Ok(RepoHealthWizardAnswers {
        preferred_parallelism,
        validation_command: if ws.validation.trim().is_empty() {
            default_validation_command.to_string()
        } else {
            ws.validation
        },
        external_docs_accessible: ws.docs_accessible,
        backlog_approval: ws.backlog_approval,
        additional_context: ws.notes,
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
        command_stream_window, format_breadcrumb, format_state_label, render_dashboard,
        render_dashboard_at_tick, render_triage, reset_workers_scroll, scroll_workers_down,
        scroll_workers_up, worker_command_stream, worker_flow_chain_spans, AppState, BacklogView,
        CommandEntry, QueueStats, StageState, StartupHeadlineView, WorkerCard, WorkerMetrics,
        WorkerRow, WorkerState,
    };

    fn worker(heartbeat: u64, missing: bool) -> WorkerRow {
        WorkerRow {
            worker_id: "w1".to_string(),
            state: "doing".to_string(),
            task_id: None,
            last_state_line: 0,
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
    fn render_and_key_handling_cover_ui_branches() {
        let frame = render_dashboard(
            &[worker(10, false)],
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
            &BacklogView {
                in_progress: vec!["INP P1 fix queue".to_string()],
                queued: vec![
                    "P0 abc123 queued task".to_string(),
                    "P2 def456 tune logs".to_string(),
                ],
            },
            80,
            40,
        );
        assert!(frame.contains("GARDENER"));
        assert!(frame.contains("Now"));
        assert!(frame.contains("Scanning"));
        assert!(frame.contains("parallel workers"));
        assert!(!frame.contains("Workers:"));
        assert!(!frame.contains("Problems"));
        assert!(frame.contains("Flow:"));
        assert!(frame.contains("Action:"));
        assert!(frame.contains("P0"));
        assert!(frame.contains("P2"));
        assert!(!frame.contains("fix queue"));
        assert!(!frame.contains("status="));
        assert!(!frame.contains("action="));
    }

    #[test]
    fn backlog_rendering_is_priority_ordered() {
        let frame = render_dashboard(
            &[worker(10, false)],
            &QueueStats {
                ready: 1,
                active: 1,
                failed: 0,
                unresolved: 0,
                merge_pending: 0,
                p0: 1,
                p1: 2,
                p2: 2,
            },
            &BacklogView {
                in_progress: vec![
                    "INP P1 active task should be omitted".to_string(),
                    "INP P0 bravo task".to_string(),
                    "INP P2 charlie task".to_string(),
                ],
                queued: vec![
                    "P0 abc123 queued p0".to_string(),
                    "P1 def456 queued p1".to_string(),
                    "P2 feed00 queued p2".to_string(),
                ],
            },
            80,
            40,
        );
        let backlog_section_start = frame
            .find("BACKLOG (PRIORITY ORDER)")
            .expect("backlog heading");
        let backlog_section = &frame[backlog_section_start..];
        let p0 = backlog_section.find("queued p0").expect("p0 row");
        let p1 = backlog_section.find("queued p1").expect("p1 row");
        let p2 = backlog_section.find("queued p2").expect("p2 row");
        assert!(
            p0 < p2,
            "P0 rows should render before P2 rows in Backlog panel"
        );
        assert!(
            p0 < p1,
            "P0 rows should render before P1 rows in Backlog panel"
        );
        assert!(
            !backlog_section.contains("active task should be omitted"),
            "INP items should be excluded from backlog panel"
        );
        assert!(
            !backlog_section.contains("bravo task"),
            "INP items should be excluded from backlog panel"
        );
    }

    #[test]
    fn backlog_excludes_in_progress_tasks() {
        let frame = render_dashboard(
            &[worker(10, false)],
            &QueueStats {
                ready: 1,
                active: 1,
                failed: 0,
                unresolved: 0,
                merge_pending: 0,
                p0: 1,
                p1: 1,
                p2: 0,
            },
            &BacklogView {
                in_progress: vec!["INP P1 5d8c91 active task".to_string()],
                queued: vec!["P0 abc123 queued task".to_string()],
            },
            120,
            30,
        );
        assert!(frame.contains("queued task"));
        assert!(!frame.contains("P1 active task"));
    }

    #[test]
    fn dashboard_panes_render_with_borders() {
        let frame = render_dashboard(
            &[worker(10, false)],
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
            &BacklogView {
                in_progress: vec!["P1 abc123 fix queue".to_string()],
                queued: vec!["P2 def456 tune logs".to_string()],
            },
            120,
            30,
        );
        let border_chars = |line: &str| {
            line.chars().any(|ch| {
                matches!(
                    ch,
                    '─' | '│'
                        | '┌'
                        | '┐'
                        | '└'
                        | '┘'
                        | '╭'
                        | '╮'
                        | '╰'
                        | '╯'
                        | '+'
                        | '┬'
                        | '┴'
                        | '├'
                        | '┤'
                )
            })
        };
        let has_title_with_border = |frame: &str, title: &str| {
            frame
                .lines()
                .any(|line| line.contains(title) && border_chars(line))
        };
        let top_left_corners =
            frame.matches('┌').count() + frame.matches('╭').count() + frame.matches('+').count();
        let top_right_corners =
            frame.matches('┐').count() + frame.matches('╮').count() + frame.matches('+').count();
        assert!(
            top_left_corners >= 3,
            "expected now/backlog/merge queue borders"
        );
        assert!(
            top_right_corners >= 3,
            "expected now/backlog/merge queue/nows borders"
        );
        assert!(has_title_with_border(&frame, "Backlog"));
        assert!(has_title_with_border(&frame, "Merge Queue"));
        assert!(frame.contains("Backlog"));
        assert!(frame.contains("Merge Queue"));
    }

    #[test]
    fn work_now_card_freezes_spinner_after_startup() {
        let active_frame = render_dashboard_at_tick(
            &[worker(10, false)],
            &QueueStats {
                ready: 1,
                active: 1,
                failed: 0,
                unresolved: 0,
                merge_pending: 0,
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
                unresolved: 0,
                merge_pending: 0,
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
    fn does_not_render_human_problem_panel() {
        let frame = render_dashboard(
            &[worker(901, false)],
            &QueueStats {
                ready: 0,
                active: 1,
                failed: 0,
                unresolved: 0,
                merge_pending: 0,
                p0: 1,
                p1: 0,
                p2: 0,
            },
            &BacklogView::default(),
            80,
            20,
        );
        assert!(!frame.contains("Problems Requiring Human"));
        assert!(!frame.contains("needs intervention"));
    }

    #[test]
    fn dashboard_worker_labels_are_readable() {
        let frame = render_dashboard(
            &[
                WorkerRow {
                    worker_id: "w1".to_string(),
                    state: "backlog_sync".to_string(),
                    task_id: None,
                    last_state_line: 0,
                    task_title: "task one".to_string(),
                    tool_line: "tool".to_string(),
                    breadcrumb: "boot>backlog_sync".to_string(),
                    last_heartbeat_secs: 5,
                    session_age_secs: 1,
                    lease_held: true,
                    session_missing: false,
                    command_details: Vec::new(),
                },
                WorkerRow {
                    worker_id: "w2".to_string(),
                    state: "merging".to_string(),
                    task_id: Some("task-two".to_string()),
                    last_state_line: 0,
                    task_title: "task two".to_string(),
                    tool_line: "prompt 12".to_string(),
                    breadcrumb: "state>merging".to_string(),
                    last_heartbeat_secs: 5,
                    session_age_secs: 1,
                    lease_held: true,
                    session_missing: false,
                    command_details: Vec::new(),
                },
            ],
            &QueueStats {
                ready: 0,
                active: 2,
                failed: 0,
                unresolved: 0,
                merge_pending: 0,
                p0: 0,
                p1: 2,
                p2: 0,
            },
            &BacklogView::default(),
            120,
            30,
        );
        assert_eq!(
            format_breadcrumb("boot>backlog_sync"),
            "Boot > Backlog Sync"
        );
        assert_eq!(format_breadcrumb("state>merging"), "Merging");
        assert_eq!(format_state_label("backlog_sync"), "Backlog Sync");
        assert_eq!(format_state_label("merging"), "Merging");
        assert!(frame.contains("task one"));
        assert!(frame.contains("task two"));
        assert!(frame.contains("Flow:"));
    }

    #[test]
    fn worker_flow_chain_shows_full_chain_for_understand_state() {
        let spans = worker_flow_chain_spans("understand");
        let labels = spans
            .iter()
            .map(|span| span.content.to_string())
            .filter(|label| label != " → ")
            .collect::<Vec<_>>();
        assert_eq!(
            labels,
            vec![
                "Understand",
                "Planning",
                "Doing",
                "Gitting",
                "Reviewing",
                "Merging",
                "Complete"
            ]
        );
    }

    #[test]
    fn worker_command_stream_shows_most_recent_first() {
        let entries = vec![
            CommandEntry {
                timestamp: "10:00:00".to_string(),
                command: "first".to_string(),
            },
            CommandEntry {
                timestamp: "10:00:10".to_string(),
                command: "second".to_string(),
            },
            CommandEntry {
                timestamp: "10:00:20".to_string(),
                command: "third".to_string(),
            },
        ];
        assert_eq!(
            worker_command_stream(&entries),
            "10:00:20  third  |  10:00:10  second  |  10:00:00  first"
        );
    }

    #[test]
    fn command_stream_window_truncates_without_scrolling() {
        let long = "long command stream that should be truncated";
        assert_eq!(command_stream_window(long, 10), "long comm…");
    }

    #[test]
    fn worker_flow_chain_normalizes_case_and_whitespace_before_display() {
        let spans = worker_flow_chain_spans("  PLANNING ");
        let labels = spans
            .iter()
            .map(|span| span.content.to_string())
            .filter(|label| label != " → ")
            .collect::<Vec<_>>();
        assert_eq!(
            labels,
            vec![
                "Understand",
                "Planning",
                "Doing",
                "Gitting",
                "Reviewing",
                "Merging",
                "Complete"
            ]
        );
    }

    #[test]
    fn worker_flow_chain_treats_handoff_as_merging_state() {
        let spans = worker_flow_chain_spans("handoff");
        let labels = spans
            .iter()
            .map(|span| span.content.to_string())
            .filter(|label| label != " → ")
            .collect::<Vec<_>>();
        assert_eq!(
            labels,
            vec![
                "Understand",
                "Planning",
                "Doing",
                "Gitting",
                "Reviewing",
                "Merging",
                "Complete"
            ]
        );
        assert_eq!(format_state_label("handoff"), "Merging");
    }

    #[test]
    fn worker_flow_chain_handles_state_prefixes_for_early_states() {
        for state in ["state>planning", "state planning", "\"understand\""] {
            let spans = worker_flow_chain_spans(state);
            let labels = spans
                .iter()
                .map(|span| span.content.to_string())
                .filter(|label| label != " → ")
                .collect::<Vec<_>>();
            assert_eq!(
                labels,
                vec![
                    "Understand",
                    "Planning",
                    "Doing",
                    "Gitting",
                    "Reviewing",
                    "Merging",
                    "Complete"
                ],
                "full flow chain not rendered for state '{state}'"
            );
        }
    }

    #[test]
    fn active_worker_displays_current_state_label() {
        let frame = render_dashboard(
            &[WorkerRow {
                worker_id: "w1".to_string(),
                state: "merge_polling".to_string(),
                task_id: Some("task-merge".to_string()),
                last_state_line: 0,
                task_title: "merge worker".to_string(),
                tool_line: "git merge".to_string(),
                breadcrumb: "state>merge_polling".to_string(),
                last_heartbeat_secs: 12,
                session_age_secs: 33,
                lease_held: true,
                session_missing: false,
                command_details: Vec::new(),
            }],
            &QueueStats {
                ready: 0,
                active: 1,
                failed: 0,
                unresolved: 0,
                merge_pending: 0,
                p0: 0,
                p1: 1,
                p2: 0,
            },
            &BacklogView::default(),
            120,
            24,
        );
        assert!(frame.contains("State:"));
        assert!(frame.contains("Checking mergeability"));
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
                    task_id: Some("task-1".to_string()),
                    last_state_line: 0,
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
                    task_id: Some("task-2".to_string()),
                    last_state_line: 0,
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
                unresolved: 0,
                merge_pending: 0,
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
                task_id: None,
                last_state_line: 0,
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
            unresolved: 0,
            merge_pending: 0,
            p0: 0,
            p1: workers.len(),
            p2: 0,
        };
        let backlog = BacklogView::default();

        let initial = render_dashboard(&workers, &stats, &backlog, 80, 24);
        assert!(!initial.contains("Workers:"));
        assert!(!initial.contains("Workers ("));
        assert!(initial.contains("> Worker 1"));
        assert!(!initial.contains("Worker 9"));

        for _ in 0..10 {
            let _ = scroll_workers_down();
        }
        let scrolled = render_dashboard(&workers, &stats, &backlog, 80, 24);
        assert!(!scrolled.contains("Worker 1"));
        assert!(!scroll_workers_down());

        for _ in 0..10 {
            let _ = scroll_workers_up();
        }
        let reset = render_dashboard(&workers, &stats, &backlog, 80, 24);
        assert!(reset.contains("> Worker 1"));
        assert!(!scroll_workers_up());
    }

    #[test]
    fn wizard_step_labels_has_five_steps_with_backlog() {
        assert_eq!(super::WIZARD_STEP_LABELS.len(), 5);
        assert_eq!(super::WIZARD_STEP_LABELS[3], "Backlog");
        assert_eq!(
            super::WIZARD_STEP_LABELS,
            ["Parallelism", "Validation", "Docs", "Backlog", "Notes"]
        );
    }

    #[test]
    fn wizard_step_indicator_highlights_backlog_at_step_3() {
        let line = super::wizard_step_indicator(3);
        let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert!(
            text.contains("Backlog"),
            "step indicator should show Backlog"
        );
        assert!(text.contains("Parallelism"));
        assert!(text.contains("Notes"));
    }

    #[test]
    fn wizard_answers_includes_backlog_approval() {
        let answers = super::RepoHealthWizardAnswers {
            preferred_parallelism: 3,
            validation_command: "cargo test".to_string(),
            external_docs_accessible: true,
            backlog_approval: true,
            additional_context: String::new(),
        };
        assert!(answers.backlog_approval);

        let auto = super::RepoHealthWizardAnswers {
            backlog_approval: false,
            ..answers
        };
        assert!(!auto.backlog_approval);
    }

    #[test]
    fn seeding_screen_renders_header_and_activity() {
        let activity = vec![
            "Exploring repository structure".to_string(),
            "Analyzing code quality signals".to_string(),
        ];
        let frame = super::render_seeding(&activity, 80, 20);
        assert!(
            frame.contains("seeding your backlog"),
            "should show seeding header"
        );
        assert!(
            frame.contains("Exploring repository"),
            "should show activity lines"
        );
        assert!(
            frame.contains("Analyzing code quality"),
            "should show all activity"
        );
    }

    #[test]
    fn seeding_screen_renders_empty_activity() {
        let frame = super::render_seeding(&[], 80, 20);
        assert!(frame.contains("seeding your backlog"));
    }

    #[test]
    fn quality_intro_screen_renders_header_and_dimensions() {
        let frame = super::render_quality_intro(120, 20);
        assert!(
            frame.contains("grading your repository"),
            "should show grading header"
        );
        assert!(
            frame.contains("Quality Dimensions"),
            "should show Quality Dimensions block title"
        );
        assert!(
            frame.contains("test_coverage"),
            "should show test_coverage dimension"
        );
        assert!(
            frame.contains("agent_steering"),
            "should show agent_steering dimension"
        );
        assert!(
            frame.contains("documentation_quality"),
            "should show documentation_quality dimension"
        );
        assert!(
            frame.contains("assessing 9 quality dimensions"),
            "should show footer message"
        );
    }

    #[test]
    fn quality_dimensions_has_nine_entries() {
        assert_eq!(
            super::QUALITY_DIMENSIONS.len(),
            9,
            "should have exactly 9 quality dimensions"
        );
    }

    #[test]
    fn quality_dimensions_ids_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for (id, _) in super::QUALITY_DIMENSIONS {
            assert!(seen.insert(id), "duplicate dimension ID: {id}");
        }
    }

    #[test]
    fn style_activity_line_styles_agent_activity_command() {
        let line = "Agent activity: shell started: `cargo test`";
        let styled = super::style_activity_line(line);
        // Should have 3 spans: timestamp, prefix, command
        assert_eq!(
            styled.spans.len(),
            3,
            "agent activity line should have 3 spans"
        );
        let command_span = &styled.spans[2];
        assert!(
            command_span.content.contains("cargo test"),
            "command span should contain the command"
        );
    }

    #[test]
    fn style_activity_line_handles_plain_line() {
        let line = "Agent session started";
        let styled = super::style_activity_line(line);
        // Should have 2 spans: timestamp and body
        assert_eq!(styled.spans.len(), 2, "plain line should have 2 spans");
        assert!(
            styled.spans[1].content.contains("Agent session started"),
            "body span should contain the message"
        );
    }

    #[test]
    fn seed_review_renders_task_card_with_all_fields() {
        use crate::seed_runner::SeedTask;
        let task = SeedTask {
            title: "Add AGENTS.md configuration".to_string(),
            details: "Create an AGENTS.md file with repo conventions".to_string(),
            rationale: "Helps agents understand repo norms faster".to_string(),
            domain: "agent_steering".to_string(),
            priority: "P0".to_string(),
        };
        let frame = super::render_seed_review(&task, 0, 5, 100, 25);
        assert!(
            frame.contains("review backlog"),
            "header should show review backlog"
        );
        assert!(frame.contains("(1/5)"), "should show 1-indexed counter");
        assert!(frame.contains("Add AGENTS.md"), "should show task title");
        assert!(
            frame.contains("Helps agents understand"),
            "should show rationale"
        );
        assert!(frame.contains("P0"), "should show priority badge");
        assert!(frame.contains("[k] Keep"), "should show keep hotkey");
        assert!(frame.contains("[d] Discard"), "should show discard hotkey");
        assert!(frame.contains("[q]"), "should show quit hotkey");
    }

    #[test]
    fn seed_review_renders_different_priorities() {
        use crate::seed_runner::SeedTask;
        for priority in &["P0", "P1", "P2"] {
            let task = SeedTask {
                title: "Task".to_string(),
                details: "Details".to_string(),
                rationale: "Rationale".to_string(),
                domain: "testing".to_string(),
                priority: priority.to_string(),
            };
            let frame = super::render_seed_review(&task, 2, 10, 80, 20);
            assert!(frame.contains("(3/10)"), "counter should be 1-indexed");
            assert!(frame.contains(priority), "should show {priority}");
        }
    }

    // --- WizardState key handling tests ---

    use super::{WizardAction, WizardState};
    use crossterm::event::{KeyCode, KeyModifiers};

    fn wizard_at_step(step: usize) -> WizardState {
        WizardState {
            step,
            parallelism_input: "3".to_string(),
            validation: "cargo test".to_string(),
            docs_accessible: true,
            backlog_approval: true,
            notes: String::new(),
        }
    }

    #[test]
    fn wizard_backlog_a_key_selects_auto_seed_and_advances() {
        let mut ws = wizard_at_step(3);
        assert!(ws.backlog_approval); // default: review
        ws.handle_key(KeyCode::Char('a'), KeyModifiers::NONE);
        assert!(!ws.backlog_approval, "'a' should select auto-seed");
        assert_eq!(ws.step, 4, "'a' should advance to Notes");
    }

    #[test]
    fn wizard_backlog_r_key_selects_review_and_advances() {
        let mut ws = wizard_at_step(3);
        ws.backlog_approval = false; // start at auto-seed
        ws.handle_key(KeyCode::Char('r'), KeyModifiers::NONE);
        assert!(ws.backlog_approval, "'r' should select review");
        assert_eq!(ws.step, 4, "'r' should advance to Notes");
    }

    #[test]
    fn wizard_backlog_uppercase_keys_select_and_advance() {
        let mut ws = wizard_at_step(3);
        ws.handle_key(KeyCode::Char('A'), KeyModifiers::NONE);
        assert!(!ws.backlog_approval, "'A' should select auto-seed");
        assert_eq!(ws.step, 4, "'A' should advance to Notes");
    }

    #[test]
    fn wizard_backlog_arrow_keys_toggle() {
        let mut ws = wizard_at_step(3);
        assert!(ws.backlog_approval);
        ws.handle_key(KeyCode::Down, KeyModifiers::NONE);
        assert!(!ws.backlog_approval, "Down should toggle to auto-seed");
        ws.handle_key(KeyCode::Up, KeyModifiers::NONE);
        assert!(ws.backlog_approval, "Up should toggle back to review");
    }

    #[test]
    fn wizard_backlog_tab_toggles() {
        let mut ws = wizard_at_step(3);
        assert!(ws.backlog_approval);
        ws.handle_key(KeyCode::Tab, KeyModifiers::NONE);
        assert!(!ws.backlog_approval, "Tab should toggle to auto-seed");
        ws.handle_key(KeyCode::Tab, KeyModifiers::NONE);
        assert!(ws.backlog_approval, "Tab should toggle back to review");
    }

    #[test]
    fn wizard_backlog_enter_advances_to_notes() {
        let mut ws = wizard_at_step(3);
        let action = ws.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(action, WizardAction::Continue);
        assert_eq!(ws.step, 4, "Enter should advance to step 4 (Notes)");
    }

    #[test]
    fn wizard_backlog_tab_then_enter_preserves_selection() {
        let mut ws = wizard_at_step(3);
        ws.handle_key(KeyCode::Tab, KeyModifiers::NONE); // toggle to auto-seed
        assert!(!ws.backlog_approval);
        assert_eq!(ws.step, 3, "Tab should not advance");
        ws.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(ws.step, 4);
        assert!(
            !ws.backlog_approval,
            "auto-seed selection should persist after Enter"
        );
    }

    #[test]
    fn wizard_esc_finishes_at_any_step() {
        for step in 0..5 {
            let mut ws = wizard_at_step(step);
            let action = ws.handle_key(KeyCode::Esc, KeyModifiers::NONE);
            assert_eq!(
                action,
                WizardAction::Finish,
                "Esc at step {step} should finish"
            );
        }
    }

    #[test]
    fn wizard_step_progression_through_all_steps() {
        let mut ws = wizard_at_step(0);
        ws.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(ws.step, 1);
        ws.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(ws.step, 2);
        ws.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(ws.step, 3);
        ws.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(ws.step, 4);
        let action = ws.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(action, WizardAction::Finish, "Enter on Notes should finish");
    }

    #[test]
    fn wizard_unrelated_keys_on_backlog_step_ignored() {
        let mut ws = wizard_at_step(3);
        ws.handle_key(KeyCode::Char('x'), KeyModifiers::NONE);
        assert!(
            ws.backlog_approval,
            "unrelated key should not change selection"
        );
        assert_eq!(ws.step, 3, "unrelated key should not change step");
    }
}
