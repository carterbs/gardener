//! Global `SessionRecorder` singleton and thread-local worker ID.
//!
//! Follows the `RUN_LOGGER` / `OnceLock<Mutex<Option<…>>>` pattern from `logging.rs`.

use crate::errors::GardenerError;
use crate::replay::recording::RecordEntry;
use crate::runtime::{ProcessOutput, ProcessRequest, ProcessRunner};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

// ── Thread-local worker ID ────────────────────────────────────────────────────

thread_local! {
    static RECORDING_WORKER_ID: RefCell<String> = RefCell::new("pool".into());
}

pub fn set_recording_worker_id(id: &str) {
    RECORDING_WORKER_ID.with(|cell| *cell.borrow_mut() = id.to_string());
}

pub fn get_recording_worker_id() -> String {
    RECORDING_WORKER_ID.with(|cell| cell.borrow().clone())
}

// ── Global sequence counter ───────────────────────────────────────────────────

static RECORD_SEQ: AtomicU64 = AtomicU64::new(1);

pub fn next_seq() -> u64 {
    RECORD_SEQ.fetch_add(1, Ordering::Relaxed)
}

pub fn timestamp_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

// ── SessionRecorder ───────────────────────────────────────────────────────────

struct RecorderState {
    path: PathBuf,
    write_lock: Mutex<()>,
}

impl RecorderState {
    fn emit(&self, entry: &RecordEntry) -> Result<(), GardenerError> {
        let line =
            serde_json::to_string(entry).map_err(|e| GardenerError::Io(e.to_string()))?;
        let _guard = self.write_lock.lock().expect("recorder write lock");
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| GardenerError::Io(e.to_string()))?;
        writeln!(file, "{line}").map_err(|e| GardenerError::Io(e.to_string()))
    }
}

static SESSION_RECORDER: OnceLock<Mutex<Option<Arc<RecorderState>>>> = OnceLock::new();

fn recorder_slot() -> &'static Mutex<Option<Arc<RecorderState>>> {
    SESSION_RECORDER.get_or_init(|| Mutex::new(None))
}

/// Initialize the global session recorder to write to `path`.
pub fn init_session_recorder(path: impl AsRef<Path>) -> Result<(), GardenerError> {
    let path = path.as_ref().to_path_buf();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| GardenerError::Io(e.to_string()))?;
    }
    let state = Arc::new(RecorderState {
        path,
        write_lock: Mutex::new(()),
    });
    *recorder_slot().lock().expect("recorder init lock") = Some(state);
    Ok(())
}

/// Clear the global session recorder (call after session ends).
pub fn clear_session_recorder() {
    *recorder_slot().lock().expect("recorder clear lock") = None;
    // Reset sequence counter for clean test isolation
    RECORD_SEQ.store(1, Ordering::Relaxed);
}

/// Emit a record to the session file, if recording is active.
/// Returns `Ok(())` if not recording (no-op).
pub fn emit_record(entry: RecordEntry) {
    let guard = recorder_slot().lock().expect("recorder emit lock");
    if let Some(state) = guard.as_ref() {
        let state = Arc::clone(state);
        drop(guard); // release the mutex before doing I/O
        let _ = state.emit(&entry);
    }
}

// ── RecordingProcessRunner ────────────────────────────────────────────────────

use crate::replay::recording::{
    ProcessCallRecord, ProcessOutputRecord, ProcessRequestRecord,
};

/// Wraps any `ProcessRunner`, recording every call/response as a `ProcessCall` entry.
pub struct RecordingProcessRunner {
    inner: Arc<dyn ProcessRunner>,
    in_flight: Mutex<HashMap<u64, (ProcessRequestRecord, u64 /* start_ns */)>>,
}

impl RecordingProcessRunner {
    pub fn new(inner: Arc<dyn ProcessRunner>) -> Self {
        Self {
            inner,
            in_flight: Mutex::new(HashMap::new()),
        }
    }
}

impl ProcessRunner for RecordingProcessRunner {
    fn spawn(&self, request: ProcessRequest) -> Result<u64, GardenerError> {
        let record = ProcessRequestRecord {
            program: request.program.clone(),
            args: request.args.clone(),
            cwd: request.cwd.as_ref().map(|p| p.display().to_string()),
        };
        let handle = self.inner.spawn(request)?;
        let start_ns = timestamp_ns();
        self.in_flight
            .lock()
            .expect("in_flight lock")
            .insert(handle, (record, start_ns));
        Ok(handle)
    }

    fn wait(&self, handle: u64) -> Result<ProcessOutput, GardenerError> {
        let output = self.inner.wait(handle)?;
        self.emit_process_call(handle, &output);
        Ok(output)
    }

    fn kill(&self, handle: u64) -> Result<(), GardenerError> {
        self.inner.kill(handle)?;
        // Remove from in-flight tracking without emitting a record
        self.in_flight.lock().expect("in_flight lock").remove(&handle);
        Ok(())
    }

    fn wait_with_line_stream(
        &self,
        handle: u64,
        on_stdout_line: &mut dyn FnMut(&str),
        on_stderr_line: &mut dyn FnMut(&str),
    ) -> Result<ProcessOutput, GardenerError> {
        let output =
            self.inner
                .wait_with_line_stream(handle, on_stdout_line, on_stderr_line)?;
        self.emit_process_call(handle, &output);
        Ok(output)
    }
}

impl RecordingProcessRunner {
    fn emit_process_call(&self, handle: u64, output: &ProcessOutput) {
        let entry = {
            let mut guard = self.in_flight.lock().expect("in_flight lock");
            let Some((request, start_ns)) = guard.remove(&handle) else {
                return; // handle unknown – shouldn't happen
            };
            let now_ns = timestamp_ns();
            let duration_ns = now_ns.saturating_sub(start_ns);
            let result = ProcessOutputRecord::from_output(
                output.exit_code,
                output.stdout.clone(),
                output.stderr.clone(),
            );
            RecordEntry::ProcessCall(ProcessCallRecord {
                seq: next_seq(),
                timestamp_ns: now_ns,
                worker_id: get_recording_worker_id(),
                thread_id: format!("{:?}", std::thread::current().id()),
                request,
                result,
                duration_ns,
            })
        };
        emit_record(entry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{FakeProcessRunner, ProcessOutput};

    #[test]
    fn set_get_recording_worker_id_roundtrip() {
        set_recording_worker_id("worker-42");
        assert_eq!(get_recording_worker_id(), "worker-42");
        set_recording_worker_id("pool"); // restore default
    }

    #[test]
    fn recording_process_runner_delegates_and_records() {
        use tempfile::NamedTempFile;
        let tmp = NamedTempFile::new().expect("create temp file for recording");
        init_session_recorder(tmp.path()).expect("initialize session recorder");

        let fake = Arc::new(FakeProcessRunner::default());
        fake.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "hello".to_string(),
            stderr: String::new(),
        }));
        let runner = RecordingProcessRunner::new(fake.clone());
        let handle = runner
            .spawn(ProcessRequest {
                program: "echo".to_string(),
                args: vec!["hello".to_string()],
                cwd: None,
            })
            .expect("spawn process");
        let out = runner.wait(handle).expect("wait process output");
        assert_eq!(out.stdout, "hello");
        assert_eq!(fake.spawned().len(), 1);

        clear_session_recorder();
        let contents = std::fs::read_to_string(tmp.path()).expect("read recorder output");
        let line: serde_json::Value = serde_json::from_str(contents.trim())
            .expect("parse recorder output json");
        assert_eq!(line["type"], "process_call");
        assert_eq!(line["request"]["program"], "echo");
    }
}
