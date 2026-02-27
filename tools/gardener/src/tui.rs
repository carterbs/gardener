use crate::errors::GardenerError;
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
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| draw_dashboard_frame(frame, workers, stats, backlog, 15, 900))
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

fn draw_dashboard_frame(
    frame: &mut ratatui::Frame<'_>,
    workers: &[WorkerRow],
    stats: &QueueStats,
    backlog: &BacklogView,
    heartbeat_interval_seconds: u64,
    lease_timeout_seconds: u64,
) {
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
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
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

    let worker_items = workers
        .iter()
        .map(|row| {
            let state_style = match row.state.as_str() {
                "complete" => Style::default().fg(Color::Green),
                "failed" => Style::default().fg(Color::Red),
                "doing" | "gitting" | "reviewing" | "merging" | "planning" => {
                    Style::default().fg(Color::Yellow)
                }
                "init" | "backlog_sync" | "working" | "understand" => {
                    Style::default().fg(Color::Cyan)
                }
                _ => Style::default().fg(Color::Gray),
            };
            ListItem::new(vec![
                Line::from(vec![
                    Span::styled(
                        format!("{:<3}", row.worker_id),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(": "),
                    Span::raw(row.task_title.clone()),
                ]),
                Line::from(vec![
                    Span::raw("    "),
                    Span::styled("State: ", Style::default().fg(Color::Blue)),
                    Span::styled(format!("{:<13}", humanize_state(&row.state)), state_style),
                    Span::raw("  "),
                    Span::styled("Action: ", Style::default().fg(Color::Blue)),
                    Span::raw(format!("{:<22}", humanize_action(&row.tool_line))),
                    Span::raw("  "),
                    Span::styled("Flow: ", Style::default().fg(Color::Blue)),
                    Span::raw(humanize_breadcrumb(&row.breadcrumb)),
                ]),
            ])
        })
        .collect::<Vec<_>>();

    let workers_panel = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(body[0]);
    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            "Workers",
            Style::default()
                .fg(Color::Rgb(126, 231, 135))
                .add_modifier(Modifier::BOLD),
        )])),
        workers_panel[0],
    );
    frame.render_widget(
        List::new(worker_items).block(
            Block::default()
                .borders(Borders::RIGHT)
                .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
        ),
        workers_panel[1],
    );

    let mut backlog_items = Vec::new();
    backlog_items.push(ListItem::new(Line::from(vec![Span::styled(
        "In Progress",
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )])));
    if backlog.in_progress.is_empty() {
        backlog_items.push(ListItem::new("- none"));
    } else {
        backlog_items.extend(
            backlog
                .in_progress
                .iter()
                .map(|line| ListItem::new(format!("- {line}"))),
        );
    }
    backlog_items.push(ListItem::new(""));
    backlog_items.push(ListItem::new(Line::from(vec![Span::styled(
        "Queued",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )])));
    if backlog.queued.is_empty() {
        backlog_items.push(ListItem::new("- none"));
    } else {
        backlog_items.extend(
            backlog
                .queued
                .iter()
                .map(|line| ListItem::new(format!("- {line}"))),
        );
    }
    let backlog_panel = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(body[1]);
    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            "Backlog",
            Style::default()
                .fg(Color::Rgb(245, 196, 95))
                .add_modifier(Modifier::BOLD),
        )])),
        backlog_panel[0],
    );
    frame.render_widget(List::new(backlog_items), backlog_panel[1]);

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
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(2),
        ])
        .split(frame.area());
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
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

    let activity_items = if activity.is_empty() {
        vec![ListItem::new("- waiting for triage updates")]
    } else {
        activity
            .iter()
            .map(|line| ListItem::new(format!("- {line}")))
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

    let artifact_items = if artifacts.is_empty() {
        vec![ListItem::new("- no triage artifacts yet")]
    } else {
        artifacts
            .iter()
            .map(|line| ListItem::new(format!("- {line}")))
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
}

pub fn draw_dashboard_live(
    workers: &[WorkerRow],
    stats: &QueueStats,
    backlog: &BacklogView,
    heartbeat_interval_seconds: u64,
    lease_timeout_seconds: u64,
) -> Result<(), GardenerError> {
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
                )
            })
            .map(|_| ())
            .map_err(|e| GardenerError::Io(e.to_string()))
    })
}

pub fn draw_report_live(path: &str, report: &str) -> Result<(), GardenerError> {
    let lines = report.lines().take(22).collect::<Vec<_>>().join("\n");
    with_live_terminal(|terminal| {
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
        "gitting" => "Git + PR".to_string(),
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
    if action.eq_ignore_ascii_case("orchestrator") {
        return "Coordinator".to_string();
    }
    title_case_words(action)
}

fn humanize_breadcrumb(path: &str) -> String {
    path.split('>')
        .map(title_case_words)
        .collect::<Vec<_>>()
        .join(" > ")
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
                    .constraints([Constraint::Length(3), Constraint::Min(4), Constraint::Length(3)])
                    .split(area);

                let title = Paragraph::new(
                    "Gardener Setup Wizard (Esc = keep defaults, Enter = next)",
                )
                    .block(Block::default().borders(Borders::ALL).title("Repo Health"));
                frame.render_widget(title, chunks[0]);

                let body_text = match step {
                    0 => format!(
                        "Worker parallelism:\n{}\n\nType a number from 1-32, Enter to continue.",
                        parallelism_input
                    ),
                    1 => format!(
                        "Validation command:\n{}\n\nType to edit, Backspace to delete, Enter to continue.",
                        validation
                    ),
                    2 => format!(
                        "Are architecture/quality docs available in repo?\nCurrent: {}\n\nPress 'y' or 'n', Enter to continue.",
                        if docs_accessible { "yes" } else { "no" }
                    ),
                    _ => format!(
                        "Additional constraints (optional):\n{}\n\nType notes, Enter to finish.",
                        notes
                    ),
                };
                let body = Paragraph::new(body_text)
                    .block(Block::default().borders(Borders::ALL).title("Input"));
                frame.render_widget(body, chunks[1]);

                let footer = Paragraph::new(match step {
                    0 => "Step 1/4",
                    1 => "Step 2/4",
                    2 => "Step 3/4",
                    _ => "Step 4/4",
                })
                .block(Block::default().borders(Borders::ALL));
                frame.render_widget(footer, chunks[2]);
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
        render_triage, BacklogView, ProblemClass, QueueStats, WorkerRow,
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
            20,
        );
        assert!(frame.contains("Queue"));
        assert!(frame.contains("Workers"));
        assert!(!frame.contains("Problems"));
        assert!(frame.contains("Backlog"));
        assert!(frame.contains("In Progress"));
        assert!(frame.contains("Queued"));
        assert!(frame.contains("State:"));
        assert!(frame.contains("Action:"));
        assert!(!frame.contains("status="));
        assert!(!frame.contains("action="));

        // handle_key removed — real dispatch uses hotkeys::action_for_key
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
}
