use crate::backlog_store::BacklogStore;
use crate::errors::GardenerError;
use crate::scheduler::{SchedulerEngine, SchedulerRunSummary};

pub fn run_worker_pool_stub(
    store: &BacklogStore,
    parallelism: usize,
    target: usize,
) -> Result<SchedulerRunSummary, GardenerError> {
    let mut scheduler = SchedulerEngine::new();
    scheduler.run_stub_complete(store, parallelism, target)
}
