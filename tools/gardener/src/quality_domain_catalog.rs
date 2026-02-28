use crate::logging::append_run_log;
use serde_json::json;
use std::collections::BTreeSet;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualityDomain {
    pub name: String,
}

pub fn discover_domains(repo_root: &Path) -> Vec<QualityDomain> {
    let src_dir = repo_root.join("src");
    if !src_dir.is_dir() {
        return vec![QualityDomain {
            name: "infrastructure".to_string(),
        }];
    }

    let mut names = BTreeSet::new();
    collect_source_domains(&src_dir, &mut names);
    names.insert("infrastructure".to_string());

    let domains: Vec<_> = names
        .into_iter()
        .map(|name| QualityDomain { name })
        .collect();

    append_run_log(
        "debug",
        "quality.domains.discovered",
        json!({
            "repo_root": repo_root.display().to_string(),
            "domain_count": domains.len(),
        }),
    );

    domains
}

fn collect_source_domains(path: &Path, names: &mut BTreeSet<String>) {
    let mut entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.by_ref().flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_source_domains(&path, names);
            continue;
        }

        let Some(_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let rel = path
            .to_string_lossy()
            .to_ascii_lowercase();
        let domain = match rel.as_str() {
            p if p.contains("/agent/") || p.ends_with("/protocol.rs") || p.ends_with("/output_envelope.rs") => Some("agent-adapters"),
            p if p.contains("/triage") || p.ends_with("/triage.rs") || p.starts_with("/triage") => Some("triage"),
            p if p.contains("/backlog") || p.ends_with("/priority.rs") || p.ends_with("/task_identity.rs") => Some("backlog"),
            p if p.contains("/seeding.rs") || p.contains("/seed_runner.rs") => Some("seeding"),
            p if p.contains("/worker") || p.ends_with("/fsm.rs") => Some("worker-pool"),
            p if p.ends_with("/tui.rs") || p.ends_with("/hotkeys.rs") => Some("tui"),
            p if p.contains("/quality") => Some("quality-grades"),
            p if p.ends_with("/startup.rs") || p.ends_with("/worktree_audit.rs") || p.ends_with("/pr_audit.rs") => Some("startup"),
            p if p.ends_with("/git.rs") || p.ends_with("/gh.rs") || p.ends_with("/worktree.rs") => Some("git-integration"),
            p if p.contains("/prompt") => Some("prompts"),
            p if p.ends_with("/learning_loop.rs") || p.ends_with("/postmerge_analysis.rs") || p.ends_with("/postmortem.rs") => Some("learning"),
            _ => None,
        };
        if let Some(domain) = domain {
            names.insert(domain.to_string());
        }
    }
}
