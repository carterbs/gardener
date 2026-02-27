use ratatui::backend::TestBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use std::io::{self, Stdout};
use crate::errors::GardenerError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerRow {
    pub worker_id: String,
    pub state: String,
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

#[derive(Debug, Clone, PartialEq, Eq)]
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

pub fn render_dashboard(workers: &[WorkerRow], stats: &QueueStats, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(3), Constraint::Min(5), Constraint::Length(5)])
                .split(frame.size());

            let summary = Paragraph::new(format!(
                "ready={} active={} failed={} | P0={} P1={} P2={}",
                stats.ready, stats.active, stats.failed, stats.p0, stats.p1, stats.p2
            ))
            .block(Block::default().borders(Borders::ALL).title("Queue"));
            frame.render_widget(summary, chunks[0]);

            let worker_items = workers
                .iter()
                .map(|row| {
                    ListItem::new(Line::from(vec![
                        Span::styled(
                            format!("{}", row.worker_id),
                            Style::default().fg(Color::Cyan),
                        ),
                        Span::raw(format!(
                            " {} tool={} path={} ",
                            row.state, row.tool_line, row.breadcrumb
                        )),
                    ]))
                })
                .collect::<Vec<_>>();

            frame.render_widget(
                List::new(worker_items).block(Block::default().borders(Borders::ALL).title("Workers")),
                chunks[1],
            );

            let problems = workers
                .iter()
                .map(|row| {
                    let class = classify_problem(row, 15, 900);
                    format!(
                        "{} => {:?} owner={} blocking={} interventions=[retry|release lease|park/escalate]",
                        row.worker_id,
                        class,
                        row.session_age_secs,
                        row.tool_line
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");

            frame.render_widget(
                Paragraph::new(problems)
                    .block(Block::default().borders(Borders::ALL).title("Problems")),
                chunks[2],
            );
        })
        .expect("draw");

    let mut out = String::new();
    let buffer = terminal.backend().buffer().clone();
    for y in 0..height {
        for x in 0..width {
            out.push_str(buffer.get(x, y).symbol());
        }
        out.push('\n');
    }
    out
}

pub fn handle_key(key: char) -> &'static str {
    match key {
        'q' => "quit",
        'r' => "retry",
        'l' => "release-lease",
        'p' => "park-escalate",
        _ => "noop",
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

fn teardown_terminal(mut terminal: Terminal<CrosstermBackend<Stdout>>) -> Result<(), GardenerError> {
    disable_raw_mode().map_err(|e| GardenerError::Io(e.to_string()))?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)
        .map_err(|e| GardenerError::Io(e.to_string()))?;
    terminal
        .show_cursor()
        .map_err(|e| GardenerError::Io(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{classify_problem, handle_key, render_dashboard, ProblemClass, QueueStats, WorkerRow};

    fn worker(heartbeat: u64, missing: bool) -> WorkerRow {
        WorkerRow {
            worker_id: "w1".to_string(),
            state: "doing".to_string(),
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
        assert_eq!(classify_problem(&worker(10, false), 15, 900), ProblemClass::Healthy);
        assert_eq!(classify_problem(&worker(31, false), 15, 900), ProblemClass::Stalled);
        assert_eq!(classify_problem(&worker(901, false), 15, 900), ProblemClass::Zombie);
        assert_eq!(classify_problem(&worker(1, true), 15, 900), ProblemClass::Zombie);
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
            80,
            20,
        );
        assert!(frame.contains("Queue"));
        assert!(frame.contains("Workers"));
        assert!(frame.contains("Problems"));

        assert_eq!(handle_key('q'), "quit");
        assert_eq!(handle_key('r'), "retry");
        assert_eq!(handle_key('l'), "release-lease");
        assert_eq!(handle_key('p'), "park-escalate");
        assert_eq!(handle_key('x'), "noop");
    }
}
