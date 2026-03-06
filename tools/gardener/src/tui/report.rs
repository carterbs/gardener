use std::cell::RefCell;

use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::hotkeys::report_controls_legend;

use super::formatting::markdown_to_lines;
use super::render_to_string;

thread_local! {
    static REPORT_SCROLL_OFFSET: RefCell<usize> = const { RefCell::new(0) };
    static REPORT_TOTAL_LINES: RefCell<usize> = const { RefCell::new(0) };
}

pub fn render_report_view(path: &str, report: &str, width: u16, height: u16) -> String {
    render_to_string(width, height, |frame| {
        draw_report_frame(frame, path, report)
    })
}

pub(super) fn draw_report_frame(frame: &mut ratatui::Frame<'_>, path: &str, report_raw: &str) {
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
    let viewport_height = chunks[1].height.saturating_sub(2) as usize;
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
