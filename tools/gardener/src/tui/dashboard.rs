use crate::hotkeys::dashboard_controls_legend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};

use super::backlog::{
    backlog_items_with_capacity, dashboard_worker_rows_for_width, ordered_merge_queue_items,
    BacklogItem, BacklogPriority, ParsedBacklogPriority,
};
use super::formatting::{
    command_stream_window, format_current_state_line, merge_worker_card_item, run_context_summary,
    truncate_right, worker_command_stream, worker_flow_chain_spans,
};
use super::render_to_string;
use super::startup::StartupHeadlineView;
use super::state::{AppState, BacklogView, QueueStats, StartupHeadline, WorkerMetrics, WorkerRow};
use super::terminal;

const WORKER_LIST_ROW_HEIGHT: usize = 3;
const COMPACT_WORKER_LIST_ROW_HEIGHT: usize = 2;

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
    render_to_string(width, height, |frame| {
        draw_dashboard_frame(frame, workers, stats, backlog, 15, 900, startup_headline)
    })
}

#[cfg(test)]
pub(crate) fn render_dashboard_at_tick(
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

pub(super) fn draw_dashboard_frame(
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
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(16),
            Constraint::Length(2),
        ])
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
    let worker_row_capacity = (viewport_height as usize / worker_row_height_for_layout).max(1);
    terminal::set_worker_viewport(worker_row_capacity, visible_worker_count);
    let selected_worker = terminal::clamped_selected_worker(visible_worker_count);
    let worker_offset = terminal::worker_offset_for_selection(
        selected_worker,
        worker_row_capacity,
        visible_worker_count,
    );
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
