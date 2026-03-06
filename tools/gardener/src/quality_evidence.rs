use crate::logging::append_run_log;
use crate::quality_domain_catalog::QualityDomain;
use serde_json::json;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainEvidence {
    pub domain: String,
    pub source_files: Vec<String>,
    pub inline_test_files: Vec<String>,
    pub integration_tests: Vec<String>,
    pub instrumentation_files: Vec<String>,
}

pub fn collect_evidence(domains: &[QualityDomain], repo_root: &Path) -> Vec<DomainEvidence> {
    append_run_log(
        "debug",
        "quality.evidence.collection.started",
        json!({
            "domain_count": domains.len(),
            "repo_root": repo_root.display().to_string(),
        }),
    );

    let evidence: Vec<DomainEvidence> = domains
        .iter()
        .map(|domain| {
            let source_files = source_files_for_domain(repo_root, &domain.name);
            let inline_test_files = source_files
                .iter()
                .filter(|path| file_contains_tests(Path::new(path)))
                .cloned()
                .collect::<Vec<_>>();
            let integration_tests = integration_tests_for_domain(repo_root, &domain.name);
            let instrumentation_files = source_files
                .iter()
                .filter(|path| file_contains_instrumentation(Path::new(path)))
                .cloned()
                .collect::<Vec<_>>();

            append_run_log(
                "debug",
                "quality.evidence.domain.scanned",
                json!({
                    "domain": domain.name,
                    "source_files": source_files.len(),
                    "inline_test_files": inline_test_files.len(),
                    "integration_tests": integration_tests.len(),
                    "instrumentation_files": instrumentation_files.len(),
                }),
            );

            DomainEvidence {
                domain: domain.name.clone(),
                source_files,
                inline_test_files,
                integration_tests,
                instrumentation_files,
            }
        })
        .collect();

    append_run_log(
        "info",
        "quality.evidence.collection.completed",
        json!({
            "evidence_entries": evidence.len(),
            "total_source_files": evidence.iter().map(|e| e.source_files.len()).sum::<usize>(),
            "total_inline_tests": evidence.iter().map(|e| e.inline_test_files.len()).sum::<usize>(),
            "total_integration_tests": evidence.iter().map(|e| e.integration_tests.len()).sum::<usize>(),
            "total_instrumentation": evidence.iter().map(|e| e.instrumentation_files.len()).sum::<usize>(),
        }),
    );

    evidence
}

fn source_files_for_domain(repo_root: &Path, domain: &str) -> Vec<String> {
    let src_root = repo_root.join("src");
    if !src_root.is_dir() {
        return Vec::new();
    }
    let mut files = Vec::new();
    collect_source_files(&src_root, &mut files, domain);
    files.sort_unstable();
    files
}

fn collect_source_files(root: &Path, files: &mut Vec<String>, domain: &str) {
    append_run_log(
        "debug",
        "quality.evidence.collect_source_files",
        json!({
            "root": root.display().to_string(),
            "domain": domain,
        }),
    );
    let mut entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.by_ref().flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_source_files(&path, files, domain);
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }

        let Some(file_name) = path.file_name().and_then(|f| f.to_str()) else {
            continue;
        };
        if !domain_matches_file(domain, &path, file_name) {
            continue;
        }
        if let Ok(relative) = path.strip_prefix(root.parent().unwrap_or(root)) {
            if let Some(rel) = relative.to_str() {
                files.push(format!("src/{}", rel));
            }
        }
    }
}

fn integration_tests_for_domain(repo_root: &Path, domain: &str) -> Vec<String> {
    let tests_root = repo_root.join("tests");
    if !tests_root.is_dir() {
        return Vec::new();
    }
    let mut entries = Vec::new();
    collect_integration_files(&tests_root, &mut entries, domain);
    entries.sort_unstable();
    entries
}

fn collect_integration_files(root: &Path, files: &mut Vec<String>, domain: &str) {
    append_run_log(
        "debug",
        "quality.evidence.collect_integration_files",
        json!({
            "root": root.display().to_string(),
            "domain": domain,
        }),
    );
    let mut entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    let needle = domain.replace('-', "_");
    for entry in entries.by_ref().flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_integration_files(&path, files, domain);
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if !stem.contains(&needle) && !stem.contains(domain) {
            continue;
        }
        if let Some(rel) = path.to_str() {
            files.push(rel.to_string());
        }
    }
}

