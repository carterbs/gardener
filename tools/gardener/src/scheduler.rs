use crate::backlog_store::{BacklogStore, NewTask, TaskStatus};
use crate::errors::GardenerError;
use crate::priority::Priority;
use crate::task_identity::TaskKind;
use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkRequest {
    pub request_id: u64,
    pub worker_id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SchedulerMetrics {
    pub claim_latency_ms: u64,
    pub queue_depth_p0: usize,
    pub queue_depth_p1: usize,
    pub queue_depth_p2: usize,
    pub requeue_count: usize,
    pub starvation_watchdog_count: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SchedulerRunSummary {
    pub request_trace: Vec<u64>,
    pub assignment_trace: Vec<(u64, String)>,
    pub completed_count: usize,
    pub metrics: SchedulerMetrics,
}

pub struct SchedulerEngine {
    waiting: VecDeque<WorkRequest>,
    next_request_id: u64,
    pub metrics: SchedulerMetrics,
    last_event_id: u64,
    projection_last_event_by_task: HashMap<String, u64>,
}

impl SchedulerEngine {
    pub fn new() -> Self {
        Self {
            waiting: VecDeque::new(),
            next_request_id: 1,
            metrics: SchedulerMetrics::default(),
            last_event_id: 0,
            projection_last_event_by_task: HashMap::new(),
        }
    }

    pub fn enqueue_request(&mut self, worker_id: &str) -> u64 {
        let id = self.next_request_id;
        self.next_request_id += 1;
        self.waiting.push_back(WorkRequest {
            request_id: id,
            worker_id: worker_id.to_string(),
        });
        id
    }

    pub fn run_stub_complete(
        &mut self,
        store: &BacklogStore,
        parallelism: usize,
        target: usize,
    ) -> Result<SchedulerRunSummary, GardenerError> {
        ensure_seed_tasks(store, target)?;

        let mut summary = SchedulerRunSummary::default();
        for i in 0..parallelism {
            let rid = self.enqueue_request(&format!("worker-{}", i + 1));
            summary.request_trace.push(rid);
        }

        while summary.completed_count < target {
            let Some(request) = self.waiting.pop_front() else {
                self.metrics.starvation_watchdog_count += 1;
                break;
            };
            summary.request_trace.push(request.request_id);

            let claimed = store.claim_next(&request.worker_id, 900)?;
            let Some(task) = claimed else {
                self.metrics.starvation_watchdog_count += 1;
                break;
            };
            summary
                .assignment_trace
                .push((request.request_id, task.task_id.clone()));
            self.record_projection_event(&task.task_id);
            if task.status == TaskStatus::Leased {
                let _ = store.mark_complete(&task.task_id, &request.worker_id)?;
                summary.completed_count += 1;
            }

            let next = self.enqueue_request(&request.worker_id);
            summary.request_trace.push(next);
            refresh_queue_depth_metrics(&mut self.metrics, store)?;
        }

        summary.metrics = self.metrics.clone();
        Ok(summary)
    }

    fn record_projection_event(&mut self, task_id: &str) {
        self.last_event_id += 1;
        let entry = self
            .projection_last_event_by_task
            .entry(task_id.to_string())
            .or_insert(0);
        if self.last_event_id > *entry {
            *entry = self.last_event_id;
        }
    }
}

fn ensure_seed_tasks(store: &BacklogStore, target: usize) -> Result<(), GardenerError> {
    if !store.list_tasks()?.is_empty() {
        return Ok(());
    }
    for idx in 0..(target.saturating_mul(2).max(3)) {
        let priority = if idx == 0 {
            Priority::P0
        } else if idx % 2 == 0 {
            Priority::P1
        } else {
            Priority::P2
        };
        let _ = store.upsert_task(NewTask {
            kind: TaskKind::Maintenance,
            title: format!("scheduler-stub-task-{idx}"),
            details: "phase4".to_string(),
            scope_key: "domain:scheduler".to_string(),
            priority,
            source: "phase4-stub".to_string(),
            related_pr: None,
            related_branch: None,
        })?;
    }
    Ok(())
}

fn refresh_queue_depth_metrics(
    metrics: &mut SchedulerMetrics,
    store: &BacklogStore,
) -> Result<(), GardenerError> {
    let tasks = store.list_tasks()?;
    metrics.queue_depth_p0 = tasks
        .iter()
        .filter(|t| t.priority == Priority::P0 && t.status == TaskStatus::Ready)
        .count();
    metrics.queue_depth_p1 = tasks
        .iter()
        .filter(|t| t.priority == Priority::P1 && t.status == TaskStatus::Ready)
        .count();
    metrics.queue_depth_p2 = tasks
        .iter()
        .filter(|t| t.priority == Priority::P2 && t.status == TaskStatus::Ready)
        .count();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::SchedulerEngine;
    use crate::backlog_store::BacklogStore;

    #[test]
    fn scheduler_serves_fifo_worker_requests_and_completes_target() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = BacklogStore::open(dir.path().join("b.sqlite")).expect("store");
        let mut scheduler = SchedulerEngine::new();
        let summary = scheduler
            .run_stub_complete(&store, 3, 3)
            .expect("run scheduler");

        assert_eq!(summary.completed_count, 3);
        assert!(!summary.assignment_trace.is_empty());
        assert!(summary.assignment_trace[0].0 < summary.assignment_trace[1].0);
    }
}
