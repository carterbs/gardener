use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const MIN_INSTRUMENTATION_COVERAGE: f64 = 90.0;

const EXCLUDED_FILES: &[&str] = &[
    "errors.rs",
    "hotkeys.rs",
    "main.rs",
    "output_envelope.rs",
    "priority.rs",
    "prompt_context.rs",
    "prompt_knowledge.rs",
    "prompt_registry.rs",
    "prompts.rs",
    "protocol.rs",
    "quality_domain_catalog.rs",
    // replay module: recording/replay infrastructure (writes session file, not otel log)
    "replay/mod.rs",
    "replay/recorder.rs",
    "replay/recording.rs",
    "replay/replayer.rs",
    "runtime/mod.rs",
    "task_identity.rs",
    "tui.rs",
    "types.rs",
    "worker_identity.rs",
];

const INSTRUMENTATION_MARKERS: &[&str] = &["append_run_log(", "structured_fallback_line("];
const SIDE_EFFECT_MARKERS: &[&str] = &[
    "Command::new(",
    ".spawn(",
    ".status(",
    ".output(",
    "fs::",
    "std::fs::",
    "OpenOptions::",
    ".execute(",
    ".query(",
    ".query_row(",
    ".write_line(",
    ".send(",
    ".recv(",
];

#[derive(Debug)]
struct FunctionStats {
    name: String,
    signature_line: usize,
    signature: String,
    body: String,
    significant_lines: usize,
    instrumented: bool,
}

#[derive(Debug)]
struct FileStats {
    path: String,
    eligible_functions: usize,
    instrumented_functions: usize,
    coverage: f64,
    missing_functions: Vec<FunctionStats>,
}

#[test]
fn linter_instrumentation_coverage_by_file() {
    let src_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut rust_files = Vec::new();
    collect_rust_files(&src_root, &mut rust_files);
    rust_files.sort();

    let mut failures = Vec::new();
    let mut graded = BTreeMap::new();

    for file in rust_files {
        let relative = file
            .strip_prefix(&src_root)
            .expect("strip prefix")
            .to_string_lossy()
            .replace('\\', "/");

        if EXCLUDED_FILES.iter().any(|excluded| relative == *excluded) {
            continue;
        }

        let source = fs::read_to_string(&file).expect("read source file");
        let stats = grade_file(&relative, &source);
        if stats.eligible_functions == 0 {
            continue;
        }

        graded.insert(relative.clone(), stats.coverage);

        if stats.coverage < MIN_INSTRUMENTATION_COVERAGE {
            failures.push(stats);
        }
    }

    if !failures.is_empty() {
        let mut message = String::new();
        message.push_str("instrumentation coverage linter failed\n");
        message.push_str(&format!(
            "minimum per-file coverage: {:.1}%\n\n",
            MIN_INSTRUMENTATION_COVERAGE
        ));

        message.push_str("Per-file grades:\n");
        for (path, coverage) in &graded {
            message.push_str(&format!("  - {path}: {:.1}%\n", coverage));
        }

        message.push_str("\nFiles below threshold:\n");
        for file in &failures {
            message.push_str(&format!(
                "  - {}: {:.1}% ({} / {} eligible functions instrumented)\n",
                file.path, file.coverage, file.instrumented_functions, file.eligible_functions
            ));
            for missing in file.missing_functions.iter().take(5) {
                message.push_str(&format!(
                    "      - {} (line {}, {} significant lines)\n",
                    missing.name, missing.signature_line, missing.significant_lines
                ));
            }
            if file.missing_functions.len() > 5 {
                message.push_str(&format!(
                    "      - ... {} more\n",
                    file.missing_functions.len() - 5
                ));
            }
        }

        panic!("{message}");
    }
}

fn grade_file(path: &str, source: &str) -> FileStats {
    let sanitized = remove_test_modules(source);
    let functions = extract_functions(&sanitized);

    let mut eligible_functions = 0usize;
    let mut instrumented_functions = 0usize;
    let mut missing = Vec::new();

    for function in functions {
        if !is_eligible_function(&function) {
            continue;
        }

        eligible_functions += 1;
        if function.instrumented {
            instrumented_functions += 1;
        } else {
            missing.push(function);
        }
    }

    let coverage = if eligible_functions == 0 {
        100.0
    } else {
        (instrumented_functions as f64 / eligible_functions as f64) * 100.0
    };

    FileStats {
        path: path.to_string(),
        eligible_functions,
        instrumented_functions,
        coverage,
        missing_functions: missing,
    }
}

