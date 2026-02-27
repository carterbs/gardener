use crate::gh::upgrade_unmerged_collision_priority;
use crate::priority::Priority;
use crate::runtime::{ProcessRequest, ProcessRunner, ProductionProcessRunner};
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PrAuditSummary {
    pub collisions_found: usize,
    pub collisions_fixed: usize,
}

pub fn reconcile_open_prs() -> PrAuditSummary {
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(_) => return PrAuditSummary::default(),
    };
    let runner = ProductionProcessRunner::new();
    let out = match runner.run(ProcessRequest {
        program: "gh".to_string(),
        args: vec![
            "pr".to_string(),
            "list".to_string(),
            "--state".to_string(),
            "open".to_string(),
            "--limit".to_string(),
            "200".to_string(),
            "--json".to_string(),
            "number,headRefName".to_string(),
        ],
        cwd: Some(cwd),
    }) {
        Ok(out) if out.exit_code == 0 => out,
        _ => return PrAuditSummary::default(),
    };
    let prs: Vec<OpenPr> = match serde_json::from_str(&out.stdout) {
        Ok(prs) => prs,
        Err(_) => return PrAuditSummary::default(),
    };
    let mut by_branch: HashMap<String, usize> = HashMap::new();
    for pr in prs {
        *by_branch.entry(pr.head_ref_name).or_insert(0) += 1;
    }
    let collisions_found = by_branch
        .values()
        .filter(|count| **count > 1)
        .map(|count| count - 1)
        .sum::<usize>();
    let collisions_fixed = if collisions_found > 0 {
        let upgraded = upgrade_unmerged_collision_priority(Priority::P2);
        if upgraded != Priority::P2 {
            collisions_found
        } else {
            0
        }
    } else {
        0
    };
    PrAuditSummary {
        collisions_found,
        collisions_fixed,
    }
}

#[derive(Debug, Clone, Deserialize)]
struct OpenPr {
    #[serde(rename = "headRefName")]
    head_ref_name: String,
}
