use crate::agent::factory::AdapterFactory;
use crate::backlog_store::{BacklogStore, NewTask};
use crate::config::AppConfig;
use crate::errors::GardenerError;
use crate::logging::append_run_log;
use crate::pr_audit::reconcile_open_prs;
use crate::priority::Priority;
use crate::protocol::{summarize_agent_event, AgentEvent};
use crate::quality_assessment_runner::{QualityAssessmentConfig, QualityProgressEvent};
use crate::quality_grade_compute::GradeReport;
use crate::quality_pipeline::run_quality_pipeline;
use crate::repo_intelligence::read_profile;
use crate::runtime::{Clock, FileSystem, ProcessRequest, ProductionRuntime};
use crate::seed_runner::SeedTask;
use crate::seeding::{
    recommend_seed_tasks_with_events, refine_seed_tasks_with_events,
    seed_backlog_if_needed_with_events,
};
use crate::task_identity::TaskKind;
use crate::triage::profile_path;
use crate::tui::now_hhmmss;
use crate::types::RuntimeScope;
use crate::worktree_audit::reconcile_worktrees;
use serde_json::json;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;
use std::time::UNIX_EPOCH;

const REPORT_TTL_SECONDS: u64 = 3600;
const RUNTIME_BACKLOG_DB_ENV: &str = "GARDENER_RUNTIME_DB_PATH";
const LEGACY_BACKLOG_DB_ENV: &str = "GARDENER_DB_PATH";
const DEFAULT_BACKLOG_DB_PATH: &str = ".gardener/backlog.sqlite";

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

fn runtime_backlog_db_path(scope: &RuntimeScope) -> PathBuf {
    if let Ok(home) = env::var("HOME") {
        return PathBuf::from(home).join(DEFAULT_BACKLOG_DB_PATH);
    }
    scope.working_dir.join(DEFAULT_BACKLOG_DB_PATH)
}

pub fn backlog_db_path(_cfg: &crate::config::AppConfig, scope: &RuntimeScope) -> PathBuf {
    if let Ok(path) = env::var(RUNTIME_BACKLOG_DB_ENV) {
        return PathBuf::from(path);
    }
    if let Ok(path) = env::var(LEGACY_BACKLOG_DB_ENV) {
        return PathBuf::from(path);
    }
    runtime_backlog_db_path(scope)
}

pub fn refresh_quality_report(
    runtime: &ProductionRuntime,
    cfg: &AppConfig,
    scope: &RuntimeScope,
    force: bool,
) -> Result<(PathBuf, bool, GradeReport), GardenerError> {
    let quality_path = quality_report_path(cfg, scope);
    let stamp_path = quality_stamp_path(&quality_path);
    let cache_path = grade_report_cache_path(&quality_path);
    let should_regen = force
        || !runtime.file_system.exists(&quality_path)
        || !runtime.file_system.exists(&cache_path)
        || report_stamp_is_stale(runtime, cfg, &stamp_path)?;

    append_run_log(
        "debug",
        "startup.quality_report.check",
        json!({
            "quality_path": quality_path.display().to_string(),
            "force": force,
            "should_regen": should_regen,
        }),
    );

    if should_regen {
        let repo_root = scope.repo_root.as_ref().unwrap_or(&scope.working_dir);

        let (quality_doc, grade_report) = if runtime.terminal.stdin_is_tty() {
            run_quality_with_heartbeat(runtime, cfg, repo_root)?
        } else {
            try_pipeline_quality_report(cfg, repo_root, runtime, None)?
        };

        append_run_log(
            "info",
            "startup.quality_report.pipeline_succeeded",
            json!({ "quality_path": quality_path.display().to_string() }),
        );

        if let Some(parent) = quality_path.parent() {
            runtime.file_system.create_dir_all(parent)?;
        }
        runtime
            .file_system
            .write_string(&quality_path, &quality_doc)?;
        let now = runtime
            .clock
            .now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        runtime
            .file_system
            .write_string(&stamp_path, &now.to_string())?;
        // Persist the GradeReport as a sidecar JSON cache for future TTL-hit runs.
        let grade_cache_json = serde_json::to_string(&grade_report).map_err(|e| {
            GardenerError::Process(format!("failed to serialize grade report: {e}"))
        })?;
        runtime
            .file_system
            .write_string(&cache_path, &grade_cache_json)?;
        append_run_log(
            "info",
            "startup.quality_report.refreshed",
            json!({
                "quality_path": quality_path.display().to_string(),
                "stamp_ts": now,
            }),
        );
        return Ok((quality_path, true, grade_report));
    }

    // TTL hit: load the cached GradeReport from the sidecar JSON file.
    let cache_path = grade_report_cache_path(&quality_path);
    let grade_report = runtime
        .file_system
        .read_to_string(&cache_path)
        .ok()
        .and_then(|json| serde_json::from_str::<GradeReport>(&json).ok())
        .ok_or_else(|| {
            GardenerError::Process(
                "quality report exists but grade-report cache is missing or corrupt; re-run with --force to regenerate".to_string(),
            )
        })?;
    Ok((quality_path, false, grade_report))
}

/// Attempt to generate a quality report using the new pipeline.
fn try_pipeline_quality_report(
    cfg: &AppConfig,
    repo_root: &Path,
    runtime: &ProductionRuntime,
    on_progress: Option<&(dyn Fn(QualityProgressEvent) + Send + Sync)>,
) -> Result<(String, GradeReport), GardenerError> {
    let backend = cfg.quality.backend.unwrap_or(cfg.seeding.backend);
    let model = cfg
        .quality
        .model
        .clone()
        .unwrap_or_else(|| cfg.seeding.model.clone());
    let assessment_config = QualityAssessmentConfig {
        backend,
        model,
        max_turns: cfg.quality.max_turns,
        ..QualityAssessmentConfig::default()
    };

    let factory = AdapterFactory::with_defaults();
    let (doc, report) = run_quality_pipeline(
        repo_root,
        Some(&factory),
        runtime.process_runner.as_ref(),
        None, // no store — backlog emission is handled separately during seeding
        &assessment_config,
        on_progress,
    )?;

    Ok((doc, report))
}

