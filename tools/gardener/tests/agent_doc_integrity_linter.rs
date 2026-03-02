use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug)]
struct LinkViolation {
    file: PathBuf,
    line: usize,
    target: String,
}

#[derive(Debug)]
struct CommandTargetViolation {
    file: PathBuf,
    line: usize,
    command: String,
}

#[test]
fn linter_agent_docs_have_valid_links_and_command_targets() {
    let repo_root = repo_root_path();
    let mut link_issues = Vec::new();
    let mut command_issues = Vec::new();

    for path in agent_doc_paths(&repo_root) {
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));

        link_issues.extend(collect_link_violations(&repo_root, &path, &source));
        command_issues.extend(collect_command_target_violations(
            &repo_root, &path, &source,
        ));
    }

    if link_issues.is_empty() && command_issues.is_empty() {
        return;
    }

    let mut message = String::new();
    message.push_str(
        "agent-doc integrity linter failed: invalid local links or missing command targets\n\n",
    );

    for issue in link_issues {
        message.push_str(&format!(
            "- {}:{}: broken markdown link target `{}`\n",
            issue.file.display(),
            issue.line,
            issue.target
        ));
    }

    for issue in command_issues {
        message.push_str(&format!(
            "- {}:{}: missing command target `{}`\n",
            issue.file.display(),
            issue.line,
            issue.command
        ));
    }

    panic!("{message}");
}

fn repo_root_path() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(|path| path.parent())
        .expect("crate should live under <repo_root>/tools/gardener")
        .to_path_buf()
}

fn agent_doc_paths(repo_root: &Path) -> Vec<PathBuf> {
    let mut paths = vec![
        repo_root.join("AGENTS.md"),
        repo_root.join("CLAUDE.md"),
        repo_root.join("docs/README.md"),
        repo_root.join("docs/conventions/workflow.md"),
        repo_root.join("docs/runbooks/backlog-operations.md"),
    ];

    paths.extend(collect_skill_docs(repo_root.join(".codex/skills")));
    paths.extend(collect_skill_docs(repo_root.join(".claude/skills")));

    paths.sort_unstable();
    paths.dedup();
    paths
}

fn collect_skill_docs(root: PathBuf) -> Vec<PathBuf> {
    let mut docs = Vec::new();
    let mut stack = vec![root];

    while let Some(directory) = stack.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => continue,
            };

            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }

            if path.file_name().and_then(|name| name.to_str()) == Some("SKILL.md") {
                docs.push(path);
            }
        }
    }

    docs
}

fn collect_link_violations(repo_root: &Path, path: &Path, source: &str) -> Vec<LinkViolation> {
    let mut violations = Vec::new();
    let base_dir = path
        .parent()
        .expect("agent-doc path should have a parent directory");

    for (line_index, line) in source.lines().enumerate() {
        for raw_target in extract_markdown_links(line) {
            let target = strip_link_title(raw_target.trim());
            if target.is_empty() || is_external_link(target) {
                continue;
            }

            let candidate = target.split('#').next().unwrap_or(target);
            if candidate.is_empty() || candidate.starts_with('#') {
                continue;
            }

            let target_path = if candidate.starts_with('/') {
                repo_root.join(candidate.trim_start_matches('/'))
            } else {
                base_dir.join(candidate)
            };

            if !target_path.exists() {
                violations.push(LinkViolation {
                    file: path.to_path_buf(),
                    line: line_index + 1,
                    target: target.to_string(),
                });
            }
        }
    }

    violations
}

fn collect_command_target_violations(
    repo_root: &Path,
    path: &Path,
    source: &str,
) -> Vec<CommandTargetViolation> {
    let mut violations = Vec::new();

    let mut in_fence = false;
    for (line_index, line) in source.lines().enumerate() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }

        let line_number = line_index + 1;
        if in_fence {
            if let Some(command) = parse_code_reference(path, line_number, line) {
                if !command_target_exists(repo_root, &command) {
                    violations.push(CommandTargetViolation {
                        file: path.to_path_buf(),
                        line: line_number,
                        command,
                    });
                }
            }
        } else {
            for snippet in extract_inline_code_references(line) {
                if let Some(command) = parse_code_reference(path, line_number, &snippet) {
                    if !command_target_exists(repo_root, &command) {
                        violations.push(CommandTargetViolation {
                            file: path.to_path_buf(),
                            line: line_number,
                            command,
                        });
                    }
                }
            }
        }
    }

    violations
}

