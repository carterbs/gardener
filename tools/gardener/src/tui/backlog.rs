use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::ListItem;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedBacklogItem {
    pub priority: ParsedBacklogPriority,
    pub title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParsedBacklogPriority {
    P0,
    P1,
    P2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BacklogPriority {
    P0,
    P1,
    P2,
}

impl BacklogPriority {
    pub(super) fn span_style(self) -> Style {
        match self {
            Self::P0 => Style::default().fg(Color::Rgb(255, 122, 122)),
            Self::P1 => Style::default().fg(Color::Rgb(255, 207, 105)),
            Self::P2 => Style::default().fg(Color::Rgb(127, 230, 148)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BacklogItem {
    pub priority: BacklogPriority,
    pub title: String,
}

pub(super) fn dashboard_worker_rows_for_width(width: u16) -> u16 {
    match width {
        0..=79 => 6,
        80..=119 => 8,
        _ => 10,
    }
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

pub(super) fn ordered_backlog_items(
    in_progress: &[String],
    queued: &[String],
) -> Vec<ParsedBacklogItem> {
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

pub(super) fn ordered_merge_queue_items(in_progress: &[String]) -> Vec<ParsedBacklogItem> {
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

pub(super) fn backlog_items_with_capacity(
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
