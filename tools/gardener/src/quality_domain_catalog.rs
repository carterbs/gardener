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

#[cfg(test)]
mod tests {
    use super::{collect_source_domains, discover_domains, QualityDomain};
    use std::fs;

    #[test]
    fn discover_domains_defaults_to_infrastructure_if_no_src_dir() {
        let temp = tempfile::tempdir().expect("tempdir");
        let domains = discover_domains(temp.path());
        assert_eq!(
            domains,
            vec![QualityDomain {
                name: "infrastructure".to_string()
            }]
        );
    }

    #[test]
    fn collect_source_domains_maps_known_files_to_domain_labels() {
        let mut names = std::collections::BTreeSet::new();
        let missing = tempfile::tempdir().expect("tempdir");
        collect_source_domains(missing.path(), &mut names);
        assert_eq!(names.len(), 0);

        let temp = tempfile::tempdir().expect("tempdir");
        let src = temp.path().join("src");
        fs::create_dir_all(src.join("agent"))
            .expect("create source directory");
        fs::write(src.join("agent").join("protocol.rs"), "let x = 1;")
            .expect("write source");
        fs::write(src.join("tui.rs"), "fn main() {}").expect("write source");
        fs::write(src.join("triage.rs"), "fn main() {}").expect("write source");
        fs::write(src.join("bad.txt"), "skip").expect("write source");

        let mut names = std::collections::BTreeSet::new();
        collect_source_domains(&src, &mut names);
        let domains: Vec<_> = names.into_iter().collect();
        assert_eq!(
            domains,
            vec![
                "agent-adapters".to_string(),
                "triage".to_string(),
                "tui".to_string()
            ]
        );
    }

    #[test]
    fn discover_domains_includes_infrastructure_plus_detected_domains() {
        let temp = tempfile::tempdir().expect("tempdir");
        let src = temp.path().join("src");
        fs::create_dir_all(src.join("worker").join("nested")).expect("create source dir");
        fs::create_dir_all(src.join("quality")).expect("create quality dir");
        fs::write(src.join("worker").join("worker_pool.rs"), "fn one() {}").expect("write file");
        fs::write(src.join("quality").join("grades.rs"), "fn two() {}").expect("write file");

        let domains = discover_domains(temp.path());
        let names: Vec<_> = domains.into_iter().map(|d| d.name).collect();
        assert_eq!(
            names,
            vec![
                "infrastructure".to_string(),
                "quality-grades".to_string(),
                "worker-pool".to_string()
            ]
        );
    }
}
