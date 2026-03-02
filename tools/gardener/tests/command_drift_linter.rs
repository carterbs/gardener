use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, Debug)]
enum CommandKind {
    Gardener,
    SeedBacklog,
    Unknown,
}

#[derive(Debug)]
struct CommandReference {
    file: PathBuf,
    line: usize,
    command: String,
    command_kind: CommandKind,
    flag: Vec<String>,
}

#[derive(Debug)]
struct CommandDrift {
    file: PathBuf,
    line: usize,
    command: String,
    missing_flags: Vec<String>,
    unknown_command: Option<String>,
}

#[test]
fn linter_agent_facing_commands_match_current_cli() {
    let gardener_help = gardener::render_help();
    let seed_backlog_help = seed_backlog_help();

    let mentions = collect_command_references();
    let mut drifts = Vec::new();

    for reference in mentions {
        match reference.command_kind {
            CommandKind::Unknown => {
                let command = reference.command.clone();
                drifts.push(CommandDrift {
                    file: reference.file,
                    line: reference.line,
                    command: command.clone(),
                    missing_flags: Vec::new(),
                    unknown_command: Some(command),
                });
            }
            CommandKind::Gardener => {
                let mut missing_flags = Vec::new();
                for flag in &reference.flag {
                    if !help_contains_flag(&gardener_help, flag) {
                        missing_flags.push(flag.clone());
                    }
                }

                if !missing_flags.is_empty() {
                    drifts.push(CommandDrift {
                        file: reference.file,
                        line: reference.line,
                        command: reference.command,
                        missing_flags,
                        unknown_command: None,
                    });
                }
            }
            CommandKind::SeedBacklog => {
                let mut missing_flags = Vec::new();
                for flag in &reference.flag {
                    if !help_contains_flag(&seed_backlog_help, flag) {
                        missing_flags.push(flag.clone());
                    }
                }

                if !missing_flags.is_empty() {
                    drifts.push(CommandDrift {
                        file: reference.file,
                        line: reference.line,
                        command: reference.command,
                        missing_flags,
                        unknown_command: None,
                    });
                }
            }
        }
    }

    if !drifts.is_empty() {
        let mut message = String::new();
        message.push_str("command-drift linter failed: command docs/reference text drifts from supported CLI flags\n\n");

        for drift in drifts {
            message.push_str(&format!(
                "- {}:{}: {}\n",
                drift.file.display(),
                drift.line,
                drift.command,
            ));
            if let Some(command) = drift.unknown_command {
                message.push_str(&format!("  - unknown command `{command}`\n"));
            }
            if !drift.missing_flags.is_empty() {
                for flag in drift.missing_flags {
                    message.push_str(&format!("  - missing flag `{flag}`\n"));
                }
            }
            message.push('\n');
        }

        panic!("{message}");
    }
}

fn collect_command_references() -> Vec<CommandReference> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir
        .parent()
        .and_then(|path| path.parent())
        .expect("crate lives under <repo_root>/tools/gardener");
    let mut references = Vec::new();

    const FILE_PATHS: &[&str] = &[
        "AGENTS.md",
        "README.md",
        "docs/conventions/workflow.md",
        "tools/gardener/src/startup.rs",
        "tools/gardener/src/triage.rs",
    ];

    for relative_path in FILE_PATHS {
        let path = repo_root.join(relative_path);
        let source = fs::read_to_string(&path).expect("read command-facing reference file");
        references.extend(collect_command_mentions(&path, &source));
    }

    references
}

fn collect_command_mentions(path: &Path, source: &str) -> Vec<CommandReference> {
    let mut references = Vec::new();
    let is_markdown = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension == "md");

    if is_markdown {
        references.extend(collect_markdown_references(path, source));
    } else {
        references.extend(collect_inline_code_references(path, source));
    }

    references
}

fn collect_markdown_references(path: &Path, source: &str) -> Vec<CommandReference> {
    let mut references = Vec::new();
    let mut in_fence = false;

    for (line_index, line) in source.lines().enumerate() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }

        if in_fence {
            if let Some(reference) = parse_command_reference(path, line_index + 1, line) {
                references.push(reference);
            }
            continue;
        }

        references.extend(collect_inline_code_references_from_line(
            line_index + 1,
            path,
            line,
        ));
    }

    references
}

fn collect_inline_code_references(path: &Path, source: &str) -> Vec<CommandReference> {
    let mut references = Vec::new();

    for (line_index, line) in source.lines().enumerate() {
        references.extend(collect_inline_code_references_from_line(
            line_index + 1,
            path,
            line,
        ));
    }

    references
}

fn collect_inline_code_references_from_line(
    line_number: usize,
    path: &Path,
    line: &str,
) -> Vec<CommandReference> {
    let mut references = Vec::new();

    let mut segments = line.split('`');
    while let Some(_before) = segments.next() {
        if let Some(raw) = segments.next() {
            if let Some(reference) = parse_command_reference(path, line_number, raw) {
                references.push(reference);
            }
        } else {
            break;
        }
    }

    references
}

fn parse_command_reference(
    path: &Path,
    line_number: usize,
    command_line: &str,
) -> Option<CommandReference> {
    let cleaned = command_line.trim();
    if cleaned.is_empty() {
        return None;
    }

    let mut command_parts = cleaned.split_whitespace();
    let raw_command = command_parts.next()?;
    let normalized_command = normalize_token(raw_command);
    let command_kind = match normalized_command {
        "scripts/brad-gardener" | "gardener" | "brad-gardener" => {
            if normalized_command == "brad-gardener" {
                CommandKind::Unknown
            } else {
                CommandKind::Gardener
            }
        }
        "seed-backlog" => CommandKind::SeedBacklog,
        _ => return None,
    };

    let mut flags = Vec::new();
    for part in command_parts {
        if !part.starts_with("--") {
            continue;
        }

        let normalized = normalize_token(part);
        if normalized == "--" {
            continue;
        }

        let without_value = normalized
            .split_once('=')
            .map_or(normalized, |(before, _)| before);
        if without_value.starts_with("--") {
            flags.push(without_value.to_string());
        }
    }

    Some(CommandReference {
        file: path.to_path_buf(),
        line: line_number,
        command: cleaned.to_string(),
        command_kind,
        flag: flags,
    })
}

fn normalize_token(token: &str) -> &str {
    token.trim_matches(
        &[
            '[', ']', '(', ')', '{', '}', '<', '>', '`', ',', ';', ':', '.', '!',
        ][..],
    )
}

fn seed_backlog_help() -> String {
    let mut cmd = if let Ok(path) = std::env::var("CARGO_BIN_EXE_seed-backlog") {
        let mut cmd = Command::new(path);
        cmd.arg("--help");
        cmd
    } else {
        let mut cmd = Command::new("cargo");
        cmd.args(["run", "--quiet", "--bin", "seed-backlog", "--"]);
        cmd.arg("--help");
        cmd
    };
    let out = cmd.output().expect("seed-backlog --help");
    assert!(out.status.success(), "seed-backlog --help should succeed");
    String::from_utf8(out.stdout).expect("seed-backlog --help utf-8")
}

fn help_contains_flag(help: &str, flag: &str) -> bool {
    let flag_with_equals = format!("{flag}=");
    help.split_whitespace().any(|token| {
        let normalized = normalize_token(token);
        normalized == flag || normalized.starts_with(&flag_with_equals)
    })
}
