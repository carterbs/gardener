use crate::config::AppConfig;
use crate::errors::GardenerError;
use crate::logging::append_run_log;
use crate::protocol::AgentEvent;
use crate::repo_intelligence::RepoIntelligenceProfile;
use crate::runtime::ProcessRunner;
use crate::seed_runner::{run_legacy_seed_runner_v1_with_events, SeedTask};
use crate::types::RuntimeScope;
use serde_json::json;
use std::fmt::Write as _;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedPromptContext {
    pub primary_gap: String,
    pub readiness_score: i64,
    pub readiness_grade: String,
    pub quality_doc: String,
    pub agents_md: String,
    pub claude_md: String,
    pub docs_listing: String,
    pub quality_risks: String,
}

pub fn build_seed_prompt(
    profile: &RepoIntelligenceProfile,
    quality_doc: &str,
    scope: &RuntimeScope,
) -> String {
    let context = build_seed_prompt_context(profile, quality_doc, scope);
    build_seed_prompt_v2(&context)
}

pub fn build_seed_prompt_v2(context: &SeedPromptContext) -> String {
    let mut out = String::new();
    let mut quality_risks = context.quality_risks.clone();
    if quality_risks.is_empty() {
        quality_risks.push_str(
            "No parseable coverage rows found in quality report; infer risk from repository signals.",
        );
    }

    out.push_str("You are the Gardener backlog seeding worker.\n");
    out.push_str(
        "Goal: generate precise, high-signal repo work for future agents and reduce measurable quality risk.\n\n",
    );

    out.push_str("System framing\n");
    out.push_str("- Do not invent nonexistent files, architecture, or conventions.\n");
    out.push_str(
        "- Use AGENTS.md, CLAUDE.md, docs listing, and quality grades as source of truth.\n",
    );
    out.push_str("- Emit tasks that can be picked up immediately by a runtime worker without additional context assumptions.\n");
    out.push_str(
        "- Prefer changes that improve repository legibility, automation, and reliability.\n\n",
    );

    out.push_str("Inputs\n");
    out.push_str(&format!(
        "- primary_gap: {}\n- readiness_score: {}\n- readiness_grade: {}\n\n",
        context.primary_gap, context.readiness_score, context.readiness_grade
    ));
    out.push_str("Quality risks extracted from report\n");
    out.push_str(&quality_risks);
    out.push('\n');
    out.push_str("Relevant repo anchors\n");
    out.push_str("1) AGENTS.md\n");
    if context.agents_md.is_empty() {
        out.push_str("No AGENTS.md found.\n");
    } else {
        out.push_str(&context.agents_md);
        out.push('\n');
    }
    out.push_str("2) CLAUDE.md\n");
    if context.claude_md.is_empty() {
        out.push_str("No CLAUDE.md found.\n");
    } else {
        out.push_str(&context.claude_md);
        out.push('\n');
    }
    out.push_str("3) docs/\n");
    if context.docs_listing.is_empty() {
        out.push_str("No docs directory found or readable.\n");
    } else {
        out.push_str(&context.docs_listing);
    }
    out.push('\n');

    out.push_str("Task contract\n");
    out.push_str(
        "Return exactly 10 tasks in one JSON payload. Prefer a practical mix of immediate fixes and cleanup debt.\n",
    );
    out.push_str("- Each task must be in the exact format listed in Output contract.\n");
    out.push_str("- At least 2 tasks should map to primary_gap.\n");
    out.push_str("- At least 2 tasks should be cleanup/debt reduction tasks.\n");
    out.push_str("- priority must be one of P0, P1, P2.\n");
    out.push_str("- domain should be concrete and align to discovered file families.\n");
    out.push_str("- rationale should state the immediate quality signal and why now.\n\n");

    out.push_str("Output contract\n");
    out.push_str(
        "Respond with strict JSON only. Top-level keys: schema_version, state, payload.\n",
    );
    out.push_str("schema_version must be 1.\n");
    out.push_str("state must be seeding.\n");
    out.push_str("payload.tasks is an array of SeedTask objects.\n");
    out.push_str("Each object must include: title, details, rationale, domain, priority.\n\n");

    out.push_str("SeedTask schema\n");
    out.push_str("- title: concise actionable sentence\n");
    out.push_str("- details: 1-3 sentence implementation scope and expected outcome\n");
    out.push_str("- rationale: 1-2 sentence why this task improves readiness/quality now\n");
    out.push_str("- domain: one of triage, backlog, seeding, worker-pool, agent-adapters, tui, quality-grades, startup, git-integration, prompts, learning, infrastructure\n");
    out.push_str("- priority: P0|P1|P2\n\n");

    out.push_str("Example (format only; do not copy text)\n");
    out.push_str(
        r#"{"schema_version":1,"state":"seeding","payload":{"tasks":[{"title":"Add integration coverage for startup seeding fallback","details":"Identify seeding edge cases and add regression tests around fallback trigger conditions.","rationale":"This reduces reseed risk and improves startup reliability.","domain":"startup","priority":"P1"}]}}"#,
    );
    out.push('\n');

    out.push_str("\nQuality doc (truncated for prompt budget)\n");
    out.push_str(&context.quality_doc);

    out
}

