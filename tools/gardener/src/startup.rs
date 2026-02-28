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
use crate::seeding::seed_backlog_if_needed_with_events;
use crate::task_identity::TaskKind;
use crate::triage::profile_path;
use crate::types::RuntimeScope;
use crate::worktree_audit::reconcile_worktrees;
use serde_json::json;
use std::path::PathBuf;
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
) -> Result<StartupSummary, GardenerError> {
    run_startup_audits_with_progress(runtime, cfg, scope, run_seeding, |_detail| Ok(()))
}

pub fn run_startup_audits_with_progress<F>(
    runtime: &ProductionRuntime,
    cfg: &mut AppConfig,
    scope: &RuntimeScope,
    run_seeding: bool,
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
            "No repo intelligence profile found. Run `brad-gardener --triage-only` in a terminal to complete setup."
                .to_string(),
        ));
    }
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
            let db_path = scope
                .repo_root
                .as_ref()
                .unwrap_or(&scope.working_dir)
                .join(".cache/gardener/backlog.sqlite");
            let store = BacklogStore::open(db_path)?;
            store.upsert_task(NewTask {
                kind: TaskKind::Maintenance,
                title: "Recovery: startup validation failed".to_string(),
                details: format!("Validation command exited with code {}", out.exit_code),
                scope_key: "startup".to_string(),
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

    let db_path = scope
        .repo_root
        .as_ref()
        .unwrap_or(&scope.working_dir)
        .join(".cache/gardener/backlog.sqlite");
    let mut seeded_tasks_upserted = 0usize;
    let existing_active_backlog_count = if run_seeding && !cfg.execution.test_mode {
        BacklogStore::open(&db_path)?.count_active_tasks()?
    } else {
        0
    };
    if should_seed_backlog(
        run_seeding,
        cfg.execution.test_mode,
        existing_active_backlog_count,
    ) {
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
        let fallback_target = cfg.orchestrator.parallelism.max(3) as usize;
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
        let seeded = match run_seed_with_heartbeat(
            runtime,
            scope,
            cfg,
            &profile,
            &quality_doc,
            &mut progress,
        ) {
            Ok(tasks) => {
                append_run_log(
                    "info",
                    "startup.seeding.agent_returned",
                    json!({ "task_count": tasks.len() }),
                );
                progress(&format!(
                    "Seeding agent returned {} candidate task(s)",
                    tasks.len()
                ))?;
                if !runtime.terminal.stdin_is_tty() {
                    runtime.terminal.write_line(&format!(
                        "startup backlog seeding: agent returned {} candidate tasks",
                        tasks.len()
                    ))?;
                }
                tasks
            }
            Err(err) => {
                append_run_log(
                    "warn",
                    "startup.seeding.agent_failed",
                    json!({
                        "error": err.to_string(),
                        "fallback_target": fallback_target,
                    }),
                );
                progress(&format!(
                    "Seeding agent failed ({err}); continuing with fallback task templates"
                ))?;
                runtime
                    .terminal
                    .write_line(&format!("WARN backlog seeding failed: {err}"))?;
                Vec::new()
            }
        };
        if !seeded.is_empty() {
            append_run_log(
                "info",
                "startup.seeding.persisting",
                json!({ "task_count": seeded.len(), "source": "seed_runner_v1" }),
            );
            progress(&format!(
                "Persisting {} seeded task(s) to backlog store",
                seeded.len()
            ))?;
            let store = BacklogStore::open(&db_path)?;
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
            append_run_log(
                "info",
                "startup.seeding.fallback",
                json!({
                    "fallback_target": fallback_target,
                    "primary_gap": profile.agent_readiness.primary_gap,
                }),
            );
            progress(&format!(
                "No seeded tasks produced; generating {} fallback bootstrap task(s)",
                fallback_target
            ))?;
            let store = BacklogStore::open(&db_path)?;
            for task in fallback_seed_tasks(&profile.agent_readiness.primary_gap, fallback_target) {
                let bootstrap = store.upsert_task(task)?;
                if !bootstrap.task_id.is_empty() {
                    seeded_tasks_upserted = seeded_tasks_upserted.saturating_add(1);
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

fn should_seed_backlog(run_seeding: bool, test_mode: bool, existing_backlog_count: usize) -> bool {
    run_seeding && !test_mode && existing_backlog_count == 0
}

fn run_seed_with_heartbeat<F>(
    runtime: &ProductionRuntime,
    scope: &RuntimeScope,
    cfg: &AppConfig,
    profile: &crate::repo_intelligence::RepoIntelligenceProfile,
    quality_doc: &str,
    progress: &mut F,
) -> Result<Vec<crate::seed_runner::SeedTask>, GardenerError>
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
        Done(Result<Vec<crate::seed_runner::SeedTask>, GardenerError>),
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

fn fallback_seed_tasks(primary_gap: &str, target: usize) -> Vec<NewTask> {
    let templates = [
        (
            "Bootstrap backlog",
            "Seed runner returned no tasks; map the repo and identify concrete work items.",
        ),
        (
            "Stabilize validation loop",
            "Audit failing validations and convert findings into prioritized remediation tasks.",
        ),
        (
            "Rank quality risks",
            "Review quality grades and convert the top risks into actionable backlog items.",
        ),
    ];
    let count = target.max(3);
    (0..count)
        .map(|idx| {
            let (title, details) = templates[idx % templates.len()];
            NewTask {
                kind: TaskKind::QualityGap,
                title: format!("{title} for {primary_gap} #{}", idx + 1),
                details: details.to_string(),
                scope_key: primary_gap.to_string(),
                priority: Priority::P1,
                source: "seed_runner_v1_fallback".to_string(),
                related_pr: None,
                related_branch: None,
            }
        })
        .collect()
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
    use super::{fallback_seed_tasks, should_seed_backlog};

    #[test]
    fn fallback_seed_tasks_generate_multiple_unique_items() {
        let tasks = fallback_seed_tasks("agent_steering", 3);
        assert_eq!(tasks.len(), 3);
        assert_ne!(tasks[0].title, tasks[1].title);
        assert_ne!(tasks[1].title, tasks[2].title);
    }

    #[test]
    fn seeding_gate_requires_empty_backlog() {
        assert!(should_seed_backlog(true, false, 0));
        assert!(!should_seed_backlog(true, false, 1));
        assert!(!should_seed_backlog(false, false, 0));
        assert!(!should_seed_backlog(true, true, 0));
    }
}
