use crate::backlog_store::BacklogStore;
use crate::config::AppConfig;
use crate::errors::GardenerError;
use crate::scheduler::{SchedulerEngine, SchedulerRunSummary};
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
    target: usize,
    task: Option<&str>,
) -> Result<usize, GardenerError> {
    let mut completed = 0usize;
    for i in 0..target {
        let summary = execute_task(
            cfg,
            &format!("worker-{}", i + 1),
            task.unwrap_or("task: runtime execution"),
        )?;
        if summary.final_state == crate::types::WorkerState::Complete {
            completed = completed.saturating_add(1);
        }
    }
    Ok(completed)
}
