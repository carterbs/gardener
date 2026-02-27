use crate::backlog_store::BacklogStore;
use crate::config::AppConfig;
use crate::errors::GardenerError;
use crate::logging::structured_fallback_line;
use crate::runtime::ProductionRuntime;
use crate::scheduler::{SchedulerEngine, SchedulerRunSummary};
use crate::startup::refresh_quality_report;
use crate::tui::{render_dashboard, render_report_view, BacklogView, QueueStats, WorkerRow};
use crate::runtime::Terminal;
use crate::types::RuntimeScope;
use crate::worker::execute_task;

pub fn run_worker_pool_stub(
    store: &BacklogStore,
    parallelism: usize,
    target: usize,
) -> Result<SchedulerRunSummary, GardenerError> {
    let mut scheduler = SchedulerEngine::new();
    scheduler.run_stub_complete(store, parallelism, target)
}

pub fn run_worker_pool_fsm(
    runtime: &ProductionRuntime,
    scope: &RuntimeScope,
    cfg: &AppConfig,
    store: &BacklogStore,
    terminal: &dyn Terminal,
    target: usize,
    task_override: Option<&str>,
) -> Result<usize, GardenerError> {
    let mut report_visible = false;
    let parallelism = cfg.orchestrator.parallelism.max(1) as usize;
    let mut workers = (0..parallelism)
        .map(|idx| WorkerRow {
            worker_id: format!("worker-{}", idx + 1),
            state: "idle".to_string(),
            task_title: "idle".to_string(),
            tool_line: "waiting for claim".to_string(),
            breadcrumb: "idle".to_string(),
            last_heartbeat_secs: 0,
            session_age_secs: 0,
            lease_held: false,
            session_missing: false,
        })
        .collect::<Vec<_>>();
    let mut completed = 0usize;
    render(terminal, &workers, &dashboard_snapshot(store)?)?;

    while completed < target {
        handle_hotkeys(
            runtime,
            scope,
            cfg,
            terminal,
            &mut report_visible,
        )?;
        if report_visible {
            continue;
        }
        let mut claimed_any = false;
        for idx in 0..workers.len() {
            handle_hotkeys(
                runtime,
                scope,
                cfg,
                terminal,
                &mut report_visible,
            )?;
            if report_visible {
                break;
            }
            if completed >= target {
                break;
            }

            let worker_id = workers[idx].worker_id.clone();
            let claimed = store.claim_next(
                &worker_id,
                cfg.scheduler.lease_timeout_seconds as i64,
            )?;
            let Some(task) = claimed else {
                workers[idx].state = "idle".to_string();
                workers[idx].task_title = "idle".to_string();
                workers[idx].tool_line = "waiting for claim".to_string();
                workers[idx].lease_held = false;
                continue;
            };
            claimed_any = true;
            let _ = store.mark_in_progress(&task.task_id, &worker_id)?;

            workers[idx].state = "doing".to_string();
            workers[idx].task_title = task.title.clone();
            workers[idx].tool_line = "claimed".to_string();
            workers[idx].breadcrumb = "claim>doing".to_string();
            workers[idx].lease_held = true;
            render(terminal, &workers, &dashboard_snapshot(store)?)?;

            let summary = execute_task(
                cfg,
                &worker_id,
                task_override.unwrap_or(task.title.as_str()),
            )?;
            for event in summary.logs {
                workers[idx].state = event.state.as_str().to_string();
                workers[idx].tool_line = format!("prompt {}", event.prompt_version);
                workers[idx].breadcrumb = format!("state>{}", event.state.as_str());
                render(terminal, &workers, &dashboard_snapshot(store)?)?;
            }

            if summary.final_state == crate::types::WorkerState::Complete {
                let _ = store.mark_complete(&task.task_id, &worker_id)?;
                completed = completed.saturating_add(1);
                workers[idx].state = "complete".to_string();
                workers[idx].tool_line = format!("completed {}", task.task_id);
                workers[idx].lease_held = false;
            } else {
                workers[idx].state = "failed".to_string();
                workers[idx].tool_line = format!("failed {}", task.task_id);
            }
            render(terminal, &workers, &dashboard_snapshot(store)?)?;
        }

        if !claimed_any {
            break;
        }
    }
    Ok(completed)
}