fn parse_code_reference(_path: &Path, _line_number: usize, snippet: &str) -> Option<String> {
    let statement = snippet.trim();
    if statement.is_empty() {
        return None;
    }

    let token = statement.split_whitespace().next().map(normalize_token)?;
    let token = token.trim_matches(|c| c == '`' || c == ',' || c == ';');
    if token.is_empty()
        || token.starts_with('$')
        || token == "\\"
        || token == "if"
        || token == "for"
        || token == "while"
        || token == "in"
        || token == "then"
        || token == "do"
        || token.starts_with("--")
    {
        return None;
    }

    if token.contains('=') {
        return None;
    }

    if looks_like_command_target(token) {
        Some(token.to_string())
    } else {
        None
    }
}

fn command_target_exists(repo_root: &Path, token: &str) -> bool {
    let normalized = token.trim_start_matches("./").trim_start_matches(".\\");

    let candidate = if token.starts_with('/') {
        PathBuf::from(token)
    } else {
        repo_root.join(normalized)
    };

    candidate.exists()
}

fn extract_markdown_links(line: &str) -> Vec<String> {
    let mut links = Vec::new();
    let mut index = 0;

    while let Some(open) = line[index..].find('[') {
        let open_index = index + open + 1;
        let close_text = match line[open_index..].find(']') {
            Some(pos) => open_index + pos,
            None => break,
        };

        let after_text = match line[close_text + 1..].chars().next() {
            Some('(') => &line[close_text + 1..],
            _ => {
                index = close_text + 1;
                continue;
            }
        };

        let url_and_rest = &after_text[1..];
        let close_url = match url_and_rest.find(')') {
            Some(pos) => pos,
            None => break,
        };
        let raw = url_and_rest[..close_url].trim();
        if !raw.is_empty() {
            links.push(raw.to_string());
        }

        index = close_text + 2 + close_url + 1;
    }

    links
}

fn extract_inline_code_references(line: &str) -> Vec<String> {
    let mut snippets = Vec::new();
    let mut sections = line.split('`');
    while let Some(before) = sections.next() {
        if let Some(raw) = sections.next() {
            if !before.ends_with('\\') {
                snippets.push(raw.to_string());
            }
        } else {
            break;
        }
    }

    snippets
}

fn strip_link_title(raw: &str) -> &str {
    raw.split_whitespace().next().unwrap_or(raw)
}

fn is_external_link(target: &str) -> bool {
    matches!(
        target.split_once(':').map(|(scheme, _)| scheme),
        Some("http") | Some("https") | Some("mailto") | Some("tel")
    ) || target.starts_with('#')
}

fn normalize_token(token: &str) -> &str {
    token.trim_matches(
        &[
            '[', ']', '(', ')', '{', '}', '<', '>', '`', ',', ';', '.', ':',
        ][..],
    )
}

fn looks_like_command_target(token: &str) -> bool {
    if token.starts_with("http") || token.starts_with("https") {
        return false;
    }

    if token.contains('$') {
        return false;
    }

    if token == "export"
        || token == "workdir"
        || token == "git"
        || token == "jq"
        || token == "grep"
        || token == "cargo"
    {
        return false;
    }

    let known_prefixes = ["./", "scripts/"];

    if token.starts_with('~') || token.starts_with('$') || token.starts_with('/') {
        return false;
    }

    if token.ends_with(".md")
        || token.ends_with(".toml")
        || token.ends_with(".json")
        || token.ends_with(".jsonl")
    {
        return false;
    }

    known_prefixes
        .iter()
        .any(|prefix| token.starts_with(prefix))
        || token.ends_with(".sh")
}

#[test]
fn parses_markdown_links_from_agent_docs() {
    let parsed = extract_markdown_links(
        "Use [AGENTS](../AGENTS.md) then run `cargo run -p gardener --bin gardener -- --help`.",
    );
    let normalized = parsed
        .into_iter()
        .map(|link| strip_link_title(&link).to_string())
        .collect::<HashSet<_>>();

    assert!(normalized.contains("../AGENTS.md"));
    assert_eq!(normalized.len(), 1);
}

#[test]
fn parses_inline_code_references() {
    let snippets =
        extract_inline_code_references("Use `scripts/backlog-db.sh` and `LOG_QUERY_BIN stats`.");
    assert_eq!(
        snippets,
        vec!["scripts/backlog-db.sh", "LOG_QUERY_BIN stats"]
    );
}

#[test]
fn filters_non_targets_from_command_line_fragments() {
    assert!(parse_code_reference(Path::new("x"), 1, "$LOG_QUERY_BIN stats").is_none());
    assert!(parse_code_reference(Path::new("x"), 1, "scripts/backlog-db.sh run").is_some());
    assert!(parse_code_reference(Path::new("x"), 1, "workdir=\"/tmp\"").is_none());
}
