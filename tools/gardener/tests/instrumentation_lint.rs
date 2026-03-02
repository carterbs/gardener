use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const MIN_INSTRUMENTATION_COVERAGE: f64 = 90.0;

const EXCLUDED_FILES: &[&str] = &[
    "errors.rs",
    "hotkeys.rs",
    // log_retention.rs is the log-rotation/pruning infrastructure. Calling
    // append_run_log from within it while the write lock is held would deadlock.
    "log_retention.rs",
    "main.rs",
    "output_envelope.rs",
    "priority.rs",
    "prompt_context.rs",
    "prompt_knowledge.rs",
    "prompt_registry.rs",
    "prompts.rs",
    "protocol.rs",
    "quality_domain_catalog.rs",
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

    // dashboard_snapshot is a pure data-aggregation helper; its caller
    // (render / run_worker_pool_fsm) owns the instrumentation.
    if function.name == "dashboard_snapshot" {
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

#[test]
fn linter_e2e_binary_spawn_requires_log_isolation() {
    let tests_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut test_files = Vec::new();
    collect_rust_files(&tests_root, &mut test_files);
    test_files.sort();

    let mut violations = Vec::new();

    for file in &test_files {
        let source = fs::read_to_string(file).expect("read test file");
        if !source.contains("CARGO_BIN_EXE_gardener") {
            continue;
        }
        if !source.contains("GARDENER_LOG_PATH") {
            let relative = file
                .strip_prefix(&tests_root)
                .expect("strip prefix")
                .display()
                .to_string();
            violations.push(relative);
        }
    }

    if !violations.is_empty() {
        let mut message = String::new();
        message.push_str("e2e test isolation linter failed\n\n");
        message.push_str(
            "The following test files spawn the gardener binary (CARGO_BIN_EXE_gardener)\n",
        );
        message.push_str("but do not set GARDENER_LOG_PATH, so the child process writes to the\n");
        message.push_str(
            "live ~/.gardener/otel-logs.jsonl and pollutes production observability data.\n\n",
        );
        message.push_str(
            "Fix: pass .env(\"GARDENER_LOG_PATH\", dir.path().join(\"otel-logs.jsonl\"))\n",
        );
        message.push_str("alongside GARDENER_DB_PATH in every fixture that spawns the binary.\n\n");
        message.push_str("Violations:\n");
        for v in &violations {
            message.push_str(&format!("  - {v}\n"));
        }
        panic!("{message}");
    }
}

#[test]
fn linter_run_triage_with_tty_must_be_ignored() {
    let tests_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut test_files = Vec::new();
    collect_rust_files(&tests_root, &mut test_files);
    test_files.sort();

    let tty_patterns: &[&str] = &["FakeTerminal::new(true)", "is_tty: true"];
    let mut violations = Vec::new();

    for file in &test_files {
        if file.ends_with("instrumentation_lint.rs") {
            continue; // skip self — contains pattern strings as literals
        }
        let source = fs::read_to_string(file).expect("read test file");
        if !source.contains("run_triage(") {
            continue;
        }

        let lines: Vec<&str> = source.lines().collect();

        // Find helper functions that forward a bool to FakeTerminal::new().
        // e.g. `fn basic_runtime_for_toml(…, tty: bool) { … FakeTerminal::new(tty) … }`
        let tty_helpers = find_tty_forwarding_helpers(&lines);

        let mut i = 0;
        while i < lines.len() {
            let trimmed = lines[i].trim();
            if trimmed != "#[test]" {
                i += 1;
                continue;
            }

            // Found #[test] — check if #[ignore] appears before the fn line
            let mut ignored = false;
            let mut fn_line_idx = None;
            for (j, line) in lines.iter().enumerate().skip(i + 1) {
                let attr_trimmed = line.trim();
                if attr_trimmed.starts_with("fn ") || attr_trimmed.starts_with("async fn ") {
                    fn_line_idx = Some(j);
                    break;
                }
                if attr_trimmed.starts_with("#[ignore") {
                    ignored = true;
                }
            }

            let fn_idx = match fn_line_idx {
                Some(idx) => idx,
                None => {
                    i += 1;
                    continue;
                }
            };

            if ignored {
                i = fn_idx + 1;
                continue;
            }

            // Extract function name
            let fn_name = lines[fn_idx]
                .trim()
                .split("fn ")
                .nth(1)
                .unwrap_or("<unknown>")
                .split(|ch: char| ch == '(' || ch.is_whitespace())
                .next()
                .unwrap_or("<unknown>");

            // Scan body (brace-delimited)
            let mut brace_depth = 0isize;
            let mut body_start = None;
            let mut body_end = fn_idx;
            for (k, line) in lines.iter().enumerate().skip(fn_idx) {
                for ch in line.chars() {
                    if ch == '{' {
                        if body_start.is_none() {
                            body_start = Some(k);
                        }
                        brace_depth += 1;
                    } else if ch == '}' {
                        brace_depth -= 1;
                        if brace_depth == 0 {
                            body_end = k;
                        }
                    }
                }
                if brace_depth == 0 && body_start.is_some() {
                    break;
                }
            }

            let body = lines[fn_idx..=body_end].join("\n");
            let calls_run_triage = body.contains("run_triage(");
            let has_direct_tty = tty_patterns.iter().any(|pat| body.contains(pat));
            let has_helper_tty = tty_helpers.iter().any(|helper| {
                // Match e.g. `basic_runtime_for_toml(…, true)`
                let call_prefix = format!("{helper}(");
                body.contains(&call_prefix) && body.contains(", true)")
            });
            if calls_run_triage && (has_direct_tty || has_helper_tty) {
                let relative = file
                    .strip_prefix(&tests_root)
                    .expect("strip prefix")
                    .display()
                    .to_string();
                violations.push(format!("{relative}::{fn_name}"));
            }

            i = body_end + 1;
        }
    }

    if !violations.is_empty() {
        let mut message = String::new();
        message.push_str("TUI safety linter failed\n\n");
        message.push_str("The following tests call run_triage() with a TTY-enabled terminal,\n");
        message.push_str("which triggers run_repo_health_wizard() and launches the real TUI.\n");
        message.push_str("Fix: mark the test #[ignore], use is_tty: false, or make draw() fail\n");
        message.push_str("before the wizard is reached.\n\n");
        message.push_str("Violations:\n");
        for v in &violations {
            message.push_str(&format!("  - {v}\n"));
        }
        panic!("{message}");
    }
}

/// Find non-test helper functions whose signature takes a `bool` and whose body
/// contains `FakeTerminal::new(` — these forward the bool to create a TTY terminal.
fn find_tty_forwarding_helpers(lines: &[&str]) -> Vec<String> {
    let mut helpers = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim();
        // Skip test functions — we only want file-level helpers
        if trimmed == "#[test]" {
            i += 1;
            continue;
        }
        if !(trimmed.starts_with("fn ") || trimmed.starts_with("pub fn ")) {
            i += 1;
            continue;
        }
        let sig = trimmed;
        if !sig.contains("bool") {
            i += 1;
            continue;
        }
        let name = sig
            .split("fn ")
            .nth(1)
            .unwrap_or("")
            .split(|ch: char| ch == '(' || ch == '<' || ch.is_whitespace())
            .next()
            .unwrap_or("")
            .to_string();
        // Scan the function body for FakeTerminal::new(
        let mut brace_depth = 0isize;
        let mut body_started = false;
        let mut body_end = i;
        let mut body_lines = Vec::new();
        for (k, line) in lines.iter().enumerate().skip(i) {
            for ch in line.chars() {
                if ch == '{' {
                    body_started = true;
                    brace_depth += 1;
                } else if ch == '}' {
                    brace_depth -= 1;
                }
            }
            body_lines.push(*line);
            if body_started && brace_depth == 0 {
                body_end = k;
                break;
            }
        }
        let body = body_lines.join("\n");
        if body.contains("FakeTerminal::new(") {
            helpers.push(name);
        }
        i = body_end + 1;
    }
    helpers
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