pub fn quality_report_path(cfg: &AppConfig, scope: &RuntimeScope) -> PathBuf {
    if PathBuf::from(&cfg.quality_report.path).is_absolute() {
        PathBuf::from(&cfg.quality_report.path)
    } else {
        scope.working_dir.join(&cfg.quality_report.path)
    }
}

pub fn ensure_quality_report_fresh_for_validation(
    runtime: &ProductionRuntime,
    cfg: &AppConfig,
    scope: &RuntimeScope,
) -> Result<(), GardenerError> {
    ensure_quality_report_fresh_for_validation_with_context(
        runtime.file_system.as_ref(),
        runtime.clock.as_ref(),
        cfg,
        scope,
    )
}

pub(crate) fn ensure_quality_report_fresh_for_validation_with_context(
    file_system: &dyn FileSystem,
    clock: &dyn Clock,
    cfg: &AppConfig,
    scope: &RuntimeScope,
) -> Result<(), GardenerError> {
    let quality_path = quality_report_path(cfg, scope);
    let stamp_path = quality_stamp_path(&quality_path);

    if !file_system.exists(&quality_path) {
        return Err(GardenerError::Cli(
            "quality-grade report missing; run startup audits or `--quality-grades-only` before validation".to_string(),
        ));
    }

    if !file_system.exists(&stamp_path)
        || report_stamp_is_stale_with_context(
            file_system,
            clock,
            cfg,
            &stamp_path,
        )?
    {
        return Err(GardenerError::Cli(
            "quality-grade report is stale; regenerate the quality report before validation"
                .to_string(),
        ));
    }

    append_run_log(
        "debug",
        "startup.quality_report.validation_guard_passed",
        json!({
            "quality_path": quality_path.display().to_string(),
            "stamp_path": stamp_path.display().to_string(),
        }),
    );
    Ok(())
}

pub fn run_startup_audits(
    runtime: &ProductionRuntime,
    cfg: &mut AppConfig,
    scope: &RuntimeScope,
    run_seeding: bool,
    force_seed_backlog: bool,
    seed_dry_run: bool,
) -> Result<StartupSummary, GardenerError> {
    run_startup_audits_with_progress(
        runtime,
        cfg,
        scope,
        run_seeding,
        force_seed_backlog,
        seed_dry_run,
        |_detail| Ok(()),
    )
}