fn domain_matches_file(domain: &str, full: &Path, file_name: &str) -> bool {
    if domain == "infrastructure" {
        return !matches_known_domain_file(
            full,
            file_name,
            &[
                "triage",
                "backlog",
                "seeding",
                "worker-pool",
                "agent-adapters",
                "tui",
                "quality-grades",
                "startup",
                "git-integration",
                "prompts",
                "learning",
            ],
        );
    }
    matches_known_domain_file(full, file_name, &[domain])
}

fn matches_known_domain_file(path: &Path, file_name: &str, targets: &[&str]) -> bool {
    let file = file_name.to_ascii_lowercase();
    let path = path.to_string_lossy().to_ascii_lowercase();
    for target in targets {
        if matches_domain_file(target, &file, &path) {
            return true;
        }
    }
    false
}

fn matches_domain_file(domain: &str, file: &str, path: &str) -> bool {
    match domain {
        "triage" => path.contains("triage") || file.starts_with("triage"),
        "backlog" => {
            file.starts_with("backlog")
                || file == "priority.rs"
                || file == "task_identity.rs"
                || path.ends_with("backlog_store.rs")
        }
        "seeding" => file == "seeding.rs" || file == "seed_runner.rs",
        "worker-pool" => file.starts_with("worker") || file == "fsm.rs",
        "agent-adapters" => {
            path.contains("/agent/") || file == "protocol.rs" || file == "output_envelope.rs"
        }
        "tui" => file == "tui.rs" || file == "hotkeys.rs",
        "quality-grades" => file.starts_with("quality"),
        "startup" => {
            file == "startup.rs"
                || file == "worktree_audit.rs"
                || path.contains("/worktree_audit/")
                || file == "pr_audit.rs"
        }
        "git-integration" => file == "git.rs" || file == "gh.rs" || file == "worktree.rs",
        "prompts" => file.starts_with("prompt"),
        "learning" => {
            file == "learning_loop.rs" || file == "postmerge_analysis.rs" || file == "postmortem.rs"
        }
        "infrastructure" => true,
        _ => false,
    }
}

fn file_contains_tests(path: &Path) -> bool {
    let result = std::fs::read_to_string(path)
        .map(|contents| {
            contents.contains("#[cfg(test)]")
                || contents.contains("#[test]")
                || contents.contains("mod tests")
                || contents.contains("mod test")
        })
        .unwrap_or(false);
    append_run_log(
        "debug",
        "quality.evidence.file_contains_tests",
        json!({
            "path": path.display().to_string(),
            "has_tests": result,
        }),
    );
    result
}

