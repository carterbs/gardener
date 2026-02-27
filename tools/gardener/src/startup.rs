use crate::backlog_store::{BacklogStore, NewTask};
use crate::config::AppConfig;
use crate::errors::GardenerError;
use crate::priority::Priority;
use crate::pr_audit::reconcile_open_prs;
use crate::quality_grades::render_quality_grade_document;
use crate::repo_intelligence::read_profile;
use crate::runtime::{ProcessRequest, ProductionRuntime};
use crate::seeding::seed_backlog_if_needed;
use crate::task_identity::TaskKind;
use crate::triage::profile_path;
use crate::types::RuntimeScope;
use crate::worktree_audit::reconcile_worktrees;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

const REPORT_TTL_SECONDS: u64 = 3600;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupSummary {
    pub quality_path: PathBuf,
    pub quality_written: bool,
    pub stale_worktrees_found: usize,
    pub stale_worktrees_fixed: usize,
    pub pr_collisions_found: usize,
    pub pr_collisions_fixed: usize,
    pub seeded_tasks_upserted: usize,
}

pub fn refresh_quality_report(
    runtime: &ProductionRuntime,
    cfg: &AppConfig,
    scope: &RuntimeScope,
    force: bool,
) -> Result<(PathBuf, bool), GardenerError> {
    let profile_loc = profile_path(scope, cfg);
    let profile = read_profile(runtime.file_system.as_ref(), &profile_loc)?;
    let quality_path = if PathBuf::from(&cfg.quality_report.path).is_absolute() {
        PathBuf::from(&cfg.quality_report.path)
    } else {
        scope.working_dir.join(&cfg.quality_report.path)
    };
    let stamp_path = quality_stamp_path(&quality_path);
    let should_regen = force
        || !runtime.file_system.exists(&quality_path)
        || report_stamp_is_stale(runtime, &stamp_path)?;
    if should_regen {
        let quality_doc = render_quality_grade_document(&profile_loc.display().to_string(), &profile);
        if let Some(parent) = quality_path.parent() {
            runtime.file_system.create_dir_all(parent)?;
        }
        runtime.file_system.write_string(&quality_path, &quality_doc)?;
        let now = runtime
            .clock
            .now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        runtime
            .file_system
            .write_string(&stamp_path, &now.to_string())?;
    }
    Ok((quality_path, should_regen))
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

    let (quality_path, quality_written) = refresh_quality_report(runtime, cfg, scope, false)?;
    let quality_doc = runtime.file_system.read_to_string(&quality_path)?;

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

    let mut seeded_tasks_upserted = 0usize;
    if run_seeding && !cfg.execution.test_mode {
        let seeded = match seed_backlog_if_needed(
            runtime.process_runner.as_ref(),
            scope,
            cfg,
            &profile,
            &quality_doc,
        ) {
            Ok(tasks) => tasks,
            Err(err) => {
                runtime
                    .terminal
                    .write_line(&format!("WARN backlog seeding failed: {err}"))?;
                Vec::new()
            }
        };
        if !seeded.is_empty() {
            let db_path = scope
                .repo_root
                .as_ref()
                .unwrap_or(&scope.working_dir)
                .join(".cache/gardener/backlog.sqlite");
            let store = BacklogStore::open(db_path)?;
            for task in seeded {
                let row = store.upsert_task(NewTask {
                    kind: TaskKind::QualityGap,
                    title: task.title,
                    details: task.details,
                    scope_key: profile.agent_readiness.primary_gap.clone(),
                    priority: Priority::P1,
                    source: "seed_runner_v1".to_string(),
                    related_pr: None,
                    related_branch: None,
                })?;
                if !row.task_id.is_empty() {
                    seeded_tasks_upserted = seeded_tasks_upserted.saturating_add(1);
                }
            }
        } else {
            let db_path = scope
                .repo_root
                .as_ref()
                .unwrap_or(&scope.working_dir)
                .join(".cache/gardener/backlog.sqlite");
            let store = BacklogStore::open(db_path)?;
            let bootstrap = store.upsert_task(NewTask {
                kind: TaskKind::QualityGap,
                title: format!(
                    "Bootstrap backlog for {}",
                    profile.agent_readiness.primary_gap
                ),
                details: "Seeding returned no tasks; investigate current repo gaps and create follow-up tasks."
                    .to_string(),
                scope_key: profile.agent_readiness.primary_gap.clone(),
                priority: Priority::P1,
                source: "seed_runner_v1_fallback".to_string(),
                related_pr: None,
                related_branch: None,
            })?;
            if !bootstrap.task_id.is_empty() {
                seeded_tasks_upserted = seeded_tasks_upserted.saturating_add(1);
            }
        }
    }

    if !runtime.terminal.stdin_is_tty() {
        runtime.terminal.write_line(&format!(
            "startup health summary: quality={} stale_worktrees={}/{} pr_collisions={}/{} seeded_tasks={}",
            quality_path.display(),
            wt.stale_found,
            wt.stale_fixed,
            prs.collisions_found,
            prs.collisions_fixed,
            seeded_tasks_upserted
        ))?;
    }

    Ok(StartupSummary {
        quality_path,
        quality_written,
        stale_worktrees_found: wt.stale_found,
        stale_worktrees_fixed: wt.stale_fixed,
        pr_collisions_found: prs.collisions_found,
        pr_collisions_fixed: prs.collisions_fixed,
        seeded_tasks_upserted,
    })
}

fn quality_stamp_path(quality_path: &std::path::Path) -> PathBuf {
    PathBuf::from(format!("{}.stamp", quality_path.display()))
}

fn report_stamp_is_stale(
    runtime: &ProductionRuntime,
    stamp_path: &std::path::Path,
) -> Result<bool, GardenerError> {
    if !runtime.file_system.exists(stamp_path) {
        return Ok(true);
    }
    let raw = runtime.file_system.read_to_string(stamp_path)?;
    let stamp = raw.trim().parse::<u64>().unwrap_or(0);
    let now = runtime
        .clock
        .now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Ok(now.saturating_sub(stamp) > REPORT_TTL_SECONDS)
}