fn remove_test_modules(source: &str) -> String {
    let mut output = String::new();
    let mut lines = source.lines().peekable();

    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if trimmed == "#[cfg(test)]" {
            if let Some(next_line) = lines.peek() {
                if next_line.trim_start().starts_with("mod tests") {
                    let mut brace_depth = 0isize;
    for test_line in lines.by_ref() {
        for ch in test_line.chars() {
            if ch == '{' {
                brace_depth += 1;
            } else if ch == '}' {
                                brace_depth -= 1;
                            }
                        }
                        if brace_depth <= 0 && test_line.contains('}') {
                            break;
                        }
                    }
                    continue;
                }
            }
        }

        output.push_str(line);
        output.push('\n');
    }

    output
}

fn extract_functions(source: &str) -> Vec<FunctionStats> {
    let lines: Vec<&str> = source.lines().collect();
    let mut out = Vec::new();
    let mut i = 0usize;

    while i < lines.len() {
        let line = lines[i];
        if !looks_like_fn_start(line) {
            i += 1;
            continue;
        }

        let signature_line = i + 1;
        let signature_text = line.trim().to_string();
        let mut j = i;
        let mut found_open_brace = None;
        let mut saw_semicolon_before_body = false;

        while j < lines.len() {
            let current = lines[j];
            if current.contains('{') {
                found_open_brace = Some(j);
                break;
            }
            if current.contains(';') {
                saw_semicolon_before_body = true;
                break;
            }
            j += 1;
        }

        if saw_semicolon_before_body || found_open_brace.is_none() {
            i += 1;
            continue;
        }

        let open_line = found_open_brace.expect("open brace set");
        let mut brace_depth = 0isize;
        let mut close_line = open_line;

        'scan: for (line_idx, body_line) in lines.iter().enumerate().skip(open_line) {
            for ch in body_line.chars() {
                if ch == '{' {
                    brace_depth += 1;
                } else if ch == '}' {
                    brace_depth -= 1;
                    if brace_depth == 0 {
                        close_line = line_idx;
                        break 'scan;
                    }
                }
            }
        }

        let function_body = lines[open_line..=close_line].join("\n");
        let significant_lines = function_body
            .lines()
            .filter(|body_line| {
                let trimmed = body_line.trim();
                !trimmed.is_empty()
                    && !trimmed.starts_with("//")
                    && !trimmed.starts_with("/*")
                    && trimmed != "{"
                    && trimmed != "}"
            })
            .count();

        let instrumented = INSTRUMENTATION_MARKERS
            .iter()
            .any(|marker| function_body.contains(marker));

        out.push(FunctionStats {
            name: extract_fn_name(&signature_text),
            signature_line,
            signature: signature_text,
            body: function_body,
            significant_lines,
            instrumented,
        });

        i = close_line + 1;
    }

    out
}

fn is_eligible_function(function: &FunctionStats) -> bool {
    if function.significant_lines < 5 {
        return false;
    }

    if function.name == "default" || function.name == "new" {
        return false;
    }

    if function.name.starts_with("parse_")
        || function.name.starts_with("extract_")
        || function.name.starts_with("render_")
        || function.name.starts_with("build_")
    {
        return false;
    }

    function.instrumented
        || function_signature_looks_effectful(&function.signature)
        || function_contains_side_effect_markers(&function.body)
}

fn function_signature_looks_effectful(signature: &str) -> bool {
    signature.contains("async fn")
        || signature.contains("-> Result<")
        || signature.contains("-> std::result::Result<")
        || signature.contains("-> GardenerError")
        || signature.contains("-> anyhow::Result<")
}

fn function_contains_side_effect_markers(body: &str) -> bool {
    SIDE_EFFECT_MARKERS
        .iter()
        .any(|marker| body.contains(marker))
}

fn looks_like_fn_start(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with("//") || trimmed.starts_with("#") {
        return false;
    }

    trimmed.starts_with("fn ")
        || trimmed.starts_with("pub fn ")
        || trimmed.starts_with("pub(crate) fn ")
        || trimmed.starts_with("pub(super) fn ")
        || trimmed.starts_with("pub async fn ")
        || trimmed.starts_with("async fn ")
        || trimmed.starts_with("pub(crate) async fn ")
        || trimmed.starts_with("pub(super) async fn ")
}

fn extract_fn_name(signature: &str) -> String {
    let after_fn = signature.split("fn ").nth(1).unwrap_or("<unknown>").trim();
    after_fn
        .split(|ch: char| ch == '(' || ch.is_whitespace() || ch == '<')
        .next()
        .unwrap_or("<unknown>")
        .to_string()
}

fn collect_rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).expect("read directory");
    for entry in entries {
        let entry = entry.expect("read dir entry");
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, out);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}
