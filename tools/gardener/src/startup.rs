use crate::config::AppConfig;
use crate::errors::GardenerError;
use crate::pr_audit::reconcile_open_prs;
use crate::quality_grades::render_quality_grade_document;
use crate::repo_intelligence::read_profile;
use crate::runtime::{ProcessRequest, ProductionRuntime};
use crate::seeding::seed_backlog_if_needed;
use crate::triage::profile_path;
use crate::types::RuntimeScope;
use crate::worktree_audit::reconcile_worktrees;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupSummary {
    pub quality_path: PathBuf,
    pub quality_written: bool,
    pub stale_worktrees_found: usize,
    pub stale_worktrees_fixed: usize,
    pub pr_collisions_found: usize,
    pub pr_collisions_fixed: usize,
}

pub fn run_startup_audits(
    runtime: &ProductionRuntime,
    cfg: &mut AppConfig,
    scope: &RuntimeScope,
    run_seeding: bool,
) -> Result<StartupSummary, GardenerError> {
    let profile_loc = profile_path(scope, cfg);
    if !runtime.file_system.exists(&profile_loc) {
        return Err(GardenerError::Cli(
            "No repo intelligence profile found. Run `brad-gardener --triage-only` in a terminal to complete setup."
                .to_string(),
        ));
    }
    let profile = read_profile(runtime.file_system.as_ref(), &profile_loc)?;
    if (cfg.startup.validation_command.is_none()
        || cfg
            .startup
            .validation_command
            .as_ref()
            .is_some_and(|v| v.trim().is_empty()))
        && !profile.user_validated.validation_command.trim().is_empty()
    {
        cfg.startup.validation_command = Some(profile.user_validated.validation_command.clone());
    }

    let quality_path = if PathBuf::from(&cfg.quality_report.path).is_absolute() {
        PathBuf::from(&cfg.quality_report.path)
    } else {
        scope.working_dir.join(&cfg.quality_report.path)
    };

    let quality_doc = render_quality_grade_document(&profile_loc.display().to_string(), &profile);
    if let Some(parent) = quality_path.parent() {
        runtime.file_system.create_dir_all(parent)?;
    }
    runtime
        .file_system
        .write_string(&quality_path, &quality_doc)?;

    let wt = reconcile_worktrees();
    let prs = reconcile_open_prs();

    if cfg.startup.validate_on_boot {
        let command = cfg
            .startup
            .validation_command
            .clone()
            .unwrap_or_else(|| cfg.validation.command.clone());
        let out = runtime.process_runner.run(ProcessRequest {
            program: "sh".to_string(),
            args: vec!["-lc".to_string(), command],
            cwd: Some(scope.working_dir.clone()),
        })?;
        if out.exit_code != 0 {
            runtime
                .terminal
                .write_line("WARN startup validation failed; enqueue P0 recovery task")?;
        }
    }

    if run_seeding && !cfg.execution.test_mode {
        let _ = seed_backlog_if_needed(
            runtime.process_runner.as_ref(),
            scope,
            cfg,
            &profile,
            &quality_doc,
        );
    }

    runtime.terminal.write_line(&format!(
        "startup health summary: quality={} stale_worktrees={}/{} pr_collisions={}/{}",
        quality_path.display(),
        wt.stale_found,
        wt.stale_fixed,
        prs.collisions_found,
        prs.collisions_fixed
    ))?;

    Ok(StartupSummary {
        quality_path,
        quality_written: true,
        stale_worktrees_found: wt.stale_found,
        stale_worktrees_fixed: wt.stale_fixed,
        pr_collisions_found: prs.collisions_found,
        pr_collisions_fixed: prs.collisions_fixed,
    })
}