pub fn seed_backlog_if_needed(
    process_runner: &dyn ProcessRunner,
    scope: &RuntimeScope,
    cfg: &AppConfig,
    profile: &RepoIntelligenceProfile,
    quality_doc: &str,
) -> Result<Vec<SeedTask>, GardenerError> {
    seed_backlog_if_needed_with_events(process_runner, scope, cfg, profile, quality_doc, None)
}

#[allow(clippy::too_many_arguments)]
pub fn seed_backlog_if_needed_with_events(
    process_runner: &dyn ProcessRunner,
    scope: &RuntimeScope,
    cfg: &AppConfig,
    profile: &RepoIntelligenceProfile,
    quality_doc: &str,
    mut on_event: Option<&mut dyn FnMut(&AgentEvent)>,
) -> Result<Vec<SeedTask>, GardenerError> {
    append_run_log(
        "info",
        "seeding.started",
        json!({
            "backend": format!("{:?}", cfg.seeding.backend),
            "model": cfg.seeding.model,
            "primary_gap": profile.agent_readiness.primary_gap,
            "readiness_score": profile.agent_readiness.readiness_score,
            "working_dir": scope.working_dir.display().to_string(),
        }),
    );

    let prompt = build_seed_prompt(profile, quality_doc, scope);
    let result = if let Some(sink) = on_event.as_mut() {
        run_legacy_seed_runner_v1_with_events(
            process_runner,
            scope,
            cfg.seeding.backend,
            &cfg.seeding.model,
            &prompt,
            Some(*sink),
        )
    } else {
        run_legacy_seed_runner_v1_with_events(
            process_runner,
            scope,
            cfg.seeding.backend,
            &cfg.seeding.model,
            &prompt,
            None,
        )
    };
    match &result {
        Ok(tasks) => {
            append_run_log(
                "info",
                "seeding.completed",
                json!({
                    "backend": format!("{:?}", cfg.seeding.backend),
                    "model": cfg.seeding.model,
                    "task_count": tasks.len(),
                }),
            );
        }
        Err(e) => {
            append_run_log(
                "error",
                "seeding.failed",
                json!({
                    "backend": format!("{:?}", cfg.seeding.backend),
                    "model": cfg.seeding.model,
                    "error": e.to_string(),
                }),
            );
        }
    }
    result
}

fn build_seed_prompt_context(
    profile: &RepoIntelligenceProfile,
    quality_doc: &str,
    scope: &RuntimeScope,
) -> SeedPromptContext {
    let repo_root = scope
        .repo_root
        .as_ref()
        .cloned()
        .unwrap_or_else(|| scope.working_dir.clone());
    let agents_md = read_optional_file(&repo_root.join("AGENTS.md"));
    let claude_md = read_optional_file(&repo_root.join("CLAUDE.md"));
    let docs_listing = collect_docs_listing(&repo_root);
    let quality_risks = extract_quality_risks(quality_doc);

    SeedPromptContext {
        primary_gap: profile.agent_readiness.primary_gap.clone(),
        readiness_score: profile.agent_readiness.readiness_score,
        readiness_grade: profile.agent_readiness.readiness_grade.clone(),
        quality_doc: quality_doc.to_string(),
        agents_md,
        claude_md,
        docs_listing,
        quality_risks,
    }
}

fn read_optional_file(path: &std::path::Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn collect_docs_listing(repo_root: &std::path::Path) -> String {
    let docs_root = repo_root.join("docs");
    if !docs_root.is_dir() {
        return String::new();
    }

    let mut files = Vec::new();
    walk_docs(&docs_root, &mut files);
    files.sort_unstable();
    let mut out = String::new();
    for file in files {
        let _ = writeln!(&mut out, "- {file}");
    }
    out
}

fn walk_docs(root: &std::path::Path, files: &mut Vec<String>) {
    walk_docs_with_root(root, root, files)
}

fn walk_docs_with_root(root: &std::path::Path, path: &std::path::Path, files: &mut Vec<String>) {
    append_run_log(
        "debug",
        "seeding.walk_docs",
        json!({
            "root": root.display().to_string(),
        }),
    );
    let mut entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.by_ref().flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_docs_with_root(root, &path, files);
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .ok()
            .and_then(|p| p.to_str())
            .unwrap_or("")
            .trim_start_matches('/');
        files.push(format!("docs/{rel}"));
    }
}

