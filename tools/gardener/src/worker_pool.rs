use crate::backlog_store::BacklogStore;
use crate::config::AppConfig;
use crate::errors::GardenerError;
use crate::logging::structured_fallback_line;
use crate::scheduler::{SchedulerEngine, SchedulerRunSummary};
use crate::tui::{render_dashboard, QueueStats, WorkerRow};
use crate::runtime::Terminal;
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
    cfg: &AppConfig,
    store: &BacklogStore,
    terminal: &dyn Terminal,
    target: usize,
    task_override: Option<&str>,
) -> Result<usize, GardenerError> {
    let parallelism = cfg.orchestrator.parallelism.max(1) as usize;
    let mut workers = (0..parallelism)
        .map(|idx| WorkerRow {
            worker_id: format!("worker-{}", idx + 1),
            state: "idle".to_string(),
            tool_line: "waiting for claim".to_string(),
            breadcrumb: "idle".to_string(),
            last_heartbeat_secs: 0,
            session_age_secs: 0,
            lease_held: false,
            session_missing: false,
        })
        .collect::<Vec<_>>();
    let mut completed = 0usize;
    render(terminal, &workers, &queue_stats(store)?)?;

    while completed < target {
        let mut claimed_any = false;
        for idx in 0..workers.len() {
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
                workers[idx].tool_line = "waiting for claim".to_string();
                workers[idx].lease_held = false;
                continue;
            };
            claimed_any = true;
            let _ = store.mark_in_progress(&task.task_id, &worker_id)?;

            workers[idx].state = "doing".to_string();
            workers[idx].tool_line = task.title.clone();
            workers[idx].breadcrumb = "claim>doing".to_string();
            workers[idx].lease_held = true;
            render(terminal, &workers, &queue_stats(store)?)?;

            let summary = execute_task(
                cfg,
                &worker_id,
                task_override.unwrap_or(task.title.as_str()),
            )?;
            for event in summary.logs {
                workers[idx].state = event.state.as_str().to_string();
                workers[idx].tool_line = format!("prompt={}", event.prompt_version);
                workers[idx].breadcrumb = format!("state>{}", event.state.as_str());
                render(terminal, &workers, &queue_stats(store)?)?;
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
            render(terminal, &workers, &queue_stats(store)?)?;
        }

        if !claimed_any {
            break;
        }
    }
    Ok(completed)
}

fn queue_stats(store: &BacklogStore) -> Result<QueueStats, GardenerError> {
    let tasks = store.list_tasks()?;
    let mut stats = QueueStats {
        ready: 0,
        active: 0,
        failed: 0,
        p0: 0,
        p1: 0,
        p2: 0,
    };
    for task in tasks {
        match task.status {
            crate::backlog_store::TaskStatus::Ready => stats.ready += 1,
            crate::backlog_store::TaskStatus::Leased | crate::backlog_store::TaskStatus::InProgress => {
                stats.active += 1
            }
            crate::backlog_store::TaskStatus::Failed => stats.failed += 1,
            crate::backlog_store::TaskStatus::Complete => {}
        }
        match task.priority {
            crate::priority::Priority::P0 => stats.p0 += 1,
            crate::priority::Priority::P1 => stats.p1 += 1,
            crate::priority::Priority::P2 => stats.p2 += 1,
        }
    }
    Ok(stats)
}

fn render(
    terminal: &dyn Terminal,
    workers: &[WorkerRow],
    stats: &QueueStats,
) -> Result<(), GardenerError> {
    if terminal.stdin_is_tty() {
        let frame = render_dashboard(workers, stats, 120, 30);
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
