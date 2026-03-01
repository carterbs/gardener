use std::fs;
use std::path::PathBuf;

const TARGET_FILES: &[&str] = &["worker.rs", "worker_pool.rs"];
const WORKER_LOG_CALL: &str = "append_run_log(";

#[derive(Debug)]
struct CallViolation {
    file: &'static str,
    line: usize,
    event_type: String,
    snippet: String,
}

#[test]
fn worker_log_payloads_include_worker_id() {
    let src_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations = Vec::new();

    for file in TARGET_FILES {
        let path = src_root.join(file);
        let source = fs::read_to_string(&path).expect("read source file");
        let calls = extract_append_run_log_calls(&source);

        for (offset, call) in calls {
            let Some(args) = split_call_args(&call) else {
                continue;
            };

            if args.len() < 3 {
                continue;
            }

            if !payload_has_worker_id(&args[2]) {
                let line = source[..offset].bytes().filter(|b| *b == b'\n').count() + 1;
                violations.push(CallViolation {
                    file,
                    line,
                    event_type: args[1].trim().to_string(),
                    snippet: snippet(call),
                });
            }
        }
    }

    if !violations.is_empty() {
        let mut message = String::new();
        message.push_str(
            "worker log linter failed: worker logging must include \"worker_id\" in json payload\n",
        );
        message.push_str("Missing worker_id:\n");
        for v in violations {
            message.push_str(&format!(
                "  - {file}:{line} :: {event}\n    {snippet}\n",
                file = v.file,
                line = v.line,
                event = v.event_type,
                snippet = v.snippet,
            ));
        }
        panic!("{message}");
    }
}

fn extract_append_run_log_calls(source: &str) -> Vec<(usize, String)> {
    let mut calls = Vec::new();
    let mut cursor = 0usize;
    let call_len = WORKER_LOG_CALL.len();

    while let Some(offset) = source[cursor..].find(WORKER_LOG_CALL) {
        let call_start = cursor + offset;
        let abs_call_start = call_start;
        let call_end = match extract_call_end(source.as_bytes(), call_start + call_len - 1) {
            Some(end) => end,
            None => break,
        };
        let text = source[call_start..call_end].to_string();
        calls.push((abs_call_start, text));
        cursor = call_end;
    }

    calls
}

fn extract_call_end(bytes: &[u8], open_paren_pos: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut i = open_paren_pos;
    let mut in_double_quote = false;
    let mut in_single_quote = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut escaped = false;

    while i < bytes.len() {
        let byte = bytes[i];

        if in_line_comment {
            if byte == b'\n' {
                in_line_comment = false;
            }
            i += 1;
            continue;
        }
        if in_block_comment {
            if byte == b'/' && i > open_paren_pos && bytes[i - 1] == b'*' {
                in_block_comment = false;
            }
            i += 1;
            continue;
        }
        if in_double_quote {
            if escaped {
                escaped = false;
                i += 1;
                continue;
            }
            if byte == b'\\' {
                escaped = true;
                i += 1;
                continue;
            }
            if byte == b'"' {
                in_double_quote = false;
            }
            i += 1;
            continue;
        }
        if in_single_quote {
            if escaped {
                escaped = false;
                i += 1;
                continue;
            }
            if byte == b'\\' {
                escaped = true;
                i += 1;
                continue;
            }
            if byte == b'\'' {
                in_single_quote = false;
            }
            i += 1;
            continue;
        }

        if byte == b'"' {
            in_double_quote = true;
            i += 1;
            continue;
        }
        if byte == b'\'' {
            in_single_quote = true;
            i += 1;
            continue;
        }
        if byte == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            in_line_comment = true;
            i += 2;
            continue;
        }
        if byte == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            in_block_comment = true;
            i += 2;
            continue;
        }

        if byte == b'(' {
            depth += 1;
        } else if byte == b')' {
            depth -= 1;
            if depth == 0 {
                return Some(i + 1);
            }
        }

        i += 1;
    }

    None
}

fn split_call_args(call: &str) -> Option<Vec<String>> {
    let call = call.trim();
    let open_paren = call.find('(')?;
    let close_paren = call.rfind(')')?;
    if close_paren <= open_paren + 1 {
        return Some(Vec::new());
    }

    let args_text = &call[open_paren + 1..close_paren];
    let mut args: Vec<String> = Vec::new();
    let mut current = String::new();

    let mut depth_paren = 0i32;
    let mut depth_brace = 0i32;
    let mut depth_bracket = 0i32;
    let mut in_double_quote = false;
    let mut in_single_quote = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut escaped = false;
    let bytes = args_text.as_bytes();
    let mut i = 0usize;

    while i < bytes.len() {
        let byte = bytes[i];
        let prev = if i > 0 { bytes[i - 1] } else { 0 };

        if in_line_comment {
            if byte == b'\n' {
                in_line_comment = false;
            }
            current.push(byte as char);
            i += 1;
            continue;
        }
        if in_block_comment {
            if byte == b'/' && prev == b'*' {
                in_block_comment = false;
            }
            current.push(byte as char);
            i += 1;
            continue;
        }
        if in_double_quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_double_quote = false;
            }
            current.push(byte as char);
            i += 1;
            continue;
        }
        if in_single_quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'\'' {
                in_single_quote = false;
            }
            current.push(byte as char);
            i += 1;
            continue;
        }

        if byte == b'"' {
            in_double_quote = true;
            current.push(byte as char);
            i += 1;
            continue;
        }
        if byte == b'\'' {
            in_single_quote = true;
            current.push(byte as char);
            i += 1;
            continue;
        }
        if byte == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            in_line_comment = true;
            current.push(byte as char);
            i += 1;
            continue;
        }
        if byte == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            in_block_comment = true;
            current.push(byte as char);
            i += 1;
            continue;
        }

        match byte {
            b'(' => {
                depth_paren += 1;
            }
            b')' => {
                depth_paren -= 1;
            }
            b'{' => {
                depth_brace += 1;
            }
            b'}' => {
                depth_brace -= 1;
            }
            b'[' => {
                depth_bracket += 1;
            }
            b']' => {
                depth_bracket -= 1;
            }
            b',' if depth_paren == 0 && depth_brace == 0 && depth_bracket == 0 => {
                args.push(current.trim().to_string());
                current.clear();
                i += 1;
                continue;
            }
            _ => {}
        }

        current.push(byte as char);
        i += 1;
    }

    if !current.trim().is_empty() {
        args.push(current.trim().to_string());
    }

    Some(args)
}

fn payload_has_worker_id(payload_arg: &str) -> bool {
    if !payload_arg.contains("json!(") {
        return true;
    }
    payload_arg.contains("\"worker_id\"")
}

fn snippet(call: String) -> String {
    call.lines().next().unwrap_or_default().trim().to_string()
}
