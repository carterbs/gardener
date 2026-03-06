use std::cell::RefCell;
#[cfg(not(test))]
use std::io;
use std::io::Stdout;

use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, LeaveAlternateScreen};
#[cfg(not(test))]
use crossterm::terminal::{enable_raw_mode, EnterAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use serde_json::json;

use crate::errors::GardenerError;
use crate::logging::append_run_log;
use crate::tui::dashboard::draw_dashboard_frame;
use crate::tui::report::draw_report_frame;
use crate::tui::startup::{live_startup_headline, reset_live_startup_headline};
use crate::tui::state::{BacklogView, QueueStats, WorkerRow};
use crate::tui::triage::draw_triage_frame;
use crate::tui::views::terminal::{draw_seeding_frame, draw_shutdown_frame};

thread_local! {
    static LIVE_TUI: RefCell<Option<Terminal<CrosstermBackend<Stdout>>>> = const { RefCell::new(None) };
    static LIVE_TUI_SIZE: RefCell<Option<(u16, u16)>> = const { RefCell::new(None) };
    static WORKERS_VIEWPORT_OFFSET: RefCell<usize> = const { RefCell::new(0) };
    static WORKERS_VIEWPORT_SELECTED: RefCell<usize> = const { RefCell::new(0) };
    static WORKERS_VIEWPORT_CAPACITY: RefCell<usize> = const { RefCell::new(1) };
    static WORKERS_TOTAL_COUNT: RefCell<usize> = const { RefCell::new(0) };
}

pub(crate) fn draw_live_frame<F>(draw: F) -> Result<(), GardenerError>
where
    F: FnOnce(&mut ratatui::Frame<'_>),
{
    append_run_log("debug", "tui.live_terminal.draw_frame", json!({}));
    #[cfg(test)]
    {
        let _ = crate::tui::render_to_string(80, 18, draw);
        Ok(())
    }
    #[cfg(not(test))]
    {
        append_run_log("debug", "tui.live_terminal.ensure", json!({}));
        LIVE_TUI.with(|cell| -> Result<(), GardenerError> {
            let mut slot = cell.borrow_mut();
            if slot.is_none() {
                let mut stdout = io::stdout();
                enable_raw_mode().map_err(|e| GardenerError::Io(e.to_string()))?;
                execute!(stdout, EnterAlternateScreen)
                    .map_err(|e| GardenerError::Io(e.to_string()))?;
                let backend = CrosstermBackend::new(stdout);
                let terminal =
                    Terminal::new(backend).map_err(|e| GardenerError::Io(e.to_string()))?;
                *slot = Some(terminal);
                let size =
                    crossterm::terminal::size().map_err(|e| GardenerError::Io(e.to_string()))?;
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
            terminal
                .draw(draw)
                .map(|_| ())
                .map_err(|e| GardenerError::Io(e.to_string()))
        })
    }
}

pub(crate) fn teardown_terminal(
    mut terminal: Terminal<CrosstermBackend<Stdout>>,
) -> Result<(), GardenerError> {
    disable_raw_mode().map_err(|e| GardenerError::Io(e.to_string()))?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)
        .map_err(|e| GardenerError::Io(e.to_string()))?;
    terminal
        .show_cursor()
        .map_err(|e| GardenerError::Io(e.to_string()))
}

pub fn draw_dashboard_live(
    workers: &[WorkerRow],
    stats: &QueueStats,
    backlog: &BacklogView,
    heartbeat_interval_seconds: u64,
    lease_timeout_seconds: u64,
) -> Result<(), GardenerError> {
    let startup_headline = live_startup_headline();
    append_run_log("debug", "tui.dashboard.draw_live", json!({ "workers": workers.len() }));
    draw_live_frame(|frame| {
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
}

pub fn draw_report_live(path: &str, report: &str) -> Result<(), GardenerError> {
    append_run_log(
        "debug",
        "tui.report.draw_live",
        json!({ "path": path, "chars": report.chars().count() }),
    );
    draw_live_frame(|frame| draw_report_frame(frame, path, report))
}

pub fn draw_seeding_live(activity: &[String]) -> Result<(), GardenerError> {
    append_run_log(
        "debug",
        "tui.seeding.draw_live",
        json!({ "activity_lines": activity.len() }),
    );
    draw_live_frame(|frame| draw_seeding_frame(frame, activity))
}

pub fn draw_triage_live(activity: &[String], artifacts: &[String]) -> Result<(), GardenerError> {
    append_run_log(
        "debug",
        "tui.triage.draw_live",
        json!({
            "activity_lines": activity.len(),
            "artifacts": artifacts.len()
        }),
    );
    draw_live_frame(|frame| draw_triage_frame(frame, activity, artifacts))
}

pub fn draw_shutdown_screen_live(title: &str, message: &str) -> Result<(), GardenerError> {
    append_run_log(
        "debug",
        "tui.shutdown.draw_live",
        json!({ "title": title, "chars": message.chars().count() }),
    );
    draw_live_frame(|frame| draw_shutdown_frame(frame, title, message))
}

pub fn close_live_terminal() -> Result<(), GardenerError> {
    append_run_log("debug", "tui.live_terminal.close", json!({}));
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

pub(crate) fn selected_worker_state() -> usize {
    WORKERS_VIEWPORT_SELECTED.with(|cell| *cell.borrow())
}

pub(crate) fn set_worker_viewport(capacity: usize, total: usize) {
    WORKERS_VIEWPORT_CAPACITY.with(|cell| {
        *cell.borrow_mut() = capacity;
    });
    WORKERS_TOTAL_COUNT.with(|cell| {
        *cell.borrow_mut() = total;
    });
}

pub(crate) fn clamped_selected_worker(total: usize) -> usize {
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

pub(crate) fn worker_offset_for_selection(
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
