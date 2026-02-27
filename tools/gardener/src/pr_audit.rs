use crate::gh::upgrade_unmerged_collision_priority;
use crate::logging::append_run_log;
use crate::priority::Priority;
use crate::runtime::{ProcessRequest, ProcessRunner, ProductionProcessRunner};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PrAuditSummary {
    pub collisions_found: usize,
    pub collisions_fixed: usize,
}

pub fn reconcile_open_prs() -> PrAuditSummary {
    append_run_log("info", "pr_audit.started", json!({}));
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => {
            append_run_log(
                "error",
                "pr_audit.cwd_failed",
                json!({ "error": e.to_string() }),
            );
            return PrAuditSummary::default();
        }
    };
    append_run_log(
        "info",
        "pr_audit.listing_prs",
        json!({
            "cwd": cwd.display().to_string()
        }),
    );
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
        cwd: Some(cwd.clone()),
    }) {
        Ok(out) if out.exit_code == 0 => out,
        Ok(out) => {
            append_run_log(
                "error",
                "pr_audit.list_failed",
                json!({
                    "cwd": cwd.display().to_string(),
                    "exit_code": out.exit_code,
                    "stderr": out.stderr
                }),
            );
            return PrAuditSummary::default();
        }
        Err(e) => {
            append_run_log(
                "error",
                "pr_audit.list_error",
                json!({
                    "cwd": cwd.display().to_string(),
                    "error": e.to_string()
                }),
            );
            return PrAuditSummary::default();
        }
    };
    let prs: Vec<OpenPr> = match serde_json::from_str(&out.stdout) {
        Ok(prs) => prs,
        Err(e) => {
            append_run_log(
                "error",
                "pr_audit.parse_failed",
                json!({
                    "cwd": cwd.display().to_string(),
                    "error": e.to_string()
                }),
            );
            return PrAuditSummary::default();
        }
    };
    let total_prs = prs.len();
    append_run_log(
        "info",
        "pr_audit.prs_listed",
        json!({
            "cwd": cwd.display().to_string(),
            "total_open_prs": total_prs
        }),
    );
    let mut by_branch: HashMap<String, usize> = HashMap::new();
    for pr in prs {
        *by_branch.entry(pr.head_ref_name).or_insert(0) += 1;
    }
    let collisions_found = by_branch
        .values()
        .filter(|count| **count > 1)
        .map(|count| count - 1)
        .sum::<usize>();
    if collisions_found > 0 {
        let colliding_branches: Vec<&str> = by_branch
            .iter()
            .filter(|(_, count)| **count > 1)
            .map(|(branch, _)| branch.as_str())
            .collect();
        append_run_log(
            "warn",
            "pr_audit.collisions_found",
            json!({
                "cwd": cwd.display().to_string(),
                "collisions_found": collisions_found,
                "colliding_branches": colliding_branches
            }),
        );
    } else {
        append_run_log(
            "debug",
            "pr_audit.no_collisions",
            json!({
                "cwd": cwd.display().to_string(),
                "total_open_prs": total_prs
            }),
        );
    }
    let collisions_fixed = if collisions_found > 0 {
        let upgraded = upgrade_unmerged_collision_priority(Priority::P2);
        if upgraded != Priority::P2 {
            append_run_log(
                "info",
                "pr_audit.priority_upgraded",
                json!({
                    "cwd": cwd.display().to_string(),
                    "collisions_found": collisions_found,
                    "upgraded_to": format!("{:?}", upgraded)
                }),
            );
            collisions_found
        } else {
            0
        }
    } else {
        0
    };
    append_run_log(
        "info",
        "pr_audit.completed",
        json!({
            "cwd": cwd.display().to_string(),
            "total_open_prs": total_prs,
            "collisions_found": collisions_found,
            "collisions_fixed": collisions_fixed
        }),
    );
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
