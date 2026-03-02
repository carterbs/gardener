use crate::backlog_store::{BacklogStore, NewTask};
use crate::config::AppConfig;
use crate::errors::GardenerError;
use crate::logging::append_run_log;
use crate::pr_audit::reconcile_open_prs;
use crate::priority::Priority;
use crate::protocol::{AgentEvent, AgentEventKind};
use crate::quality_grades::render_quality_grade_document;
use crate::repo_intelligence::read_profile;
use crate::runtime::{ProcessRequest, ProductionRuntime};
use crate::seed_runner::SeedTask;
use crate::seeding::{recommend_seed_tasks_with_events, seed_backlog_if_needed_with_events};
use crate::task_identity::TaskKind;
use crate::triage::profile_path;
use crate::types::RuntimeScope;
use crate::worktree_audit::reconcile_worktrees;
use serde_json::json;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;
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

pub fn backlog_db_path(cfg: &crate::config::AppConfig, scope: &RuntimeScope) -> PathBuf {
    if let Ok(path) = env::var("GARDENER_DB_PATH") {
        return PathBuf::from(path);
    }

    if cfg.execution.test_mode {
        return scope
            .repo_root
            .as_ref()
            .unwrap_or(&scope.working_dir)
            .join(".cache/gardener/backlog.sqlite");
    }

    if let Some(home) = env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")) {
        return PathBuf::from(home).join(".gardener").join("backlog.sqlite");
    }

    scope
        .repo_root
        .as_ref()
        .unwrap_or(&scope.working_dir)
        .join(".cache/gardener/backlog.sqlite")
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
        || report_stamp_is_stale(runtime, cfg, &stamp_path, scope)?;

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
        append_run_log(
            "info",
            "startup.quality_report.regenerating",
            json!({
                "quality_path": quality_path.display().to_string(),
                "primary_gap": profile.agent_readiness.primary_gap,
                "readiness_score": profile.agent_readiness.readiness_score,
            }),
        );
        let repo_root = scope.repo_root.as_ref().unwrap_or(&scope.working_dir);
        let quality_doc =
            render_quality_grade_document(&profile_loc.display().to_string(), &profile, repo_root);
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
        append_run_log(
            "info",
            "startup.quality_report.refreshed",
            json!({
                "quality_path": quality_path.display().to_string(),
                "stamp_ts": now,
            }),
        );
    }
    Ok((quality_path, should_regen))
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
            "No repo intelligence profile found. Run `scripts/brad-gardener --triage-only` in a terminal to complete setup."
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

    let (quality_path, quality_written) = refresh_quality_report(runtime, cfg, scope, false)?;
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
                &mut progress,
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
            let mut seeding_error: Option<String> = None;
            if let Err(err) = run_seed_with_heartbeat(
                runtime,
                scope,
                cfg,
                &profile,
                &quality_doc,
                &backlog_snapshot,
                &mut progress,
            ) {
                append_run_log(
                    "warn",
                    "startup.seeding.agent_failed",
                    json!({ "error": err.to_string() }),
                );
                progress(&format!("Seeding agent failed ({err})"))?;
                runtime
                    .terminal
                    .write_line(&format!("WARN backlog seeding failed: {err}"))?;
                seeding_error = Some(err.to_string());
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
                if let Some(err) = seeding_error {
                    append_run_log(
                        "warn",
                        "startup.seeding.direct_persisted_after_error",
                        json!({ "error": err, "task_count": agent_seeded }),
                    );
                }
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

fn run_seed_with_heartbeat<F>(
    runtime: &ProductionRuntime,
    scope: &RuntimeScope,
    cfg: &AppConfig,
    profile: &crate::repo_intelligence::RepoIntelligenceProfile,
    quality_doc: &str,
    backlog_snapshot: &str,
    progress: &mut F,
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
                if let Some(summary) = summarize_seed_agent_event(event) {
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
                Some(&mut on_event),
            );
            let _ = tx.send(SeedProgressMessage::Done(result));
        });

        let mut waited_seconds = 0u64;
        let mut last_event: Option<String> = None;
        loop {
            match rx.recv_timeout(Duration::from_secs(10)) {
                Ok(SeedProgressMessage::AgentUpdate(update)) => {
                    if last_event.as_deref() != Some(update.as_str()) {
                        progress(&update)?;
                        if !runtime.terminal.stdin_is_tty() {
                            runtime
                                .terminal
                                .write_line(&format!("startup backlog seeding: {update}"))?;
                        }
                        last_event = Some(update);
                    }
                }
                Ok(SeedProgressMessage::Done(result)) => return result,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    waited_seconds = waited_seconds.saturating_add(10);
                    progress(&format!(
                        "Backlog seeding agent still running ({waited_seconds}s elapsed); waiting for model output"
                    ))?;
                    if !runtime.terminal.stdin_is_tty() {
                        runtime.terminal.write_line(&format!(
                            "startup backlog seeding: still running, elapsed={}s",
                            waited_seconds
                        ))?;
                    }
                    if waited_seconds == 60 {
                        progress(
                            "Backlog seeding is taking longer than expected; this can happen during first-run auth or slow model/network response",
                        )?;
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

fn run_seed_recommendations_with_heartbeat<F>(
    runtime: &ProductionRuntime,
    scope: &RuntimeScope,
    cfg: &AppConfig,
    profile: &crate::repo_intelligence::RepoIntelligenceProfile,
    quality_doc: &str,
    backlog_snapshot: &str,
    progress: &mut F,
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
                if let Some(summary) = summarize_seed_agent_event(event) {
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
                Some(&mut on_event),
            );
            let _ = tx.send(SeedProgressMessage::Done(result));
        });

        let mut waited_seconds = 0u64;
        let mut last_event: Option<String> = None;
        loop {
            match rx.recv_timeout(Duration::from_secs(10)) {
                Ok(SeedProgressMessage::AgentUpdate(update)) => {
                    if last_event.as_deref() != Some(update.as_str()) {
                        progress(&update)?;
                        if !runtime.terminal.stdin_is_tty() {
                            runtime.terminal.write_line(&format!(
                                "startup backlog seeding dry-run: {update}"
                            ))?;
                        }
                        last_event = Some(update);
                    }
                }
                Ok(SeedProgressMessage::Done(result)) => return result,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    waited_seconds = waited_seconds.saturating_add(10);
                    progress(&format!(
                        "Backlog seeding dry-run still running ({waited_seconds}s elapsed); waiting for model output"
                    ))?;
                    if !runtime.terminal.stdin_is_tty() {
                        runtime.terminal.write_line(&format!(
                            "startup backlog seeding dry-run: still running, elapsed={}s",
                            waited_seconds
                        ))?;
                    }
                    if waited_seconds == 60 {
                        progress(
                            "Backlog seeding dry-run is taking longer than expected; this can happen during first-run auth or slow model/network response",
                        )?;
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

fn summarize_active_backlog(store: &BacklogStore) -> Result<String, GardenerError> {
    append_run_log(
        "debug",
        "startup.summarize_active_backlog.started",
        json!({}),
    );
    let mut lines = Vec::new();
    for task in store.list_tasks()?.into_iter() {
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

fn summarize_seed_agent_event(event: &AgentEvent) -> Option<String> {
    match event.kind {
        AgentEventKind::ThreadStarted => Some("Agent session started".to_string()),
        AgentEventKind::TurnStarted => Some("Agent turn started".to_string()),
        AgentEventKind::ToolCall => {
            let label =
                extract_event_label(&event.payload).unwrap_or_else(|| event.raw_type.clone());
            let command = extract_command_preview(&event.payload);
            Some(match command {
                Some(cmd) => format!("Agent activity: {label} started: `{cmd}`"),
                None => format!("Agent activity: {label} started"),
            })
        }
        AgentEventKind::ToolResult => {
            let label =
                extract_event_label(&event.payload).unwrap_or_else(|| event.raw_type.clone());
            let command = extract_command_preview(&event.payload);
            Some(match command {
                Some(cmd) => format!("Agent activity: {label} completed: `{cmd}`"),
                None => format!("Agent activity: {label} completed"),
            })
        }
        AgentEventKind::Message => {
            extract_message_preview(&event.payload).map(|msg| format!("Agent thought: {msg}"))
        }
        AgentEventKind::TurnCompleted => Some("Agent turn completed".to_string()),
        AgentEventKind::TurnFailed => Some(format!(
            "Agent turn failed: {}",
            extract_event_label(&event.payload).unwrap_or_else(|| event.raw_type.clone())
        )),
        AgentEventKind::Unknown => None,
    }
}

fn extract_event_label(payload: &serde_json::Value) -> Option<String> {
    let candidates = [
        payload
            .pointer("/item/type")
            .and_then(serde_json::Value::as_str),
        payload
            .pointer("/item/name")
            .and_then(serde_json::Value::as_str),
        payload.pointer("/name").and_then(serde_json::Value::as_str),
        payload
            .pointer("/tool_name")
            .and_then(serde_json::Value::as_str),
        payload
            .pointer("/reason")
            .and_then(serde_json::Value::as_str),
        payload
            .pointer("/error/message")
            .and_then(serde_json::Value::as_str),
    ];
    candidates
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|s| !s.is_empty())
        .map(ToString::to_string)
}

fn extract_message_preview(payload: &serde_json::Value) -> Option<String> {
    let candidates = [
        payload
            .pointer("/delta/text")
            .and_then(serde_json::Value::as_str),
        payload.pointer("/text").and_then(serde_json::Value::as_str),
        payload
            .pointer("/message")
            .and_then(serde_json::Value::as_str),
    ];
    candidates
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|s| !s.is_empty())
        .map(|s| {
            let mut clipped = s.to_string();
            if clipped.len() > 120 {
                clipped.truncate(120);
                clipped.push_str("...");
            }
            clipped
        })
}

fn extract_command_preview(payload: &serde_json::Value) -> Option<String> {
    let candidates = [
        payload
            .pointer("/item/command")
            .and_then(serde_json::Value::as_str),
        payload
            .pointer("/item/command_line")
            .and_then(serde_json::Value::as_str),
        payload
            .pointer("/item/cmd")
            .and_then(serde_json::Value::as_str),
        payload
            .pointer("/command")
            .and_then(serde_json::Value::as_str),
        payload
            .pointer("/command_line")
            .and_then(serde_json::Value::as_str),
        payload.pointer("/cmd").and_then(serde_json::Value::as_str),
        payload
            .pointer("/item/input/command")
            .and_then(serde_json::Value::as_str),
        payload
            .pointer("/item/input/cmd")
            .and_then(serde_json::Value::as_str),
    ];
    candidates
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|s| !s.is_empty())
        .map(|s| {
            let mut clipped = s.to_string();
            if clipped.len() > 120 {
                clipped.truncate(120);
                clipped.push_str("...");
            }
            clipped
        })
}

fn quality_stamp_path(quality_path: &std::path::Path) -> PathBuf {
    PathBuf::from(format!("{}.stamp", quality_path.display()))
}

fn report_stamp_is_stale(
    runtime: &ProductionRuntime,
    cfg: &AppConfig,
    stamp_path: &std::path::Path,
    scope: &RuntimeScope,
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
    let ttl_seconds = cfg
        .quality_report
        .stale_after_days
        .saturating_mul(24 * 60 * 60)
        .max(REPORT_TTL_SECONDS);
    if now.saturating_sub(stamp) > ttl_seconds {
        return Ok(true);
    }
    if cfg.quality_report.stale_if_head_commit_differs {
        let profile_loc = crate::triage::profile_path(scope, cfg);
        if let Ok(profile) = read_profile(runtime.file_system.as_ref(), &profile_loc) {
            if let Ok(current_head) = crate::repo_intelligence::current_head_sha(
                runtime.process_runner.as_ref(),
                &scope.working_dir,
            ) {
                if current_head != profile.meta.head_sha {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::{
        backlog_db_path, backup_db_if_exists, extract_command_preview, extract_event_label,
        extract_message_preview, print_seed_recommendations, quality_stamp_path,
        report_stamp_is_stale, should_seed_backlog, summarize_seed_agent_event,
    };
    use crate::config::AppConfig;
    use crate::protocol::{AgentEvent, AgentEventKind};
    use crate::repo_intelligence::{self, RepoIntelligenceProfile};
    use crate::runtime::{
        FakeClock, FakeFileSystem, FakeProcessRunner, FakeTerminal, FileSystem, ProcessOutput,
        ProductionRuntime,
    };
    use crate::seed_runner::SeedTask;
    use crate::triage;
    use crate::triage_discovery::DiscoveryAssessment;
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
        assert_eq!(
            backlog_db_path(&cfg, &scope),
            dir.path().join(".cache/gardener/backlog.sqlite")
        );
    }

    #[test]
    fn quality_stamp_path_appends_extension() {
        let path = quality_stamp_path(std::path::Path::new("/tmp/report.md"));
        assert_eq!(path, std::path::PathBuf::from("/tmp/report.md.stamp"));
    }

    #[test]
    fn extract_event_helpers_read_nested_and_fallback_fields() {
        assert_eq!(
            extract_event_label(&serde_json::json!({
                "item": { "name": "test" },
                "tool_name": "fallback"
            })),
            Some("test".to_string())
        );
        assert_eq!(
            extract_command_preview(&serde_json::json!({
                "item": {
                    "command_line": "cargo test --all-targets --help"
                }
            })),
            Some("cargo test --all-targets --help".to_string())
        );
        assert_eq!(
            extract_message_preview(&serde_json::json!({
                "delta": {
                    "text": "short payload"
                }
            })),
            Some("short payload".to_string())
        );
    }

    #[test]
    fn summarize_seed_agent_event_handles_multiple_kinds() {
        assert_eq!(
            summarize_seed_agent_event(&AgentEvent {
                protocol_version: 1,
                kind: AgentEventKind::ThreadStarted,
                raw_type: "thread.started".into(),
                payload: serde_json::json!({}),
            }),
            Some("Agent session started".to_string())
        );
        assert!(summarize_seed_agent_event(&AgentEvent {
            protocol_version: 1,
            kind: AgentEventKind::ToolCall,
            raw_type: "item.started".into(),
            payload: serde_json::json!({"item": {"command":"echo hi"}}),
        })
        .as_deref()
        .expect("tool call preview")
        .contains("echo hi"));
        assert_eq!(
            summarize_seed_agent_event(&AgentEvent {
                protocol_version: 1,
                kind: AgentEventKind::TurnFailed,
                raw_type: "turn.failed".into(),
                payload: serde_json::json!({}),
            }),
            Some("Agent turn failed: turn.failed".to_string())
        );
        assert_eq!(
            summarize_seed_agent_event(&AgentEvent {
                protocol_version: 1,
                kind: AgentEventKind::Unknown,
                raw_type: "unknown".into(),
                payload: serde_json::json!({}),
            }),
            None
        );
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
            &scope,
        )
        .expect("stale");
        assert!(stale);
    }

    #[test]
    fn report_stamp_is_stale_when_ttl_exceeded_or_head_commit_changes() {
        let dir = tempdir().expect("tempdir");
        let scope = RuntimeScope {
            process_cwd: dir.path().to_path_buf(),
            repo_root: Some(dir.path().to_path_buf()),
            working_dir: dir.path().to_path_buf(),
        };

        let stamp = dir.path().join("report.md.stamp");
        let fs = FakeFileSystem::default();
        fs.write_string(&stamp, "0").expect("seed stamp");
        let mut cfg = AppConfig::default();
        cfg.quality_report.stale_after_days = 0;
        cfg.quality_report.stale_if_head_commit_differs = false;
        let runtime = ProductionRuntime {
            clock: Arc::new(FakeClock::new(
                SystemTime::UNIX_EPOCH + Duration::from_secs(10_000),
            )),
            file_system: Arc::new(fs),
            process_runner: Arc::new(FakeProcessRunner::default()),
            terminal: Arc::new(FakeTerminal::default()),
        };
        assert!(report_stamp_is_stale(&runtime, &cfg, &stamp, &scope).expect("stale by ttl"));

        let fs = FakeFileSystem::default();
        fs.write_string(&stamp, "9_900").expect("seed fresh stamp");
        let mut cfg = AppConfig::default();
        cfg.quality_report.stale_after_days = 1;
        cfg.quality_report.stale_if_head_commit_differs = true;
        let profile = RepoIntelligenceProfile {
            meta: repo_intelligence::RepoMeta {
                schema_version: 1,
                created_at: "0".to_string(),
                head_sha: "abc".to_string(),
                working_dir: dir.path().display().to_string(),
                repo_root: dir.path().display().to_string(),
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
        let profile_path = triage::profile_path(&scope, &cfg);
        fs.write_string(&profile_path, &toml::to_string(&profile).expect("toml"))
            .expect("write profile");
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "different\n".to_string(),
            stderr: String::new(),
        }));
        let runtime = ProductionRuntime {
            clock: Arc::new(FakeClock::new(
                SystemTime::UNIX_EPOCH + Duration::from_secs(10_000),
            )),
            file_system: Arc::new(fs),
            process_runner: Arc::new(runner),
            terminal: Arc::new(FakeTerminal::default()),
        };
        assert!(report_stamp_is_stale(&runtime, &cfg, &stamp, &scope).expect("mismatch head"));
    }
}
