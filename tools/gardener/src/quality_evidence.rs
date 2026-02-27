use crate::quality_domain_catalog::QualityDomain;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainEvidence {
    pub domain: String,
    pub tested_files: Vec<String>,
    pub untested_files: Vec<String>,
}

pub fn collect_evidence(domains: &[QualityDomain], repo_root: &Path) -> Vec<DomainEvidence> {
    domains
        .iter()
        .map(|d| {
            let (tested, untested) = scan_source_files(repo_root);
            DomainEvidence {
                domain: d.name.clone(),
                tested_files: tested,
                untested_files: untested,
            }
        })
        .collect()
}

fn scan_source_files(repo_root: &Path) -> (Vec<String>, Vec<String>) {
    let src_dir = repo_root.join("src");
    let mut tested = Vec::new();
    let mut untested = Vec::new();

    if !src_dir.is_dir() {
        return (tested, untested);
    }

    let entries = match std::fs::read_dir(&src_dir) {
        Ok(entries) => entries,
        Err(_) => return (tested, untested),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };

        if !name.ends_with(".rs") {
            continue;
        }

        let relative = format!("src/{name}");
        if file_contains_tests(&path) {
            tested.push(relative);
        } else {
            untested.push(relative);
        }
    }

    tested.sort();
    untested.sort();
    (tested, untested)
}

fn file_contains_tests(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .map(|contents| contents.contains("#[test]") || contents.contains("#[cfg(test)]"))
        .unwrap_or(false)
}
