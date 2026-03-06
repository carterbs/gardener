use crate::logging::{current_run_id, current_run_log_path};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::ListItem;
use std::time::{SystemTime, UNIX_EPOCH};

use super::state::{CommandEntry, TriageArtifact, WorkerRow};

pub(super) const WORKER_FLOW_STATES: [&str; 7] = [
    "understand",
    "planning",
    "doing",
    "gitting",
    "reviewing",
    "merging",
    "complete",
];

pub(crate) const WIZARD_STEP_LABELS: [&str; 5] =
    ["Parallelism", "Validation", "Docs", "Backlog", "Notes"];

pub(super) fn truncate_right(input: &str, max_width: usize) -> String {
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

pub(super) fn parse_triage_artifact(line: &str) -> TriageArtifact {
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

pub(super) fn now_unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
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

pub(super) fn run_context_summary() -> (String, String) {
    let run_id = current_run_id().unwrap_or_else(|| "none".to_string());
    let run_log_path = current_run_log_path()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    (truncate_right(&run_id, 28), run_log_path)
}

pub(super) fn equipment_name_for_worker(index: usize, _worker_id: &str) -> String {
    format!("Worker {}", index + 1)
}

pub(super) fn merge_worker_card_item(
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

pub(crate) fn worker_flow_chain_spans(state: &str) -> Vec<Span<'static>> {
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

pub(super) fn format_current_state_line(state: &str) -> String {
    format!("State: {}", format_state_label(state))
}

pub(crate) fn worker_command_stream(commands: &[CommandEntry]) -> String {
    let recent = commands.iter().rev().take(4).collect::<Vec<_>>();
    if recent.is_empty() {
        return "no recent commands".to_string();
    }

    recent
        .into_iter()
        .map(|entry| format!("{}  {}", entry.timestamp, entry.command))
        .collect::<Vec<_>>()
        .join("  |  ")
}

pub(crate) fn command_stream_window(stream: &str, width: usize) -> String {
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

pub(crate) fn format_breadcrumb(path: &str) -> String {
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

pub(crate) fn wizard_step_indicator(current_step: usize) -> Line<'static> {
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

pub(super) fn markdown_to_lines(raw: &str) -> Vec<Line<'static>> {
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

pub(crate) fn style_activity_line(line: &str) -> Line<'static> {
    let timestamp = now_hhmmss();
    let body = line;

    if body.contains("Agent activity:") {
        if let Some(last_colon_pos) = body.rfind(": ") {
            let prefix = &body[..last_colon_pos + 2];
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