fn extract_quality_risks(quality_doc: &str) -> String {
    let mut rows = Vec::new();
    for line in quality_doc.lines() {
        let line = line.trim();
        if !line.starts_with('|') {
            continue;
        }
        if line.contains("| Domain |") || line.starts_with("| ---") || line.contains("|---") {
            continue;
        }
        let columns: Vec<_> = line
            .split('|')
            .map(|col| col.trim())
            .filter(|col| !col.is_empty())
            .collect();
        if columns.len() != 3 {
            continue;
        }
        let grade = columns[2];
        if !matches!(grade, "A" | "B" | "C" | "D" | "F") {
            continue;
        }
        rows.push(format!("| {} | {} | {} |", columns[0], columns[1], grade));
    }
    rows.join("\n")
}

#[cfg(test)]
mod tests {
    use super::{
        build_seed_prompt, build_seed_prompt_context, build_seed_prompt_v2, collect_docs_listing,
        extract_quality_risks, read_optional_file, walk_docs, SeedPromptContext,
    };
    use crate::triage_discovery::DiscoveryAssessment;
    use crate::types::RuntimeScope;
    use crate::{repo_intelligence, repo_intelligence::RepoIntelligenceProfile};
    use std::fs;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn prompt_builds_multiple_sections() {
        let profile = RepoIntelligenceProfile {
            meta: repo_intelligence::RepoMeta {
                schema_version: 1,
                created_at: "0".to_string(),
                head_sha: "HEAD".to_string(),
                working_dir: "/repo".to_string(),
                repo_root: "/repo".to_string(),
                discovery_used: false,
            },
            detected_agent: repo_intelligence::DetectedAgentProfile {
                primary: "codex".to_string(),
                claude_signals: Vec::new(),
                codex_signals: Vec::new(),
                agents_md_present: false,
                user_confirmed: false,
            },
            discovery: DiscoveryAssessment::unknown(),
            user_validated: repo_intelligence::UserValidated {
                agent_steering_correction: String::new(),
                external_docs_surface: String::new(),
                external_docs_accessible: false,
                guardrails_correction: String::new(),
                validation_command: String::new(),
                coverage_grade_override: String::new(),
                additional_context: String::new(),
                preferred_parallelism: None,
                corrections_made: 0,
                validated_at: "0".to_string(),
            },
            agent_readiness: repo_intelligence::AgentReadiness {
                agent_steering_score: 2,
                knowledge_accessible_score: 2,
                mechanical_guardrails_score: 2,
                local_feedback_loop_score: 2,
                coverage_signal_score: 2,
                readiness_score: 82,
                readiness_grade: "B".to_string(),
                primary_gap: "coverage_signal".to_string(),
            },
        };

        let _scope = RuntimeScope {
            process_cwd: PathBuf::from("/repo"),
            repo_root: Some(PathBuf::from("/repo")),
            working_dir: PathBuf::from("/repo"),
        };
        let context = SeedPromptContext {
            primary_gap: profile.agent_readiness.primary_gap.clone(),
            readiness_score: profile.agent_readiness.readiness_score,
            readiness_grade: profile.agent_readiness.readiness_grade,
            quality_doc: "| Domain | Score | Grade |\n| --- | --- | --- |\n| startup | 40 | C |"
                .to_string(),
            agents_md: String::new(),
            claude_md: String::new(),
            docs_listing: "- docs/index.md\n".to_string(),
            quality_risks: "| startup | 40 | C |\n".to_string(),
        };
        let prompt = super::build_seed_prompt_v2(&context);
        assert!(prompt.contains("Output contract"));
        assert!(prompt.contains("primary_gap"));
        assert!(prompt.contains("Quality risks extracted from report"));
        assert!(prompt.contains("\"state\":\"seeding\""));
    }

    fn sample_profile() -> RepoIntelligenceProfile {
        RepoIntelligenceProfile {
            meta: repo_intelligence::RepoMeta {
                schema_version: 1,
                created_at: "0".to_string(),
                head_sha: "HEAD".to_string(),
                working_dir: "/repo".to_string(),
                repo_root: "/repo".to_string(),
                discovery_used: false,
            },
            detected_agent: repo_intelligence::DetectedAgentProfile {
                primary: "codex".to_string(),
                claude_signals: Vec::new(),
                codex_signals: Vec::new(),
                agents_md_present: false,
                user_confirmed: false,
            },
            discovery: DiscoveryAssessment::unknown(),
            user_validated: repo_intelligence::UserValidated {
                agent_steering_correction: String::new(),
                external_docs_surface: String::new(),
                external_docs_accessible: false,
                guardrails_correction: String::new(),
                validation_command: String::new(),
                coverage_grade_override: String::new(),
                additional_context: String::new(),
                preferred_parallelism: None,
                corrections_made: 0,
                validated_at: "0".to_string(),
            },
            agent_readiness: repo_intelligence::AgentReadiness {
                agent_steering_score: 2,
                knowledge_accessible_score: 2,
                mechanical_guardrails_score: 2,
                local_feedback_loop_score: 2,
                coverage_signal_score: 2,
                readiness_score: 82,
                readiness_grade: "B".to_string(),
                primary_gap: "coverage_signal".to_string(),
            },
        }
    }