pub fn run_startup_audits_with_progress<F>(
    runtime: &ProductionRuntime,
    cfg: &mut AppConfig,
    scope: &RuntimeScope,
    run_seeding: bool,
    force_seed_backlog: bool,
    seed_dry_run: bool,
    mut progress: F,
) -> Result<StartupSummary, GardenerError>
where
    F: FnMut(&str) -> Result<(), GardenerError>,
{
    let profile_loc = profile_path(scope, cfg);
    append_run_log(
        "info",
        "startup.audits.started",
        json!({
            "run_seeding": run_seeding,
            "force_seed_backlog": force_seed_backlog,
            "seed_dry_run": seed_dry_run,
            "profile_loc": profile_loc.display().to_string(),
            "working_dir": scope.working_dir.display().to_string(),
        }),
    );
    if !runtime.file_system.exists(&profile_loc) {
        append_run_log(
            "error",
            "startup.profile.missing",
            json!({ "profile_loc": profile_loc.display().to_string() }),
        );
        return Err(GardenerError::Cli(
            "No repo intelligence profile found. Run `cargo run -p gardener --bin gardener -- --triage-only` in a terminal to complete setup."
                .to_string(),
        ));
    }

    // Backup the database before any store opens.
    let db_path = backlog_db_path(cfg, scope);
    append_run_log(
        "info",
        "startup.backlog_db.resolved",
        json!({
            "path": db_path.display().to_string(),
            "path_state": crate::backlog_store::backlog_path_state(&db_path),
        }),
    );
    if let Err(e) = backup_db_if_exists(&db_path) {
        append_run_log(
            "error",
            "startup.backup.failed",
            json!({
                "path": db_path.display().to_string(),
                "error": e.to_string(),
            }),
        );
    }

    // Open the store at most once, only when needed.
    let needs_store = cfg.startup.validate_on_boot || (run_seeding && !cfg.execution.test_mode);
    let store = if needs_store {
        Some(BacklogStore::open(&db_path)?)
    } else {
        None
    };

    let profile = read_profile(runtime.file_system.as_ref(), &profile_loc)?;
    append_run_log(
        "debug",
        "startup.profile.loaded",
        json!({
            "profile_loc": profile_loc.display().to_string(),
            "primary_gap": profile.agent_readiness.primary_gap,
            "readiness_score": profile.agent_readiness.readiness_score,
        }),
    );
    if (cfg.startup.validation_command.is_none()
        || cfg
            .startup
            .validation_command
            .as_ref()
            .is_some_and(|v| v.trim().is_empty()))
        && !profile.user_validated.validation_command.trim().is_empty()
    {
        append_run_log(
            "info",
            "startup.validation_command.inherited",
            json!({ "command": profile.user_validated.validation_command }),
        );
        cfg.startup.validation_command = Some(profile.user_validated.validation_command.clone());
    }

    let (quality_path, quality_written, grade_report) =
        refresh_quality_report(runtime, cfg, scope, false)?;
    let quality_doc = runtime.file_system.read_to_string(&quality_path)?;

    let wt = reconcile_worktrees();
    append_run_log(
        "info",
        "startup.worktrees.reconciled",
        json!({
            "stale_found": wt.stale_found,
            "stale_fixed": wt.stale_fixed,
        }),
    );
    let prs = reconcile_open_prs();
    append_run_log(
        "info",
        "startup.prs.reconciled",
        json!({
            "collisions_found": prs.collisions_found,
            "collisions_fixed": prs.collisions_fixed,
        }),
    );

    if cfg.startup.validate_on_boot {
        let command = cfg
            .startup
            .validation_command
            .clone()
            .unwrap_or_else(|| cfg.validation.command.clone());
        append_run_log(
            "info",
            "startup.validation.running",
            json!({ "command": command }),
        );
        let out = runtime.process_runner.run(ProcessRequest {
            program: "sh".to_string(),
            args: vec!["-lc".to_string(), command.clone()],
            cwd: Some(scope.working_dir.clone()),
        })?;
        if out.exit_code != 0 {
            append_run_log(
                "warn",
                "startup.validation.failed",
                json!({
                    "command": command,
                    "exit_code": out.exit_code,
                }),
            );
            runtime
                .terminal
                .write_line("WARN startup validation failed; enqueueing P0 recovery task")?;
            // Safety: store is Some because validate_on_boot implies needs_store.
            let store = store
                .as_ref()
                .ok_or_else(|| GardenerError::Database("store not initialized".to_string()))?;
            store.upsert_task(NewTask {
                kind: TaskKind::Maintenance,
                title: "Recovery: startup validation failed".to_string(),
                details: format!("Validation command exited with code {}", out.exit_code),
                scope_key: "startup".to_string(),
                rationale:
                    "Startup validation failed and requires manual follow-up before workers can run safely."
                        .to_string(),
                priority: Priority::P0,
                source: "validate_on_boot".to_string(),
                related_pr: None,
                related_branch: None,
            })?;
        } else {
            append_run_log(
                "info",
                "startup.validation.passed",
                json!({ "command": command }),
            );
        }
    }

    let mut seeded_tasks_upserted = 0usize;
    // Safety: store is Some when run_seeding && !test_mode (implies needs_store).
    let existing_active_backlog_count = if run_seeding && !cfg.execution.test_mode {
        store
            .as_ref()
            .ok_or_else(|| GardenerError::Database("store not initialized".to_string()))?
            .count_active_tasks()?
    } else {
        0
    };
    let will_seed = should_seed_backlog(
        run_seeding,
        cfg.execution.test_mode,
        existing_active_backlog_count,
        force_seed_backlog,
    );
    append_run_log(
        "info",
        "startup.seeding_gate.checked",
        json!({
            "run_seeding": run_seeding,
            "test_mode": cfg.execution.test_mode,
            "existing_active_count": existing_active_backlog_count,
            "force_seed_backlog": force_seed_backlog,
            "seed_dry_run": seed_dry_run,
            "will_seed": will_seed,
        }),
    );
    if should_seed_backlog(
        run_seeding,
        cfg.execution.test_mode,
        existing_active_backlog_count,
        force_seed_backlog,
    ) {
        // Safety: store is Some because should_seed_backlog requires !test_mode && run_seeding.
        let store = store
            .as_ref()
            .ok_or_else(|| GardenerError::Database("store not initialized".to_string()))?;
        append_run_log(
            "info",
            "startup.seeding.started",
            json!({
                "backend": format!("{:?}", cfg.seeding.backend),
                "model": cfg.seeding.model,
                "primary_gap": profile.agent_readiness.primary_gap,
                "existing_backlog_count": existing_active_backlog_count,
            }),
        );
        progress("Preparing backlog seeding context from repo profile and quality grades")?;
        if !runtime.terminal.stdin_is_tty() {
            runtime.terminal.write_line(
                "startup backlog seeding: preparing context from repo profile + quality report",
            )?;
        }
        progress(&format!(
            "Launching {:?} seeding agent ({})",
            cfg.seeding.backend, cfg.seeding.model
        ))?;
        if !runtime.terminal.stdin_is_tty() {
            runtime.terminal.write_line(&format!(
                "startup backlog seeding: launching backend={:?} model={}",
                cfg.seeding.backend, cfg.seeding.model
            ))?;
        }
        let backlog_snapshot = summarize_active_backlog(store)?;
        let rejected_tasks_fmt = format_rejected_seeds(store);
        if seed_dry_run {
            append_run_log(
                "info",
                "startup.seeding.dry_run.started",
                json!({
                    "backend": format!("{:?}", cfg.seeding.backend),
                    "model": cfg.seeding.model,
                }),
            );
            let recommendations = run_seed_recommendations_with_heartbeat(
                runtime,
                scope,
                cfg,
                &profile,
                &quality_doc,
                &backlog_snapshot,
                &rejected_tasks_fmt,
                &mut progress,
                Some(&grade_report),
            )?;
            print_seed_recommendations(runtime, &recommendations)?;
            progress(&format!(
                "Backlog seeding dry-run complete; recommended {} task(s)",
                recommendations.len()
            ))?;
            append_run_log(
                "info",
                "startup.seeding.dry_run.completed",
                json!({
                    "recommended_tasks": recommendations.len(),
                }),
            );
        } else {
            if let Err(err) = run_seed_with_heartbeat(
                runtime,
                scope,
                cfg,
                &profile,
                &quality_doc,
                &backlog_snapshot,
                &rejected_tasks_fmt,
                &mut progress,
                Some(&grade_report),
            ) {
                append_run_log(
                    "warn",
                    "startup.seeding.agent_failed",
                    json!({
                        "error": err.to_string(),
                    }),
                );
                progress(&format!("Seeding agent failed ({err})"))?;
                runtime
                    .terminal
                    .write_line(&format!("WARN backlog seeding failed: {err}"))?;
            }
            let post_seed_active_count = store.count_active_tasks()?;
            let agent_seeded = post_seed_active_count.saturating_sub(existing_active_backlog_count);
            if agent_seeded > 0 {
                append_run_log(
                    "info",
                    "startup.seeding.direct_persisted",
                    json!({
                        "task_count": agent_seeded,
                        "source": "seed_runner_v2_direct",
                        "existing_count": existing_active_backlog_count,
                        "post_count": post_seed_active_count,
                    }),
                );
                progress(&format!(
                    "Seeding agent inserted {} backlog task(s) directly",
                    agent_seeded
                ))?;
                seeded_tasks_upserted = seeded_tasks_upserted.saturating_add(agent_seeded);
            } else {
                append_run_log(
                    "info",
                    "startup.seeding.agent_seeded_zero",
                    json!({
                        "existing_count": existing_active_backlog_count,
                        "post_count": post_seed_active_count,
                    }),
                );
                progress("Agent seeded 0 tasks; skipping fallback")?;
            }
        }
        append_run_log(
            "info",
            "startup.seeding.completed",
            json!({ "upserted_tasks": seeded_tasks_upserted }),
        );
        progress(&format!(
            "Backlog seeding complete; upserted {} task(s)",
            seeded_tasks_upserted
        ))?;
        if !runtime.terminal.stdin_is_tty() {
            runtime.terminal.write_line(&format!(
                "startup backlog seeding: complete, upserted_tasks={seeded_tasks_upserted}"
            ))?;
        }
    } else if run_seeding && !cfg.execution.test_mode {
        append_run_log(
            "info",
            "startup.seeding.skipped_existing_backlog",
            json!({ "existing_backlog_count": existing_active_backlog_count }),
        );
        progress(&format!(
            "Skipping backlog seeding; backlog already has {existing_active_backlog_count} task(s)"
        ))?;
        if !runtime.terminal.stdin_is_tty() {
            runtime.terminal.write_line(&format!(
                "startup backlog seeding: skipped, existing_backlog_count={existing_active_backlog_count}"
            ))?;
        }
    }

    append_run_log(
        "info",
        "startup.audits.completed",
        json!({
            "quality_path": quality_path.display().to_string(),
            "quality_written": quality_written,
            "stale_worktrees_found": wt.stale_found,
            "stale_worktrees_fixed": wt.stale_fixed,
            "pr_collisions_found": prs.collisions_found,
            "pr_collisions_fixed": prs.collisions_fixed,
            "seeded_tasks_upserted": seeded_tasks_upserted,
        }),
    );

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

pub fn backup_db_if_exists(path: &Path) -> Result<Option<PathBuf>, GardenerError> {
    if !path.exists() {
        append_run_log(
            "debug",
            "startup.backup.skipped_missing",
            json!({ "path": path.display().to_string() }),
        );
        return Ok(None);
    }
    let meta = std::fs::metadata(path).map_err(|e| GardenerError::Database(e.to_string()))?;
    if meta.len() == 0 {
        append_run_log(
            "error",
            "startup.backup.skipped_zero_byte",
            json!({
                "path": path.display().to_string(),
                "size_bytes": meta.len(),
            }),
        );
        return Ok(None);
    }

    let bak_path = path.with_extension("sqlite.bak");
    std::fs::copy(path, &bak_path).map_err(|e| GardenerError::Database(e.to_string()))?;

    // Also copy WAL and SHM sidecar files if they exist.
    for ext in &["sqlite-wal", "sqlite-shm"] {
        let sidecar = path.with_extension(ext);
        if sidecar.exists() {
            let sidecar_bak = bak_path.with_extension(format!(
                "bak-{}",
                ext.strip_prefix("sqlite-").unwrap_or(ext)
            ));
            match std::fs::copy(&sidecar, &sidecar_bak) {
                Ok(bytes) => append_run_log(
                    "debug",
                    "startup.backup.sidecar_copied",
                    json!({
                        "source": sidecar.display().to_string(),
                        "backup": sidecar_bak.display().to_string(),
                        "size_bytes": bytes,
                    }),
                ),
                Err(error) => append_run_log(
                    "warn",
                    "startup.backup.sidecar_copy_failed",
                    json!({
                        "source": sidecar.display().to_string(),
                        "backup": sidecar_bak.display().to_string(),
                        "error": error.to_string(),
                    }),
                ),
            };
        }
    }

    append_run_log(
        "info",
        "startup.backup.created",
        json!({
            "source": path.display().to_string(),
            "backup": bak_path.display().to_string(),
            "size_bytes": meta.len(),
        }),
    );

    Ok(Some(bak_path))
}

fn should_seed_backlog(
    run_seeding: bool,
    test_mode: bool,
    existing_backlog_count: usize,
    force_seed_backlog: bool,
) -> bool {
    run_seeding && !test_mode && (existing_backlog_count == 0 || force_seed_backlog)
}

/// Run the quality pipeline in a background thread with heartbeat TUI updates.
fn run_quality_with_heartbeat(
    runtime: &ProductionRuntime,
    cfg: &AppConfig,
    repo_root: &Path,
) -> Result<(String, GradeReport), GardenerError> {
    append_run_log(
        "debug",
        "startup.quality_report.heartbeat.started",
        json!({
            "repo_root": repo_root.display().to_string(),
        }),
    );

    enum QualityMessage {
        Progress(QualityProgressEvent),
        Done(Result<(String, GradeReport), GardenerError>),
    }

    let (tx, rx) = mpsc::channel::<QualityMessage>();

    std::thread::scope(|thread_scope| {
        let tx_clone = tx.clone();
        thread_scope.spawn(move || {
            let on_progress = |event: QualityProgressEvent| {
                let _ = tx_clone.send(QualityMessage::Progress(event));
            };
            let result = try_pipeline_quality_report(cfg, repo_root, runtime, Some(&on_progress));
            let _ = tx.send(QualityMessage::Done(result));
        });

        let is_tty = runtime.terminal.stdin_is_tty();

        // Show dimension intro screen while agents start up
        if is_tty {
            runtime.terminal.draw_quality_intro()?;
        }

        let mut activity_lines: Vec<String> = Vec::new();
        let max_activity_lines = {
            let (_, h) = runtime.terminal.draw_dimensions();
            (h as usize).saturating_sub(5).max(20)
        };
        loop {
            match rx.recv_timeout(Duration::from_secs(10)) {
                Ok(QualityMessage::Progress(event)) => {
                    let ts = now_hhmmss();
                    let summary = summarize_quality_event(&event);
                    let line = format!("{ts} {summary}");
                    activity_lines.push(line);
                    if activity_lines.len() > max_activity_lines {
                        activity_lines.drain(..activity_lines.len() - max_activity_lines);
                    }
                    if is_tty {
                        runtime.terminal.draw_quality_grading(&activity_lines)?;
                    } else {
                        runtime
                            .terminal
                            .write_line(&format!("startup quality grading: {summary}"))?;
                    }
                }
                Ok(QualityMessage::Done(result)) => return result,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // Re-render current state to keep terminal alive; no spam lines
                    if is_tty {
                        runtime.terminal.draw_quality_grading(&activity_lines)?;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(GardenerError::Process(
                        "quality grading worker channel disconnected".to_string(),
                    ));
                }
            }
        }
    })
}

/// Format a QualityProgressEvent into a human-readable string.
fn summarize_quality_event(event: &QualityProgressEvent) -> String {
    match event {
        QualityProgressEvent::PhaseStarted(name) => format!("[{name}] Started"),
        QualityProgressEvent::AgentUpdate {
            agent_name,
            summary,
        } => {
            format!("[{agent_name}] {summary}")
        }
        QualityProgressEvent::AgentCompleted(name) => format!("[{name}] Completed"),
        QualityProgressEvent::AgentFailed { agent_name, error } => {
            format!("[{agent_name}] FAILED: {error}")
        }
        QualityProgressEvent::PhaseCompleted(name) => format!("[{name}] Completed"),
    }
}

#[allow(clippy::too_many_arguments)]
fn run_seed_with_heartbeat<F>(
    runtime: &ProductionRuntime,
    scope: &RuntimeScope,
    cfg: &AppConfig,
    profile: &crate::repo_intelligence::RepoIntelligenceProfile,
    quality_doc: &str,
    backlog_snapshot: &str,
    rejected_tasks: &str,
    progress: &mut F,
    grade_report: Option<&GradeReport>,
) -> Result<(), GardenerError>
where
    F: FnMut(&str) -> Result<(), GardenerError>,
{
    append_run_log(
        "debug",
        "startup.backlog_seed.heartbeat.started",
        json!({
            "repo_root": scope.repo_root.as_ref().map(|p| p.display().to_string()),
            "profile_set": !cfg.seeding.model.is_empty(),
        }),
    );
    enum SeedProgressMessage {
        AgentUpdate(String),
        Done(Result<(), GardenerError>),
    }
    let (tx, rx) = mpsc::channel::<SeedProgressMessage>();

    std::thread::scope(|thread_scope| {
        thread_scope.spawn(|| {
            let mut on_event = |event: &AgentEvent| {
                if let Some(summary) = summarize_agent_event(event) {
                    let _ = tx.send(SeedProgressMessage::AgentUpdate(summary));
                }
            };
            let result = seed_backlog_if_needed_with_events(
                runtime.process_runner.as_ref(),
                scope,
                cfg,
                profile,
                quality_doc,
                backlog_snapshot,
                rejected_tasks,
                Some(&mut on_event),
                grade_report,
            );
            let _ = tx.send(SeedProgressMessage::Done(result));
        });

        let is_tty = runtime.terminal.stdin_is_tty();
        let mut activity_lines: Vec<String> = Vec::new();
        let max_activity_lines = {
            let (_, h) = runtime.terminal.draw_dimensions();
            // reserve ~5 rows for header/footer/border chrome, fill the rest
            (h as usize).saturating_sub(5).max(20)
        };
        let mut last_event: Option<String> = None;
        loop {
            match rx.recv_timeout(Duration::from_secs(10)) {
                Ok(SeedProgressMessage::AgentUpdate(update)) => {
                    if last_event.as_deref() != Some(update.as_str()) {
                        let line = format!("{} {update}", now_hhmmss());
                        activity_lines.push(line);
                        if activity_lines.len() > max_activity_lines {
                            activity_lines.drain(..activity_lines.len() - max_activity_lines);
                        }
                        if is_tty {
                            runtime.terminal.draw_seeding(&activity_lines)?;
                        } else {
                            runtime
                                .terminal
                                .write_line(&format!("startup backlog seeding: {update}"))?;
                        }
                        progress(&update)?;
                        last_event = Some(update);
                    }
                }
                Ok(SeedProgressMessage::Done(result)) => return result,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // Re-render current state to keep terminal alive; no spam lines
                    if is_tty {
                        runtime.terminal.draw_seeding(&activity_lines)?;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(GardenerError::Process(
                        "backlog seeding worker channel disconnected".to_string(),
                    ));
                }
            }
        }
    })
}

#[allow(clippy::too_many_arguments)]
fn run_seed_recommendations_with_heartbeat<F>(
    runtime: &ProductionRuntime,
    scope: &RuntimeScope,
    cfg: &AppConfig,
    profile: &crate::repo_intelligence::RepoIntelligenceProfile,
    quality_doc: &str,
    backlog_snapshot: &str,
    rejected_tasks: &str,
    progress: &mut F,
    grade_report: Option<&GradeReport>,
) -> Result<Vec<SeedTask>, GardenerError>
where
    F: FnMut(&str) -> Result<(), GardenerError>,
{
    append_run_log(
        "debug",
        "startup.backlog_seed.dry_run.heartbeat.started",
        json!({
            "repo_root": scope.repo_root.as_ref().map(|p| p.display().to_string()),
            "profile_set": !cfg.seeding.model.is_empty(),
        }),
    );
    enum SeedProgressMessage {
        AgentUpdate(String),
        Done(Result<Vec<SeedTask>, GardenerError>),
    }
    let (tx, rx) = mpsc::channel::<SeedProgressMessage>();

    std::thread::scope(|thread_scope| {
        thread_scope.spawn(|| {
            let mut on_event = |event: &AgentEvent| {
                if let Some(summary) = summarize_agent_event(event) {
                    let _ = tx.send(SeedProgressMessage::AgentUpdate(summary));
                }
            };
            let result = recommend_seed_tasks_with_events(
                runtime.process_runner.as_ref(),
                scope,
                cfg,
                profile,
                quality_doc,
                backlog_snapshot,
                rejected_tasks,
                Some(&mut on_event),
                grade_report,
            );
            let _ = tx.send(SeedProgressMessage::Done(result));
        });

        let is_tty = runtime.terminal.stdin_is_tty();
        let mut activity_lines: Vec<String> = Vec::new();
        let max_activity_lines = {
            let (_, h) = runtime.terminal.draw_dimensions();
            (h as usize).saturating_sub(5).max(20)
        };
        let mut last_event: Option<String> = None;
        loop {
            match rx.recv_timeout(Duration::from_secs(10)) {
                Ok(SeedProgressMessage::AgentUpdate(update)) => {
                    if last_event.as_deref() != Some(update.as_str()) {
                        let line = format!("{} {update}", now_hhmmss());
                        activity_lines.push(line);
                        if activity_lines.len() > max_activity_lines {
                            activity_lines.drain(..activity_lines.len() - max_activity_lines);
                        }
                        if is_tty {
                            runtime.terminal.draw_seeding(&activity_lines)?;
                        } else {
                            runtime.terminal.write_line(&format!(
                                "startup backlog seeding dry-run: {update}"
                            ))?;
                        }
                        progress(&update)?;
                        last_event = Some(update);
                    }
                }
                Ok(SeedProgressMessage::Done(result)) => return result,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // Re-render current state to keep terminal alive; no spam lines
                    if is_tty {
                        runtime.terminal.draw_seeding(&activity_lines)?;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(GardenerError::Process(
                        "backlog seeding dry-run worker channel disconnected".to_string(),
                    ));
                }
            }
        }
    })
}

#[allow(clippy::too_many_arguments)]
fn run_seed_refinement_with_heartbeat(
    runtime: &ProductionRuntime,
    scope: &RuntimeScope,
    cfg: &AppConfig,
    profile: &crate::repo_intelligence::RepoIntelligenceProfile,
    quality_doc: &str,
    backlog_snapshot: &str,
    rejected_tasks: &str,
    tasks_with_feedback: &[(SeedTask, String)],
    grade_report: Option<&GradeReport>,
) -> Result<Vec<SeedTask>, GardenerError> {
    append_run_log(
        "debug",
        "startup.backlog_seed.refine.heartbeat.started",
        json!({
            "task_count": tasks_with_feedback.len(),
        }),
    );
    enum SeedProgressMessage {
        AgentUpdate(String),
        Done(Result<Vec<SeedTask>, GardenerError>),
    }
    let (tx, rx) = mpsc::channel::<SeedProgressMessage>();

    std::thread::scope(|thread_scope| {
        thread_scope.spawn(|| {
            let mut on_event = |event: &AgentEvent| {
                if let Some(summary) = summarize_agent_event(event) {
                    let _ = tx.send(SeedProgressMessage::AgentUpdate(summary));
                }
            };
            let result = refine_seed_tasks_with_events(
                runtime.process_runner.as_ref(),
                scope,
                cfg,
                profile,
                quality_doc,
                backlog_snapshot,
                rejected_tasks,
                tasks_with_feedback,
                Some(&mut on_event),
                grade_report,
            );
            let _ = tx.send(SeedProgressMessage::Done(result));
        });

        let is_tty = runtime.terminal.stdin_is_tty();
        let mut activity_lines: Vec<String> = Vec::new();
        const MAX_ACTIVITY_LINES: usize = 20;
        let mut last_event: Option<String> = None;
        loop {
            match rx.recv_timeout(Duration::from_secs(10)) {
                Ok(SeedProgressMessage::AgentUpdate(update)) => {
                    if last_event.as_deref() != Some(update.as_str()) {
                        let line = format!("{} {update}", now_hhmmss());
                        activity_lines.push(line);
                        if activity_lines.len() > MAX_ACTIVITY_LINES {
                            activity_lines.drain(..activity_lines.len() - MAX_ACTIVITY_LINES);
                        }
                        if is_tty {
                            runtime.terminal.draw_seeding(&activity_lines)?;
                        }
                        last_event = Some(update);
                    }
                }
                Ok(SeedProgressMessage::Done(result)) => return result,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // Re-render current state to keep terminal alive; no spam lines
                    if is_tty {
                        runtime.terminal.draw_seeding(&activity_lines)?;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(GardenerError::Process(
                        "backlog seeding refinement worker channel disconnected".to_string(),
                    ));
                }
            }
        }
    })
}

fn print_seed_recommendations(
    runtime: &ProductionRuntime,
    recommendations: &[SeedTask],
) -> Result<(), GardenerError> {
    append_run_log(
        "info",
        "startup.seeding.dry_run.recommendations",
        json!({ "count": recommendations.len() }),
    );
    runtime
        .terminal
        .write_line("seed dry-run recommendations (no backlog writes):")?;
    for (index, task) in recommendations.iter().enumerate() {
        runtime.terminal.write_line(&format!(
            "{}. [{}] ({}) {}",
            index + 1,
            task.priority,
            task.domain,
            task.title
        ))?;
        runtime
            .terminal
            .write_line(&format!("   details: {}", task.details))?;
        runtime
            .terminal
            .write_line(&format!("   rationale: {}", task.rationale))?;
    }
    Ok(())
}

/// Run interactive seeding: reads backlog_approval from profile and routes
/// to either auto-seed (v2 direct-write) or review mode (v1 dry-run + wizard).
/// The seeding TUI screen is shown during agent execution on TTY.
pub fn run_interactive_seeding(
    runtime: &ProductionRuntime,
    scope: &RuntimeScope,
    cfg: &AppConfig,
    store: &BacklogStore,
    force_seed_backlog: bool,
) -> Result<usize, GardenerError> {
    let profile_loc = profile_path(scope, cfg);
    let profile = read_profile(runtime.file_system.as_ref(), &profile_loc)?;
    let quality_path = quality_report_path(cfg, scope);
    let quality_doc = runtime.file_system.read_to_string(&quality_path)?;
    let backlog_snapshot = summarize_active_backlog(store)?;
    let rejected_tasks_formatted = format_rejected_seeds(store);
    let existing_count = store.count_active_tasks()?;

    if !should_seed_backlog(
        true,
        cfg.execution.test_mode,
        existing_count,
        force_seed_backlog,
    ) {
        append_run_log(
            "info",
            "startup.interactive_seeding.skipped",
            json!({ "existing_count": existing_count }),
        );
        return Ok(0);
    }

    let backlog_approval = profile.user_validated.backlog_approval;
    append_run_log(
        "info",
        "startup.interactive_seeding.started",
        json!({
            "backlog_approval": backlog_approval,
            "backend": format!("{:?}", cfg.seeding.backend),
            "model": cfg.seeding.model,
        }),
    );

    let mut seeded = 0usize;

    if backlog_approval {
        // Review mode: dry-run to get recommendations, then show review wizard.
        // Supports a refinement loop: tasks marked Refine go back to the agent.
        let mut progress = |_detail: &str| -> Result<(), GardenerError> { Ok(()) };
        let mut pending_tasks: Vec<SeedTask> = run_seed_recommendations_with_heartbeat(
            runtime,
            scope,
            cfg,
            &profile,
            &quality_doc,
            &backlog_snapshot,
            &rejected_tasks_formatted,
            &mut progress,
            None, // GradeReport not available in interactive path; falls back to Markdown parsing
        )?;

        if pending_tasks.is_empty() {
            append_run_log(
                "info",
                "startup.interactive_seeding.review.empty",
                json!({}),
            );
            return Ok(0);
        }

        let mut round = 0usize;
        loop {
            // Close the live seeding TUI before launching the blocking review wizard.
            let _ = runtime.terminal.close_ui();

            let decisions = crate::tui::run_seed_review_wizard(&pending_tasks, round)?;
            let mut to_refine: Vec<(SeedTask, String)> = Vec::new();

            for (index, decision) in decisions.into_iter().enumerate() {
                match decision {
                    crate::tui::ReviewDecision::Keep => {
                        let task = &pending_tasks[index];
                        let priority = match task.priority.as_str() {
                            "P0" => Priority::P0,
                            "P2" => Priority::P2,
                            _ => Priority::P1,
                        };
                        store.upsert_task(NewTask {
                            kind: TaskKind::Maintenance,
                            title: task.title.clone(),
                            details: task.details.clone(),
                            scope_key: task.domain.clone(),
                            rationale: task.rationale.clone(),
                            priority,
                            source: "interactive_seed_review".to_string(),
                            related_pr: None,
                            related_branch: None,
                        })?;
                        seeded += 1;
                    }
                    crate::tui::ReviewDecision::Discard(reason) => {
                        let _ =
                            store.insert_rejected_seed(&pending_tasks[index], reason.as_deref());
                    }
                    crate::tui::ReviewDecision::Refine(feedback) => {
                        to_refine.push((pending_tasks[index].clone(), feedback));
                    }
                }
            }

            append_run_log(
                "info",
                "startup.interactive_seeding.review.round_completed",
                json!({
                    "round": round,
                    "kept": seeded,
                    "to_refine": to_refine.len(),
                }),
            );

            if to_refine.is_empty() {
                break;
            }

            // Re-seed with refinement feedback
            round += 1;
            pending_tasks = run_seed_refinement_with_heartbeat(
                runtime,
                scope,
                cfg,
                &profile,
                &quality_doc,
                &backlog_snapshot,
                &rejected_tasks_formatted,
                &to_refine,
                None, // GradeReport not available in interactive refinement path
            )?;

            if pending_tasks.is_empty() {
                break;
            }
        }

        append_run_log(
            "info",
            "startup.interactive_seeding.review.completed",
            json!({
                "total_rounds": round + 1,
                "kept": seeded,
            }),
        );
    } else {
        // Auto-seed mode: direct-write with seeding screen.
        let mut progress = |_detail: &str| -> Result<(), GardenerError> { Ok(()) };
        if let Err(err) = run_seed_with_heartbeat(
            runtime,
            scope,
            cfg,
            &profile,
            &quality_doc,
            &backlog_snapshot,
            &rejected_tasks_formatted,
            &mut progress,
            None, // GradeReport not available in interactive auto-seed path
        ) {
            append_run_log(
                "warn",
                "startup.interactive_seeding.auto.failed",
                json!({ "error": err.to_string() }),
            );
            runtime
                .terminal
                .write_line(&format!("WARN backlog seeding failed: {err}"))?;
            return Ok(0);
        }
        let post_count = store.count_active_tasks()?;
        seeded = post_count.saturating_sub(existing_count);
        append_run_log(
            "info",
            "startup.interactive_seeding.auto.completed",
            json!({
                "seeded": seeded,
                "existing_count": existing_count,
                "post_count": post_count,
            }),
        );
    }

    Ok(seeded)
}

fn summarize_active_backlog(store: &BacklogStore) -> Result<String, GardenerError> {
    append_run_log(
        "debug",
        "startup.summarize_active_backlog.started",
        json!({}),
    );
    let mut lines = Vec::new();
    for task in store.list_backlog_tasks()?.into_iter() {
        if matches!(
            task.status,
            crate::backlog_store::TaskStatus::Complete | crate::backlog_store::TaskStatus::Failed
        ) {
            continue;
        }
        let details = task.details.replace('\n', " ").trim().to_string();
        lines.push(format!(
            "- [{}] {} ({}) {} :: {}",
            task.priority.as_str(),
            task.title,
            task.scope_key,
            task.status.as_str(),
            details
        ));
        if lines.len() >= 40 {
            break;
        }
    }
    if lines.is_empty() {
        Ok("No active backlog tasks.".to_string())
    } else {
        Ok(lines.join("\n"))
    }
}

fn format_rejected_seeds(store: &BacklogStore) -> String {
    match store.list_rejected_seeds() {
        Ok(seeds) if !seeds.is_empty() => {
            let mut lines = Vec::new();
            for seed in &seeds {
                if seed.rejection_reason.is_empty() {
                    lines.push(format!(
                        "- \"{}\" ({}) — rejected (no reason given)",
                        seed.title, seed.domain
                    ));
                } else {
                    lines.push(format!(
                        "- \"{}\" ({}) — rejected because: \"{}\"",
                        seed.title, seed.domain, seed.rejection_reason
                    ));
                }
            }
            lines.join("\n")
        }
        _ => String::new(),
    }
}

pub fn quality_stamp_path(quality_path: &std::path::Path) -> PathBuf {
    PathBuf::from(format!("{}.stamp", quality_path.display()))
}

/// Path for the sidecar GradeReport JSON cache (alongside the quality Markdown report).
fn grade_report_cache_path(quality_path: &std::path::Path) -> PathBuf {
    PathBuf::from(format!("{}.grade-report.json", quality_path.display()))
}

/// Public accessor for the sidecar GradeReport JSON cache path.
pub fn grade_report_cache_path_for(quality_path: &std::path::Path) -> PathBuf {
    grade_report_cache_path(quality_path)
}

fn report_stamp_is_stale(
    runtime: &ProductionRuntime,
    cfg: &AppConfig,
    stamp_path: &std::path::Path,
) -> Result<bool, GardenerError> {
    report_stamp_is_stale_with_context(
        runtime.file_system.as_ref(),
        runtime.clock.as_ref(),
        cfg,
        stamp_path,
    )
}

fn report_stamp_is_stale_with_context(
    file_system: &dyn FileSystem,
    clock: &dyn Clock,
    cfg: &AppConfig,
    stamp_path: &std::path::Path,
) -> Result<bool, GardenerError> {
    if !file_system.exists(stamp_path) {
        return Ok(true);
    }
    let raw = file_system.read_to_string(stamp_path)?;
    let stamp = raw.trim().parse::<u64>().unwrap_or(0);
    let now = clock
        .now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let ttl_seconds = cfg
        .quality_report
        .stale_after_days
        .saturating_mul(24 * 60 * 60)
        .max(REPORT_TTL_SECONDS);
    if now.saturating_sub(stamp) > ttl_seconds {
        return Ok(true);
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::{
        backlog_db_path, backup_db_if_exists, print_seed_recommendations, quality_stamp_path,
        report_stamp_is_stale, should_seed_backlog,
    };
    use crate::config::AppConfig;
    use crate::runtime::{
        FakeClock, FakeFileSystem, FakeProcessRunner, FakeTerminal, FileSystem, ProductionRuntime,
    };
    use crate::seed_runner::SeedTask;
    use crate::types::RuntimeScope;
    use std::sync::Arc;
    use std::time::{Duration, SystemTime};
    use tempfile::tempdir;

    #[test]
    fn seeding_gate_requires_empty_backlog() {
        assert!(should_seed_backlog(true, false, 0, false));
        assert!(!should_seed_backlog(true, false, 1, false));
        assert!(should_seed_backlog(true, false, 1, true));
        assert!(!should_seed_backlog(false, false, 0, true));
        assert!(!should_seed_backlog(true, true, 0, true));
    }

    #[test]
    fn backup_db_if_exists_copies_file() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let db = dir.path().join("backlog.sqlite");

        // Create a valid SQLite DB via BacklogStore::open, then drop it.
        {
            let _store = crate::backlog_store::BacklogStore::open(&db).expect("create valid db");
        }

        let meta_before = std::fs::metadata(&db).expect("metadata");
        let bak = backup_db_if_exists(&db).expect("backup").expect("Some");
        assert!(bak.exists());
        let meta_bak = std::fs::metadata(&bak).expect("bak metadata");
        assert_eq!(meta_before.len(), meta_bak.len());
    }

    #[test]
    fn backup_db_if_exists_skips_missing_file() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let db = dir.path().join("does-not-exist.sqlite");
        let result = backup_db_if_exists(&db).expect("no error");
        assert!(result.is_none());
    }

    #[test]
    fn backup_db_if_exists_skips_zero_byte() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let db = dir.path().join("backlog.sqlite");
        std::fs::write(&db, b"").expect("create zero-byte file");
        let result = backup_db_if_exists(&db).expect("no error");
        assert!(result.is_none());
    }

    #[test]
    fn backlog_db_path_respects_test_mode() {
        let dir = tempdir().expect("tempdir");
        let mut cfg = AppConfig::default();
        cfg.execution.test_mode = true;
        let scope = RuntimeScope {
            process_cwd: dir.path().to_path_buf(),
            repo_root: Some(dir.path().to_path_buf()),
            working_dir: dir.path().to_path_buf(),
        };
        let path = backlog_db_path(&cfg, &scope);
        assert!(path.ends_with(".gardener/backlog.sqlite"));
        assert!(!path.display().to_string().contains(".cache/gardener"));
    }

    #[test]
    fn backlog_db_path_defaults_to_non_cache_home_path() {
        let dir = tempdir().expect("tempdir");
        let cfg = AppConfig::default();
        let scope = RuntimeScope {
            process_cwd: dir.path().to_path_buf(),
            repo_root: Some(dir.path().to_path_buf()),
            working_dir: dir.path().to_path_buf(),
        };
        let path = backlog_db_path(&cfg, &scope);
        assert!(path.ends_with(".gardener/backlog.sqlite"));
        assert!(!path.display().to_string().contains(".cache/gardener"));
    }

    #[test]
    fn quality_stamp_path_appends_extension() {
        let path = quality_stamp_path(std::path::Path::new("/tmp/report.md"));
        assert_eq!(path, std::path::PathBuf::from("/tmp/report.md.stamp"));
    }

    #[test]
    fn print_seed_recommendations_writes_expected_lines() {
        let terminal = Arc::new(FakeTerminal::default());
        let runtime = ProductionRuntime {
            clock: Arc::new(FakeClock::default()),
            file_system: Arc::new(FakeFileSystem::default()),
            process_runner: Arc::new(FakeProcessRunner::default()),
            terminal: terminal.clone(),
        };
        let tasks = vec![SeedTask {
            title: "Improve startup diagnostics".to_string(),
            details: "Add startup error categorization and operator guidance.".to_string(),
            rationale: "Recent startup failures lacked enough context for fast triage.".to_string(),
            domain: "startup".to_string(),
            priority: "P1".to_string(),
        }];

        print_seed_recommendations(&runtime, &tasks).expect("print recommendations");
        let lines = terminal.written_lines();
        assert!(lines
            .iter()
            .any(|line| line.contains("seed dry-run recommendations")));
        assert!(lines.iter().any(|line| line.contains("[P1] (startup)")));
        assert!(lines
            .iter()
            .any(|line| line.contains("Recent startup failures lacked enough context")));
    }

    #[test]
    fn report_stamp_is_stale_for_missing_stamp() {
        let dir = tempdir().expect("tempdir");
        let scope = RuntimeScope {
            process_cwd: dir.path().to_path_buf(),
            repo_root: Some(dir.path().to_path_buf()),
            working_dir: dir.path().to_path_buf(),
        };
        let cfg = AppConfig::default();
        let runtime = ProductionRuntime {
            clock: Arc::new(FakeClock::default()),
            file_system: Arc::new(FakeFileSystem::default()),
            process_runner: Arc::new(FakeProcessRunner::default()),
            terminal: Arc::new(FakeTerminal::default()),
        };
        let stale = report_stamp_is_stale(
            &runtime,
            &cfg,
            &scope.working_dir.join("report.md.stamp"),
        )
        .expect("stale");
        assert!(stale);
    }

    #[test]
    fn report_stamp_is_stale_when_ttl_exceeded() {
        let dir = tempdir().expect("tempdir");

        let stamp = dir.path().join("report.md.stamp");
        let fs = FakeFileSystem::default();
        fs.write_string(&stamp, "0").expect("seed stamp");
        let mut cfg = AppConfig::default();
        cfg.quality_report.stale_after_days = 0;
        let runtime = ProductionRuntime {
            clock: Arc::new(FakeClock::new(
                SystemTime::UNIX_EPOCH + Duration::from_secs(10_000),
            )),
            file_system: Arc::new(fs),
            process_runner: Arc::new(FakeProcessRunner::default()),
            terminal: Arc::new(FakeTerminal::default()),
        };
        assert!(report_stamp_is_stale(&runtime, &cfg, &stamp).expect("stale by ttl"));
    }
}
