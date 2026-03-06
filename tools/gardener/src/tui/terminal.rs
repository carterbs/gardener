use std::cell::RefCell;
use std::io::{self, Stdout};

use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, Paragraph};
use ratatui::Terminal;

use crate::errors::GardenerError;

use super::dashboard::draw_dashboard_frame;
use super::formatting::style_activity_line;
use super::report::draw_report_frame;
use super::startup::{live_startup_headline, reset_live_startup_headline};
use super::state::{BacklogView, QueueStats, WorkerRow};
use super::triage::draw_triage_frame;

thread_local! {
    static LIVE_TUI: RefCell<Option<Terminal<CrosstermBackend<Stdout>>>> = const { RefCell::new(None) };
    static LIVE_TUI_SIZE: RefCell<Option<(u16, u16)>> = const { RefCell::new(None) };
    static WORKERS_VIEWPORT_OFFSET: RefCell<usize> = const { RefCell::new(0) };
    static WORKERS_VIEWPORT_SELECTED: RefCell<usize> = const { RefCell::new(0) };
    static WORKERS_VIEWPORT_CAPACITY: RefCell<usize> = const { RefCell::new(1) };
    static WORKERS_TOTAL_COUNT: RefCell<usize> = const { RefCell::new(0) };
}

pub(super) fn with_live_terminal<F>(f: F) -> Result<(), GardenerError>
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

pub fn draw_seeding_live(activity: &[String]) -> Result<(), GardenerError> {
    with_live_terminal(|terminal| {
        terminal
            .draw(|frame| draw_seeding_frame(frame, activity))
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

pub fn render_seeding(activity: &[String], width: u16, height: u16) -> String {
    super::render_to_string(width, height, |frame| draw_seeding_frame(frame, activity))
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
        vec![ratatui::widgets::ListItem::new(
            "- waiting for seeding updates",
        )]
    } else {
        activity
            .iter()
            .map(|line| ratatui::widgets::ListItem::new(style_activity_line(line)))
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

    let footer = Paragraph::new("Seeding in progress — agent is exploring your repository").block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(Color::Rgb(82, 88, 126))),
    );
    frame.render_widget(footer, chunks[2]);
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
    reset_live_startup_headline();
    Ok(())
}

pub(super) fn teardown_terminal(
    mut terminal: Terminal<CrosstermBackend<Stdout>>,
) -> Result<(), GardenerError> {
    disable_raw_mode().map_err(|e| GardenerError::Io(e.to_string()))?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)
        .map_err(|e| GardenerError::Io(e.to_string()))?;
    terminal
        .show_cursor()
        .map_err(|e| GardenerError::Io(e.to_string()))
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

pub(super) fn selected_worker_state() -> usize {
    WORKERS_VIEWPORT_SELECTED.with(|cell| *cell.borrow())
}

pub(super) fn set_worker_viewport(capacity: usize, total: usize) {
    WORKERS_VIEWPORT_CAPACITY.with(|cell| {
        *cell.borrow_mut() = capacity;
    });
    WORKERS_TOTAL_COUNT.with(|cell| {
        *cell.borrow_mut() = total;
    });
}

pub(super) fn clamped_selected_worker(total: usize) -> usize {
    WORKERS_VIEWPORT_SELECTED.with(|cell| {
        let mut selected = cell.borrow_mut();
        if total == 0 {
            *selected = 0;
        } else {
            *selected = (*selected).min(total - 1);
        }
        *selected
    })
}

pub(super) fn worker_offset_for_selection(
    selected_worker: usize,
    worker_row_capacity: usize,
    visible_worker_count: usize,
) -> usize {
    let max_worker_offset = visible_worker_count.saturating_sub(worker_row_capacity);
    WORKERS_VIEWPORT_OFFSET.with(|cell| {
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
    })
}
