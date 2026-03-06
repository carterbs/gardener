use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageParserOutput {
    pub artifacts_found: Vec<String>,
    pub artifacts_parsed: Vec<String>,
    pub parse_errors: Vec<String>,
    pub coverage_available: bool,
    pub summary: Option<CoverageSummary>,
    pub per_file: Vec<FileCoverage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageSummary {
    pub total_lines: usize,
    pub covered_lines: usize,
    pub coverage_percent: f64,
    pub source_format: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileCoverage {
    pub path: String,
    pub total_lines: usize,
    pub covered_lines: usize,
    pub coverage_percent: f64,
}

/// Well-known coverage artifact paths to search for.
const COVERAGE_ARTIFACT_PATHS: &[&str] = &[
    // lcov
    "coverage/lcov.info",
    "lcov.info",
    // Istanbul JSON
    "coverage/coverage-final.json",
    ".nyc_output/coverage.json",
    "coverage/coverage-summary.json",
    // Cobertura XML
    "coverage/cobertura-coverage.xml",
    "cobertura.xml",
    "coverage.xml",
    // Tarpaulin
    "tarpaulin-report.json",
    // Go
    "cover.out",
    "coverage.out",
];

/// Parse all discoverable coverage artifacts in a repository.
pub fn parse_coverage(repo_path: &Path) -> CoverageParserOutput {
    let mut artifacts_found = Vec::new();
    let mut artifacts_parsed = Vec::new();
    let mut parse_errors = Vec::new();
    let mut all_file_coverage: BTreeMap<String, FileCoverage> = BTreeMap::new();
    let mut best_summary: Option<(u8, CoverageSummary)> = None; // (precedence, summary)

    for artifact_path in COVERAGE_ARTIFACT_PATHS {
        let full_path = repo_path.join(artifact_path);
        if !full_path.is_file() {
            continue;
        }
        artifacts_found.push(artifact_path.to_string());

        let content = match std::fs::read_to_string(&full_path) {
            Ok(c) => c,
            Err(e) => {
                parse_errors.push(format!("{artifact_path}: read error: {e}"));
                continue;
            }
        };

        let result = if artifact_path.ends_with(".json") && artifact_path.contains("istanbul")
            || artifact_path.contains("coverage-final")
            || artifact_path.contains("nyc_output")
            || artifact_path.contains("coverage-summary")
        {
            parse_istanbul_json(&content, artifact_path)
        } else if artifact_path.ends_with("lcov.info") {
            parse_lcov(&content, artifact_path)
        } else if artifact_path.ends_with(".xml") {
            parse_cobertura_xml(&content, artifact_path)
        } else if artifact_path.contains("tarpaulin") {
            parse_tarpaulin_json(&content, artifact_path)
        } else if artifact_path.ends_with(".out") {
            parse_go_cover(&content, artifact_path)
        } else if artifact_path.ends_with(".json") {
            parse_istanbul_json(&content, artifact_path)
        } else {
            Err(format!("{artifact_path}: unknown format"))
        };

        match result {
            Ok(parsed) => {
                artifacts_parsed.push(artifact_path.to_string());
                // Merge per-file coverage (highest coverage wins)
                for fc in &parsed.per_file {
                    let existing = all_file_coverage.get(&fc.path);
                    if existing.is_none_or(|e| fc.coverage_percent > e.coverage_percent) {
                        all_file_coverage.insert(fc.path.clone(), fc.clone());
                    }
                }
                // Precedence: Istanbul JSON (0) > lcov (1) > Cobertura (2) > Tarpaulin (3) > Go (4)
                if best_summary
                    .as_ref()
                    .is_none_or(|(p, _)| parsed.precedence < *p)
                {
                    best_summary = Some((parsed.precedence, parsed.summary));
                }
            }
            Err(e) => {
                parse_errors.push(e);
            }
        }
    }

    let per_file: Vec<FileCoverage> = all_file_coverage.into_values().collect();
    let coverage_available = !artifacts_parsed.is_empty();

    // If we parsed files but have no summary from a format parser, compute from per-file
    let summary = best_summary.map(|(_, s)| s).or_else(|| {
        if per_file.is_empty() {
            return None;
        }
        let total: usize = per_file.iter().map(|f| f.total_lines).sum();
        let covered: usize = per_file.iter().map(|f| f.covered_lines).sum();
        let pct = if total > 0 {
            (covered as f64 / total as f64) * 100.0
        } else {
            0.0
        };
        Some(CoverageSummary {
            total_lines: total,
            covered_lines: covered,
            coverage_percent: pct,
            source_format: "computed".to_string(),
        })
    });

    CoverageParserOutput {
        artifacts_found,
        artifacts_parsed,
        parse_errors,
        coverage_available,
        summary,
        per_file,
    }
}

struct ParsedCoverage {
    precedence: u8,
    summary: CoverageSummary,
    per_file: Vec<FileCoverage>,
}

fn parse_lcov(content: &str, artifact_path: &str) -> Result<ParsedCoverage, String> {
    let mut per_file: Vec<FileCoverage> = Vec::new();
    let mut current_file: Option<String> = None;
    let mut lines_found = 0usize;
    let mut lines_hit = 0usize;
    let mut total_found = 0usize;
    let mut total_hit = 0usize;

    for line in content.lines() {
        let line = line.trim();
        if let Some(path) = line.strip_prefix("SF:") {
            current_file = Some(path.to_string());
            lines_found = 0;
            lines_hit = 0;
        } else if let Some(val) = line.strip_prefix("LF:") {
            lines_found = val.parse().unwrap_or(0);
        } else if let Some(val) = line.strip_prefix("LH:") {
            lines_hit = val.parse().unwrap_or(0);
        } else if line == "end_of_record" {
            if let Some(path) = current_file.take() {
                let pct = if lines_found > 0 {
                    (lines_hit as f64 / lines_found as f64) * 100.0
                } else {
                    0.0
                };
                total_found += lines_found;
                total_hit += lines_hit;
                per_file.push(FileCoverage {
                    path,
                    total_lines: lines_found,
                    covered_lines: lines_hit,
                    coverage_percent: pct,
                });
            }
        }
    }

    if per_file.is_empty() {
        return Err(format!(
            "{artifact_path}: no coverage records found in lcov"
        ));
    }

    let pct = if total_found > 0 {
        (total_hit as f64 / total_found as f64) * 100.0
    } else {
        0.0
    };

    Ok(ParsedCoverage {
        precedence: 1,
        summary: CoverageSummary {
            total_lines: total_found,
            covered_lines: total_hit,
            coverage_percent: pct,
            source_format: "lcov".to_string(),
        },
        per_file,
    })
}

fn parse_istanbul_json(content: &str, artifact_path: &str) -> Result<ParsedCoverage, String> {
    // Istanbul coverage-final.json is a map of file path → coverage info
    let parsed: serde_json::Value = serde_json::from_str(content)
        .map_err(|e| format!("{artifact_path}: JSON parse error: {e}"))?;

    let obj = parsed
        .as_object()
        .ok_or_else(|| format!("{artifact_path}: expected JSON object"))?;

    let mut per_file = Vec::new();
    let mut total_stmts = 0usize;
    let mut covered_stmts = 0usize;

    // Extract the "total" bucket separately so we can use it as fallback
    let total_bucket = obj.get("total");

    for (file_path, coverage_data) in obj {
        // Skip synthetic "total" bucket from coverage-summary.json
        if file_path == "total" {
            continue;
        }
        // Try statement map approach (coverage-final.json format)
        if let Some(s) = coverage_data.get("s") {
            if let Some(s_map) = s.as_object() {
                let file_total = s_map.len();
                let file_covered = s_map
                    .values()
                    .filter(|v| v.as_u64().unwrap_or(0) > 0)
                    .count();
                total_stmts += file_total;
                covered_stmts += file_covered;
                let pct = if file_total > 0 {
                    (file_covered as f64 / file_total as f64) * 100.0
                } else {
                    0.0
                };
                per_file.push(FileCoverage {
                    path: file_path.clone(),
                    total_lines: file_total,
                    covered_lines: file_covered,
                    coverage_percent: pct,
                });
                continue;
            }
        }
        // Try coverage-summary.json format
        if let (Some(total), Some(covered)) = (
            coverage_data
                .pointer("/lines/total")
                .and_then(|v| v.as_u64()),
            coverage_data
                .pointer("/lines/covered")
                .and_then(|v| v.as_u64()),
        ) {
            total_stmts += total as usize;
            covered_stmts += covered as usize;
            let pct = if total > 0 {
                (covered as f64 / total as f64) * 100.0
            } else {
                0.0
            };
            per_file.push(FileCoverage {
                path: file_path.clone(),
                total_lines: total as usize,
                covered_lines: covered as usize,
                coverage_percent: pct,
            });
        }
    }

    // If no per-file entries but a "total" bucket exists, use it as aggregate summary
    if per_file.is_empty() {
        if let Some(totals) = total_bucket {
            if let (Some(total), Some(covered)) = (
                totals.pointer("/lines/total").and_then(|v| v.as_u64()),
                totals.pointer("/lines/covered").and_then(|v| v.as_u64()),
            ) {
                let pct = if total > 0 {
                    (covered as f64 / total as f64) * 100.0
                } else {
                    0.0
                };
                return Ok(ParsedCoverage {
                    precedence: 0,
                    summary: CoverageSummary {
                        total_lines: total as usize,
                        covered_lines: covered as usize,
                        coverage_percent: pct,
                        source_format: "istanbul".to_string(),
                    },
                    per_file: Vec::new(),
                });
            }
        }
        return Err(format!(
            "{artifact_path}: no coverage data found in Istanbul JSON"
        ));
    }

    let pct = if total_stmts > 0 {
        (covered_stmts as f64 / total_stmts as f64) * 100.0
    } else {
        0.0
    };

    Ok(ParsedCoverage {
        precedence: 0,
        summary: CoverageSummary {
            total_lines: total_stmts,
            covered_lines: covered_stmts,
            coverage_percent: pct,
            source_format: "istanbul".to_string(),
        },
        per_file,
    })
}

fn parse_cobertura_xml(content: &str, artifact_path: &str) -> Result<ParsedCoverage, String> {
    // Simple line-based XML parsing for Cobertura format (no full XML parser dep)
    let mut per_file = Vec::new();
    let mut total_lines = 0usize;
    let mut covered_lines = 0usize;

    // Look for <package> and <class> elements with line-rate attributes
    for line in content.lines() {
        let trimmed = line.trim();

        // Parse <class> elements for per-file coverage
        if trimmed.starts_with("<class ") || trimmed.starts_with("<class\t") {
            let filename = extract_xml_attr(trimmed, "filename");
            let line_rate =
                extract_xml_attr(trimmed, "line-rate").and_then(|r| r.parse::<f64>().ok());

            if let (Some(filename), Some(rate)) = (filename, line_rate) {
                // Estimate lines from complexity or just record the rate
                per_file.push(FileCoverage {
                    path: filename,
                    total_lines: 0,
                    covered_lines: 0,
                    coverage_percent: rate * 100.0,
                });
            }
        }

        // Parse top-level <coverage> element for summary
        if trimmed.starts_with("<coverage ") {
            if let Some(rate_str) = extract_xml_attr(trimmed, "line-rate") {
                if let Ok(rate) = rate_str.parse::<f64>() {
                    // Extract lines-valid and lines-covered if available
                    let valid = extract_xml_attr(trimmed, "lines-valid")
                        .and_then(|v| v.parse::<usize>().ok())
                        .unwrap_or(0);
                    let covered = extract_xml_attr(trimmed, "lines-covered")
                        .and_then(|v| v.parse::<usize>().ok())
                        .unwrap_or(0);
                    if valid > 0 {
                        total_lines = valid;
                        covered_lines = covered;
                    } else {
                        // Fallback: just use the rate
                        total_lines = 100;
                        covered_lines = (rate * 100.0) as usize;
                    }
                }
            }
        }
    }

    if total_lines == 0 && per_file.is_empty() {
        return Err(format!(
            "{artifact_path}: no coverage data found in Cobertura XML"
        ));
    }

    let pct = if total_lines > 0 {
        (covered_lines as f64 / total_lines as f64) * 100.0
    } else if !per_file.is_empty() {
        per_file.iter().map(|f| f.coverage_percent).sum::<f64>() / per_file.len() as f64
    } else {
        0.0
    };

    Ok(ParsedCoverage {
        precedence: 2,
        summary: CoverageSummary {
            total_lines,
            covered_lines,
            coverage_percent: pct,
            source_format: "cobertura".to_string(),
        },
        per_file,
    })
}

fn parse_tarpaulin_json(content: &str, artifact_path: &str) -> Result<ParsedCoverage, String> {
    // Tarpaulin JSON has a top-level array of file entries or an object with "files" key
    let parsed: serde_json::Value = serde_json::from_str(content)
        .map_err(|e| format!("{artifact_path}: JSON parse error: {e}"))?;

    let files_array = if let Some(arr) = parsed.as_array() {
        arr.clone()
    } else if let Some(arr) = parsed.get("files").and_then(|f| f.as_array()) {
        arr.clone()
    } else {
        return Err(format!(
            "{artifact_path}: expected array or object with 'files' key"
        ));
    };

    let mut per_file = Vec::new();
    let mut total = 0usize;
    let mut covered = 0usize;

    for entry in &files_array {
        let path = entry
            .get("path")
            .or_else(|| entry.get("filename"))
            .and_then(|p| p.as_str())
            .unwrap_or("unknown")
            .to_string();

        let traces = entry.get("traces").and_then(|t| t.as_array());
        if let Some(traces) = traces {
            let file_total = traces.len();
            let file_covered = traces
                .iter()
                .filter(|t| {
                    t.get("hits")
                        .or_else(|| t.get("stats").and_then(|s| s.get("Line")))
                        .and_then(|h| h.as_u64())
                        .unwrap_or(0)
                        > 0
                })
                .count();
            total += file_total;
            covered += file_covered;
            let pct = if file_total > 0 {
                (file_covered as f64 / file_total as f64) * 100.0
            } else {
                0.0
            };
            per_file.push(FileCoverage {
                path,
                total_lines: file_total,
                covered_lines: file_covered,
                coverage_percent: pct,
            });
        }
    }

    if per_file.is_empty() {
        return Err(format!(
            "{artifact_path}: no file entries found in Tarpaulin JSON"
        ));
    }

    let pct = if total > 0 {
        (covered as f64 / total as f64) * 100.0
    } else {
        0.0
    };

    Ok(ParsedCoverage {
        precedence: 3,
        summary: CoverageSummary {
            total_lines: total,
            covered_lines: covered,
            coverage_percent: pct,
            source_format: "tarpaulin".to_string(),
        },
        per_file,
    })
}

fn parse_go_cover(content: &str, artifact_path: &str) -> Result<ParsedCoverage, String> {
    // Go cover.out format: mode line, then file:startLine.startCol,endLine.endCol numStmts count
    let mut per_file_map: BTreeMap<String, (usize, usize)> = BTreeMap::new();

    for line in content.lines() {
        if line.starts_with("mode:") {
            continue;
        }
        // Parse: file:start,end numStmts count
        let parts: Vec<&str> = line.splitn(2, ':').collect();
        if parts.len() != 2 {
            continue;
        }
        let file_path = parts[0].to_string();
        let rest = parts[1];
        // Split on whitespace to get the count (last token)
        let tokens: Vec<&str> = rest.split_whitespace().collect();
        if tokens.len() < 2 {
            continue;
        }
        let num_stmts: usize = tokens
            .get(tokens.len() - 2)
            .and_then(|s| s.parse().ok())
            .unwrap_or(1);
        let count: usize = tokens.last().and_then(|s| s.parse().ok()).unwrap_or(0);

        let entry = per_file_map.entry(file_path).or_insert((0, 0));
        entry.0 += num_stmts;
        if count > 0 {
            entry.1 += num_stmts;
        }
    }

    if per_file_map.is_empty() {
        return Err(format!(
            "{artifact_path}: no coverage data found in Go cover profile"
        ));
    }

    let mut per_file = Vec::new();
    let mut total = 0usize;
    let mut covered = 0usize;

    for (path, (file_total, file_covered)) in &per_file_map {
        total += file_total;
        covered += file_covered;
        let pct = if *file_total > 0 {
            (*file_covered as f64 / *file_total as f64) * 100.0
        } else {
            0.0
        };
        per_file.push(FileCoverage {
            path: path.clone(),
            total_lines: *file_total,
            covered_lines: *file_covered,
            coverage_percent: pct,
        });
    }

    let pct = if total > 0 {
        (covered as f64 / total as f64) * 100.0
    } else {
        0.0
    };

    Ok(ParsedCoverage {
        precedence: 4,
        summary: CoverageSummary {
            total_lines: total,
            covered_lines: covered,
            coverage_percent: pct,
            source_format: "go_cover".to_string(),
        },
        per_file,
    })
}

/// Extract a simple XML attribute value from an element line.
fn extract_xml_attr(line: &str, attr_name: &str) -> Option<String> {
    let needle = format!("{attr_name}=\"");
    let start = line.find(&needle)? + needle.len();
    let rest = &line[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn parse_coverage_no_artifacts() {
        let dir = tempdir().expect("tempdir");
        let output = parse_coverage(dir.path());
        assert!(!output.coverage_available);
        assert!(output.artifacts_found.is_empty());
    }

    #[test]
    fn parse_lcov_basic() {
        let dir = tempdir().expect("tempdir");
        let lcov_content = "SF:src/main.rs\nLF:10\nLH:8\nend_of_record\n";
        fs::write(dir.path().join("lcov.info"), lcov_content).expect("write");
        let output = parse_coverage(dir.path());
        assert!(output.coverage_available);
        assert_eq!(output.per_file.len(), 1);
        assert_eq!(output.per_file[0].total_lines, 10);
        assert_eq!(output.per_file[0].covered_lines, 8);
    }

    #[test]
    fn parse_cobertura_basic() {
        let dir = tempdir().expect("tempdir");
        let xml = r#"<?xml version="1.0"?>
<coverage line-rate="0.85" lines-valid="100" lines-covered="85">
  <packages>
    <package>
      <classes>
        <class filename="src/lib.rs" line-rate="0.90"/>
      </classes>
    </package>
  </packages>
</coverage>"#;
        fs::write(dir.path().join("cobertura.xml"), xml).expect("write");
        let output = parse_coverage(dir.path());
        assert!(output.coverage_available);
        let summary = output.summary.expect("summary should be present");
        assert_eq!(summary.total_lines, 100);
        assert_eq!(summary.covered_lines, 85);
    }
}