    #[test]
    fn read_optional_file_returns_empty_when_missing() {
        let dir = tempdir().expect("tempdir");
        let missing = dir.path().join("does-not-exist.md");
        assert_eq!(read_optional_file(&missing), String::new());
    }

    #[test]
    fn read_optional_file_reads_existing_file() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("notes.md");
        fs::write(&path, "seeding notes").expect("write");
        assert_eq!(read_optional_file(&path), "seeding notes");
    }

    #[test]
    fn walk_docs_collects_nested_markdown_files() {
        let dir = tempdir().expect("tempdir");
        let docs_root = dir.path().join("docs");
        let nested = docs_root.join("nested");
        fs::create_dir_all(&nested).expect("mkdir");
        fs::write(docs_root.join("README.md"), "root").expect("write");
        fs::write(nested.join("inner.md"), "nested").expect("write");
        fs::write(nested.join("ignore.txt"), "skip").expect("write");

        let mut files = Vec::new();
        walk_docs(&docs_root, &mut files);
        assert_eq!(files.len(), 2);
        assert!(files.contains(&"docs/README.md".to_string()));
        assert!(files.contains(&"docs/nested/inner.md".to_string()));
    }

    #[test]
    fn collect_docs_listing_skips_missing_and_non_markdown_files() {
        let dir = tempdir().expect("tempdir");
        let docs_root = dir.path().join("docs");
        let nested = docs_root.join("nested");
        fs::create_dir_all(&nested).expect("mkdir");
        fs::write(docs_root.join("README.md"), "root").expect("write");
        fs::write(nested.join("nested.md"), "nested").expect("write");
        fs::write(nested.join("ignore.txt"), "skip").expect("write");

        let listing = collect_docs_listing(dir.path());
        assert_eq!(
            listing.lines().collect::<Vec<_>>(),
            vec!["- docs/README.md", "- docs/nested/nested.md"]
        );
    }

    #[test]
    fn extract_quality_risks_ignores_unknown_rows() {
        let risks = extract_quality_risks(
            "| Domain | Score | Grade |\n\
             | --- | --- | --- |\n\
             | startup | 40 | C |\n\
             | worker | 88 | A |\n\
             | infra | 12 | Z |\n\
             | docs | 55 |\n",
        );
        assert_eq!(risks, "| startup | 40 | C |\n| worker | 88 | A |");
    }

    #[test]
    fn build_seed_prompt_context_includes_repo_artifacts() {
        let dir = tempdir().expect("tempdir");
        let repo_root = dir.path().to_path_buf();
        fs::create_dir_all(repo_root.join("docs")).expect("mkdir docs");
        fs::write(repo_root.join("AGENTS.md"), "# AGENTS").expect("write");
        fs::write(repo_root.join("CLAUDE.md"), "## CLAUDE").expect("write");
        fs::write(repo_root.join("docs").join("guide.md"), "- guide").expect("write");

        let scope = RuntimeScope {
            process_cwd: repo_root.clone(),
            repo_root: Some(repo_root.clone()),
            working_dir: repo_root,
        };
        let profile = sample_profile();
        let context = build_seed_prompt_context(&profile, "Quality report", &scope);

        assert_eq!(context.primary_gap, "coverage_signal");
        assert_eq!(context.agents_md, "# AGENTS");
        assert_eq!(context.claude_md, "## CLAUDE");
        assert_eq!(context.docs_listing, "- docs/guide.md\n");
        assert!(context.quality_risks.is_empty());

        let prompt = build_seed_prompt_v2(&context);
        assert!(prompt.contains("# AGENTS"));
        assert!(prompt.contains("## CLAUDE"));
    }

    #[test]
    fn build_seed_prompt_uses_quality_markdown_without_repository_files() {
        let profile = sample_profile();
        let prompt = build_seed_prompt(
            &profile,
            "Risk summary",
            &RuntimeScope {
                process_cwd: std::env::current_dir().expect("cwd"),
                repo_root: None,
                working_dir: std::env::current_dir().expect("cwd"),
            },
        );

        assert!(prompt.contains("No AGENTS.md found."));
        assert!(prompt.contains("No CLAUDE.md found."));
        assert!(prompt.contains("No docs directory found or readable."));
        assert!(prompt.contains("Quality doc"));
    }
}