fn handle_hotkeys(
    runtime: &ProductionRuntime,
    scope: &RuntimeScope,
    cfg: &AppConfig,
    terminal: &dyn Terminal,
    report_visible: &mut bool,
) -> Result<(), GardenerError> {
    if !terminal.stdin_is_tty() {
        return Ok(());
    }
    if let Some(key) = terminal.poll_key(10)? {
        match key {
            'v' => *report_visible = true,
            'g' => {
                let _ = refresh_quality_report(runtime, cfg, scope, true)?;
                *report_visible = true;
            }
            'b' => *report_visible = false,
            _ => {}
        }
    }
    if *report_visible {
        let report_path = quality_report_path(cfg, scope);
        let report = if runtime.file_system.exists(&report_path) {
            runtime.file_system.read_to_string(&report_path)?
        } else {
            "report not found".to_string()
        };
        let frame = render_report_view(&report_path.display().to_string(), &report, 120, 30);
        terminal.draw(&frame)?;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DashboardSnapshot {
    stats: QueueStats,
    backlog: BacklogView,
}

fn dashboard_snapshot(store: &BacklogStore) -> Result<DashboardSnapshot, GardenerError> {
    let tasks = store.list_tasks()?;
    let mut stats = QueueStats {
        ready: 0,
        active: 0,
        failed: 0,
        p0: 0,
        p1: 0,
        p2: 0,
    };
    let mut backlog = BacklogView::default();
    for task in tasks {
        match task.status {
            crate::backlog_store::TaskStatus::Ready => stats.ready += 1,
            crate::backlog_store::TaskStatus::Leased | crate::backlog_store::TaskStatus::InProgress => {
                stats.active += 1;
                backlog.in_progress.push(format!(
                    "INP {} {} {}",
                    task.priority.as_str(),
                    short_task_id(&task.task_id),
                    task.title
                ));
            }
            crate::backlog_store::TaskStatus::Failed => stats.failed += 1,
            crate::backlog_store::TaskStatus::Complete => {}
        }
        if matches!(task.status, crate::backlog_store::TaskStatus::Ready) {
            backlog.queued.push(format!(
                "Q {} {} {}",
                task.priority.as_str(),
                short_task_id(&task.task_id),
                task.title
            ));
        }
        match task.priority {
            crate::priority::Priority::P0 => stats.p0 += 1,
            crate::priority::Priority::P1 => stats.p1 += 1,
            crate::priority::Priority::P2 => stats.p2 += 1,
        }
    }
    Ok(DashboardSnapshot { stats, backlog })
}

fn render(
    terminal: &dyn Terminal,
    workers: &[WorkerRow],
    snapshot: &DashboardSnapshot,
) -> Result<(), GardenerError> {
    if terminal.stdin_is_tty() {
        let frame = render_dashboard(workers, &snapshot.stats, &snapshot.backlog, 120, 30);
        terminal.draw(&frame)?;
    } else {
        for row in workers {
            terminal.write_line(&structured_fallback_line(
                &row.worker_id,
                &row.state,
                &row.tool_line,
            ))?;
        }
    }
    Ok(())
}

fn short_task_id(task_id: &str) -> &str {
    task_id.get(0..6).unwrap_or(task_id)
}

fn quality_report_path(cfg: &AppConfig, scope: &RuntimeScope) -> std::path::PathBuf {
    let path = std::path::PathBuf::from(&cfg.quality_report.path);
    if path.is_absolute() {
        path
    } else {
        scope.working_dir.join(path)
    }
}
