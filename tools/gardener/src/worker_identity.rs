use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionIdentity {
    pub session_id: String,
    pub sandbox_id: String,
    pub resume_from_session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerIdentity {
    pub worker_id: String,
    pub attempt: u32,
    pub session: SessionIdentity,
}

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(1);
static SANDBOX_COUNTER: AtomicU64 = AtomicU64::new(1);

impl WorkerIdentity {
    pub fn new(worker_id: impl Into<String>) -> Self {
        let mut id = Self {
            worker_id: worker_id.into(),
            attempt: 1,
            session: SessionIdentity {
                session_id: String::new(),
                sandbox_id: String::new(),
                resume_from_session_id: None,
            },
        };
        id.session = next_session(None);
        id
    }

    pub fn begin_retry(&mut self) {
        self.attempt = self.attempt.saturating_add(1);
        self.session = next_session(None);
    }
}

fn next_session(resume_from_session_id: Option<String>) -> SessionIdentity {
    let sid = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    let bid = SANDBOX_COUNTER.fetch_add(1, Ordering::Relaxed);
    SessionIdentity {
        session_id: format!("session-{sid}"),
        sandbox_id: format!("sandbox-{bid}"),
        resume_from_session_id,
    }
}

#[cfg(test)]
mod tests {
    use super::WorkerIdentity;

    #[test]
    fn retry_and_restart_create_fresh_session_identity() {
        let mut id = WorkerIdentity::new("worker-1");
        let first_session = id.session.session_id.clone();
        let worker_id = id.worker_id.clone();

        id.begin_retry();
        assert_eq!(id.worker_id, worker_id);
        assert_ne!(id.session.session_id, first_session);
        assert!(id.session.resume_from_session_id.is_none());
    }
}