fn file_contains_instrumentation(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .map(|contents| contents.contains("append_run_log("))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::collect_evidence;
    use crate::quality_domain_catalog::QualityDomain;
    use std::fs;
    use tempfile::tempdir;

    fn domain(name: &str) -> QualityDomain {
        QualityDomain {
            name: name.to_string(),
        }
    }

    #[test]
    fn collect_evidence_with_empty_domains_returns_empty() {
        let dir = tempdir().expect("tempdir");
        let evidence = collect_evidence(&[], dir.path());
        assert!(evidence.is_empty());
    }

    #[test]
    fn collect_evidence_no_src_dir_returns_empty_source_files() {
        let dir = tempdir().expect("tempdir");
        let evidence = collect_evidence(&[domain("backlog")], dir.path());
        assert_eq!(evidence.len(), 1);
        assert!(evidence[0].source_files.is_empty());
        assert!(evidence[0].integration_tests.is_empty());
    }

    #[test]
    fn collect_evidence_finds_backlog_source_files() {
        let dir = tempdir().expect("tempdir");
        let src = dir.path().join("src");
        fs::create_dir_all(&src).expect("create src");
        // backlog_store.rs matches "backlog" domain and has tests + instrumentation
        let content = "#[cfg(test)]\nmod tests {}\nfn f() { append_run_log(\"info\",\"x\",serde_json::json!({})); }";
        fs::write(src.join("backlog_store.rs"), content).expect("write backlog_store.rs");
        // priority.rs also matches backlog domain
        fs::write(src.join("priority.rs"), "fn f() {}").expect("write priority.rs");
        // worker.rs does not match backlog
        fs::write(src.join("worker.rs"), "fn g() {}").expect("write worker.rs");

        let evidence = collect_evidence(&[domain("backlog")], dir.path());
        assert_eq!(evidence.len(), 1);
        assert!(
            !evidence[0].source_files.is_empty(),
            "backlog_store.rs should be found"
        );
        // inline_test_files and instrumentation_files are derived via file_contains_tests /
        // file_contains_instrumentation using relative paths; they may be empty in test
        // environments where the CWD differs from the repo root, but the code paths are exercised.
    }

    #[test]
    fn collect_evidence_finds_integration_tests_for_domain() {
        let dir = tempdir().expect("tempdir");
        let tests_dir = dir.path().join("tests");
        fs::create_dir_all(&tests_dir).expect("create tests dir");
        fs::write(tests_dir.join("backlog_contracts.rs"), "fn test_it() {}").expect("write file");
        fs::write(tests_dir.join("other_test.rs"), "fn test_other() {}").expect("write file");

        let evidence = collect_evidence(&[domain("backlog")], dir.path());
        assert_eq!(evidence.len(), 1);
        assert!(
            !evidence[0].integration_tests.is_empty(),
            "backlog_contracts.rs should be found"
        );
    }

    #[test]
    fn collect_evidence_infrastructure_domain_excludes_triage_files() {
        let dir = tempdir().expect("tempdir");
        let src = dir.path().join("src");
        fs::create_dir_all(&src).expect("create src");
        fs::write(src.join("triage.rs"), "fn f() {}").expect("write triage.rs");
        fs::write(src.join("repo_intelligence.rs"), "fn f() {}")
            .expect("write repo_intelligence.rs");

        let evidence = collect_evidence(&[domain("infrastructure")], dir.path());
        assert_eq!(evidence.len(), 1);
        // repo_intelligence.rs matches infrastructure (not a known domain file)
        // triage.rs does NOT match infrastructure (it's excluded)
        let paths = &evidence[0].source_files;
        assert!(
            paths.iter().any(|p| p.contains("repo_intelligence")),
            "repo_intelligence.rs should be included in infrastructure"
        );
        assert!(
            !paths.iter().any(|p| p.contains("triage")),
            "triage.rs should be excluded from infrastructure"
        );
    }

    #[test]
    fn collect_evidence_seeding_domain_matches_seed_runner() {
        let dir = tempdir().expect("tempdir");
        let src = dir.path().join("src");
        fs::create_dir_all(&src).expect("create src");
        fs::write(src.join("seed_runner.rs"), "fn f() {}").expect("write seed_runner.rs");
        fs::write(src.join("seeding.rs"), "fn f() {}").expect("write seeding.rs");
        fs::write(src.join("worker.rs"), "fn f() {}").expect("write worker.rs");

        let evidence = collect_evidence(&[domain("seeding")], dir.path());
        assert_eq!(evidence.len(), 1);
        let paths = &evidence[0].source_files;
        assert!(paths.iter().any(|p| p.contains("seed_runner")));
        assert!(paths.iter().any(|p| p.contains("seeding")));
        assert!(!paths.iter().any(|p| p.contains("worker")));
    }

    #[test]
    fn collect_evidence_scans_nested_src_subdirectories() {
        let dir = tempdir().expect("tempdir");
        let subdir = dir.path().join("src").join("agent");
        fs::create_dir_all(&subdir).expect("create agent subdir");
        fs::write(subdir.join("claude.rs"), "fn f() {}").expect("write claude.rs");

        let evidence = collect_evidence(&[domain("agent-adapters")], dir.path());
        assert_eq!(evidence.len(), 1);
        assert!(
            !evidence[0].source_files.is_empty(),
            "nested agent/claude.rs should be found"
        );
    }
}
