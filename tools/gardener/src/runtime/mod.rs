use crate::errors::GardenerError;
use crate::logging::append_run_log;
use crate::tui::{
    close_live_terminal, draw_dashboard_live, draw_report_live, draw_triage_live, render_dashboard,
    render_triage, BacklogView, QueueStats, WorkerRow,
};
use serde_json::json;
use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::SystemTime;

const RESIZE_SENTINEL_KEY: char = '\0';

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessRequest {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;
    fn sleep_until(&self, deadline: SystemTime) -> Result<(), GardenerError>;
}

pub trait ProcessRunner: Send + Sync {
    fn spawn(&self, request: ProcessRequest) -> Result<u64, GardenerError>;
    fn wait(&self, handle: u64) -> Result<ProcessOutput, GardenerError>;
    fn kill(&self, handle: u64) -> Result<(), GardenerError>;
    fn wait_with_line_stream(
        &self,
        handle: u64,
        on_stdout_line: &mut dyn FnMut(&str),
        on_stderr_line: &mut dyn FnMut(&str),
    ) -> Result<ProcessOutput, GardenerError> {
        let output = self.wait(handle)?;
        for line in output.stdout.lines() {
            on_stdout_line(line);
        }
        for line in output.stderr.lines() {
            on_stderr_line(line);
        }
        Ok(output)
    }

    fn run(&self, request: ProcessRequest) -> Result<ProcessOutput, GardenerError> {
        let handle = self.spawn(request)?;
        self.wait(handle)
    }
}

pub trait FileSystem: Send + Sync {
    fn read_to_string(&self, path: &Path) -> Result<String, GardenerError>;
    fn write_string(&self, path: &Path, contents: &str) -> Result<(), GardenerError>;
    fn create_dir_all(&self, path: &Path) -> Result<(), GardenerError>;
    fn remove_file(&self, path: &Path) -> Result<(), GardenerError>;
    fn exists(&self, path: &Path) -> bool;
}

pub trait Terminal: Send + Sync {
    fn stdin_is_tty(&self) -> bool;
    fn write_line(&self, line: &str) -> Result<(), GardenerError>;
    fn draw(&self, frame: &str) -> Result<(), GardenerError>;
    fn draw_dashboard(
        &self,
        workers: &[WorkerRow],
        stats: &QueueStats,
        backlog: &BacklogView,
    ) -> Result<(), GardenerError> {
        self.draw_dashboard_with_config(workers, stats, backlog, 15, 900)
    }
    fn draw_dashboard_with_config(
        &self,
        workers: &[WorkerRow],
        stats: &QueueStats,
        backlog: &BacklogView,
        heartbeat_interval_seconds: u64,
        lease_timeout_seconds: u64,
    ) -> Result<(), GardenerError> {
        let frame = render_dashboard(workers, stats, backlog, 120, 30);
        let _ = (heartbeat_interval_seconds, lease_timeout_seconds);
        self.draw(&frame)
    }
    fn draw_report(&self, report_path: &str, report: &str) -> Result<(), GardenerError> {
        let frame = crate::tui::render_report_view(report_path, report, 120, 30);
        self.draw(&frame)
    }
    fn draw_triage(&self, activity: &[String], artifacts: &[String]) -> Result<(), GardenerError> {
        let frame = render_triage(activity, artifacts, 120, 30);
        self.draw(&frame)
    }
    fn draw_shutdown_screen(&self, title: &str, message: &str) -> Result<(), GardenerError> {
        self.write_line(&format!("{title}: {message}"))
    }
    fn close_ui(&self) -> Result<(), GardenerError> {
        Ok(())
    }
    fn poll_key(&self, _timeout_ms: u64) -> Result<Option<char>, GardenerError> {
        Ok(None)
    }
}

static INTERRUPT_REQUESTED: AtomicBool = AtomicBool::new(false);
pub static KEY_LISTENER_ACTIVE: AtomicBool = AtomicBool::new(false);
static KEY_QUEUE: OnceLock<Mutex<VecDeque<char>>> = OnceLock::new();
static KEY_LISTENER: OnceLock<Mutex<Option<KeyListenerState>>> = OnceLock::new();

struct KeyListenerState {
    stop: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

fn key_queue() -> &'static Mutex<VecDeque<char>> {
    KEY_QUEUE.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn key_listener_slot() -> &'static Mutex<Option<KeyListenerState>> {
    KEY_LISTENER.get_or_init(|| Mutex::new(None))
}

fn enqueue_key(key: char) {
    key_queue().lock().expect("key queue lock").push_back(key);
}

fn dequeue_key() -> Option<char> {
    key_queue().lock().expect("key queue lock").pop_front()
}

fn clear_key_queue() {
    key_queue().lock().expect("key queue lock").clear();
}

fn start_key_listener_if_needed() {
    let mut slot = key_listener_slot().lock().expect("key listener slot lock");
    if slot.is_some() {
        return;
    }

    append_run_log("debug", "runtime.key_listener.started", json!({}));
    clear_key_queue();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_thread = Arc::clone(&stop);
    KEY_LISTENER_ACTIVE.store(true, Ordering::SeqCst);
    let handle = std::thread::spawn(move || {
        while !stop_for_thread.load(Ordering::SeqCst) {
            let polled = match crossterm::event::poll(std::time::Duration::from_millis(50)) {
                Ok(value) => value,
                Err(_) => continue,
            };
            if !polled {
                continue;
            }
            let Ok(event) = crossterm::event::read() else {
                continue;
            };
            match event {
                crossterm::event::Event::Key(key) => match key.code {
                    crossterm::event::KeyCode::Char('c')
                        if key
                            .modifiers
                            .contains(crossterm::event::KeyModifiers::CONTROL) =>
                    {
                        enqueue_key('q');
                        request_interrupt();
                    }
                    crossterm::event::KeyCode::Char(c) => {
                        enqueue_key(c);
                        if c == 'q' {
                            request_interrupt();
                        }
                    }
                    _ => {}
                },
                crossterm::event::Event::Resize(_, _) => {
                    enqueue_key(RESIZE_SENTINEL_KEY);
                }
                _ => {}
            }
        }
    });
    *slot = Some(KeyListenerState { stop, handle });
}

fn stop_key_listener() {
    let mut slot = key_listener_slot().lock().expect("key listener slot lock");
    let Some(state) = slot.take() else {
        return;
    };
    state.stop.store(true, Ordering::SeqCst);
    let _ = state.handle.join();
    clear_key_queue();
    KEY_LISTENER_ACTIVE.store(false, Ordering::SeqCst);
    append_run_log("debug", "runtime.key_listener.stopped", json!({}));
}

pub fn request_interrupt() {
    INTERRUPT_REQUESTED.store(true, Ordering::SeqCst);
}

pub fn clear_interrupt() {
    INTERRUPT_REQUESTED.store(false, Ordering::SeqCst);
}

pub struct ProductionClock;

impl Clock for ProductionClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }

    fn sleep_until(&self, deadline: SystemTime) -> Result<(), GardenerError> {
        let now = SystemTime::now();
        if let Ok(duration) = deadline.duration_since(now) {
            std::thread::sleep(duration);
        }
        Ok(())
    }
}

pub struct ProductionFileSystem;

impl FileSystem for ProductionFileSystem {
    fn read_to_string(&self, path: &Path) -> Result<String, GardenerError> {
        std::fs::read_to_string(path).map_err(|e| GardenerError::Io(e.to_string()))
    }

    fn write_string(&self, path: &Path, contents: &str) -> Result<(), GardenerError> {
        let result = std::fs::write(path, contents).map_err(|e| GardenerError::Io(e.to_string()));
        match &result {
            Ok(()) => append_run_log(
                "debug",
                "runtime.fs.write",
                json!({
                    "path": path.display().to_string(),
                    "bytes": contents.len()
                }),
            ),
            Err(e) => append_run_log(
                "error",
                "runtime.fs.write_error",
                json!({
                    "path": path.display().to_string(),
                    "error": e.to_string()
                }),
            ),
        }
        result
    }

    fn create_dir_all(&self, path: &Path) -> Result<(), GardenerError> {
        std::fs::create_dir_all(path).map_err(|e| GardenerError::Io(e.to_string()))
    }

    fn remove_file(&self, path: &Path) -> Result<(), GardenerError> {
        let result = std::fs::remove_file(path).map_err(|e| GardenerError::Io(e.to_string()));
        match &result {
            Ok(()) => append_run_log(
                "debug",
                "runtime.fs.remove",
                json!({
                    "path": path.display().to_string()
                }),
            ),
            Err(e) => append_run_log(
                "warn",
                "runtime.fs.remove_error",
                json!({
                    "path": path.display().to_string(),
                    "error": e.to_string()
                }),
            ),
        }
        result
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
}

#[derive(Default)]
struct ProcessState {
    next_handle: u64,
    children: HashMap<u64, std::process::Child>,
}

pub struct ProductionProcessRunner {
    state: Mutex<ProcessState>,
}

impl ProductionProcessRunner {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(ProcessState::default()),
        }
    }
}

impl Default for ProductionProcessRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessRunner for ProductionProcessRunner {
    fn spawn(&self, request: ProcessRequest) -> Result<u64, GardenerError> {
        let mut cmd = std::process::Command::new(&request.program);
        cmd.args(&request.args);
        if let Some(cwd) = &request.cwd {
            cmd.current_dir(cwd);
        }
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let child = cmd
            .spawn()
            .map_err(|e| GardenerError::Process(e.to_string()))?;
        let mut state = self.state.lock().expect("process lock poisoned");
        let handle = state.next_handle;
        state.next_handle += 1;
        state.children.insert(handle, child);
        append_run_log(
            "info",
            "process.spawn",
            json!({
                "handle": handle,
                "program": request.program,
                "args": request.args,
                "cwd": request.cwd.map(|p| p.display().to_string())
            }),
        );
        Ok(handle)
    }

    fn wait(&self, handle: u64) -> Result<ProcessOutput, GardenerError> {
        self.wait_with_line_stream(handle, &mut |_line| {}, &mut |_line| {})
    }

    fn wait_with_line_stream(
        &self,
        handle: u64,
        on_stdout_line: &mut dyn FnMut(&str),
        on_stderr_line: &mut dyn FnMut(&str),
    ) -> Result<ProcessOutput, GardenerError> {
        let child = {
            let mut state = self.state.lock().expect("process lock poisoned");
            state.children.remove(&handle)
        };
        let mut child =
            child.ok_or_else(|| GardenerError::Process(format!("unknown handle {handle}")))?;

        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| GardenerError::Process("child stdout unavailable".to_string()))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| GardenerError::Process("child stderr unavailable".to_string()))?;

        enum StreamChunk {
            Stdout(Vec<u8>),
            Stderr(Vec<u8>),
            StdoutDone,
            StderrDone,
            StdoutReadErr(String),
            StderrReadErr(String),
        }

        let (tx, rx) = std::sync::mpsc::channel::<StreamChunk>();
        let tx_out = tx.clone();
        let out_thread = std::thread::spawn(move || {
            let mut buf = [0_u8; 4096];
            loop {
                match stdout.read(&mut buf) {
                    Ok(0) => {
                        let _ = tx_out.send(StreamChunk::StdoutDone);
                        break;
                    }
                    Ok(n) => {
                        let _ = tx_out.send(StreamChunk::Stdout(buf[..n].to_vec()));
                    }
                    Err(e) => {
                        let _ = tx_out.send(StreamChunk::StdoutReadErr(e.to_string()));
                        break;
                    }
                }
            }
        });
        let err_thread = std::thread::spawn(move || {
            let mut buf = [0_u8; 4096];
            loop {
                match stderr.read(&mut buf) {
                    Ok(0) => {
                        let _ = tx.send(StreamChunk::StderrDone);
                        break;
                    }
                    Ok(n) => {
                        let _ = tx.send(StreamChunk::Stderr(buf[..n].to_vec()));
                    }
                    Err(e) => {
                        let _ = tx.send(StreamChunk::StderrReadErr(e.to_string()));
                        break;
                    }
                }
            }
        });

        let mut stdout_bytes = Vec::new();
        let mut stderr_bytes = Vec::new();
        let mut stdout_line_buffer = Vec::new();
        let mut stderr_line_buffer = Vec::new();
        let mut stdout_closed = false;
        let mut stderr_closed = false;
        let mut exit_code: Option<i32> = None;

        loop {
            if INTERRUPT_REQUESTED.swap(false, Ordering::SeqCst) {
                let _ = child.kill();
                let _ = child.wait();
                append_run_log(
                    "warn",
                    "process.interrupt",
                    json!({
                        "handle": handle,
                        "reason": "user interrupt requested (q/Ctrl-C)"
                    }),
                );
                return Err(GardenerError::Process(
                    "user interrupt requested (q/Ctrl-C)".to_string(),
                ));
            }

            match child.try_wait() {
                Ok(Some(status)) => {
                    exit_code = Some(status.code().unwrap_or(-1));
                }
                Ok(None) => {
                    // Keep draining output streams while the process is still running.
                }
                Err(e) => {
                    append_run_log(
                        "error",
                        "process.wait.error",
                        json!({
                            "handle": handle,
                            "error": e.to_string()
                        }),
                    );
                    return Err(GardenerError::Process(e.to_string()));
                }
            }

            match rx.recv_timeout(std::time::Duration::from_millis(25)) {
                Ok(StreamChunk::Stdout(chunk)) => {
                    stdout_bytes.extend_from_slice(&chunk);
                    append_and_flush_lines(&mut stdout_line_buffer, &chunk, on_stdout_line);
                }
                Ok(StreamChunk::Stderr(chunk)) => {
                    stderr_bytes.extend_from_slice(&chunk);
                    append_and_flush_lines(&mut stderr_line_buffer, &chunk, on_stderr_line);
                }
                Ok(StreamChunk::StdoutDone) => {
                    stdout_closed = true;
                }
                Ok(StreamChunk::StderrDone) => {
                    stderr_closed = true;
                }
                Ok(StreamChunk::StdoutReadErr(err)) => {
                    append_run_log(
                        "error",
                        "process.stdout.read_error",
                        json!({
                            "handle": handle,
                            "error": err
                        }),
                    );
                    return Err(GardenerError::Process(format!(
                        "stdout stream read failed: {err}"
                    )));
                }
                Ok(StreamChunk::StderrReadErr(err)) => {
                    append_run_log(
                        "error",
                        "process.stderr.read_error",
                        json!({
                            "handle": handle,
                            "error": err
                        }),
                    );
                    return Err(GardenerError::Process(format!(
                        "stderr stream read failed: {err}"
                    )));
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    break;
                }
            }

            if exit_code.is_some() && stdout_closed && stderr_closed {
                break;
            }
        }

        let waited_exit_code = child.wait().ok().map(|status| status.code().unwrap_or(-1));
        let _ = out_thread.join();
        let _ = err_thread.join();

        flush_trailing_line(&mut stdout_line_buffer, on_stdout_line);
        flush_trailing_line(&mut stderr_line_buffer, on_stderr_line);

        let output = ProcessOutput {
            exit_code: exit_code.or(waited_exit_code).unwrap_or(-1),
            stdout: String::from_utf8_lossy(&stdout_bytes).to_string(),
            stderr: String::from_utf8_lossy(&stderr_bytes).to_string(),
        };
        append_run_log(
            if output.exit_code == 0 {
                "info"
            } else {
                "error"
            },
            "process.exit",
            json!({
                "handle": handle,
                "exit_code": output.exit_code,
                "stdout_bytes": output.stdout.len(),
                "stderr_bytes": output.stderr.len()
            }),
        );
        Ok(output)
    }

    fn kill(&self, handle: u64) -> Result<(), GardenerError> {
        let mut child = {
            let mut state = self.state.lock().expect("process lock poisoned");
            state.children.remove(&handle)
        }
        .ok_or_else(|| GardenerError::Process(format!("unknown handle {handle}")))?;

        child
            .kill()
            .map_err(|e| GardenerError::Process(e.to_string()))?;
        append_run_log(
            "warn",
            "process.kill",
            json!({
                "handle": handle
            }),
        );
        Ok(())
    }
}

fn append_and_flush_lines(line_buffer: &mut Vec<u8>, chunk: &[u8], on_line: &mut dyn FnMut(&str)) {
    line_buffer.extend_from_slice(chunk);
    let mut cursor = 0usize;
    while let Some(pos) = line_buffer[cursor..].iter().position(|b| *b == b'\n') {
        let end = cursor + pos;
        let line = String::from_utf8_lossy(&line_buffer[..end]);
        on_line(line.trim_end_matches('\r'));
        cursor = end + 1;
    }
    if cursor > 0 {
        line_buffer.drain(..cursor);
    }
}

fn flush_trailing_line(line_buffer: &mut Vec<u8>, on_line: &mut dyn FnMut(&str)) {
    if line_buffer.is_empty() {
        return;
    }
    let line = String::from_utf8_lossy(line_buffer);
    on_line(line.trim_end_matches('\r'));
    line_buffer.clear();
}

pub struct ProductionTerminal;

impl Terminal for ProductionTerminal {
    fn stdin_is_tty(&self) -> bool {
        if std::env::var("GARDENER_FORCE_TTY")
            .map(|v| v == "1")
            .unwrap_or(false)
        {
            return true;
        }
        std::io::IsTerminal::is_terminal(&std::io::stdin())
    }

    fn write_line(&self, line: &str) -> Result<(), GardenerError> {
        use std::io::Write;
        append_run_log(
            "info",
            "terminal.line",
            json!({
                "line": line
            }),
        );
        let mut out = std::io::stdout();
        writeln!(out, "{line}").map_err(|e| GardenerError::Io(e.to_string()))
    }

    fn draw(&self, frame: &str) -> Result<(), GardenerError> {
        self.write_line(frame)
    }

    fn draw_dashboard_with_config(
        &self,
        workers: &[WorkerRow],
        stats: &QueueStats,
        backlog: &BacklogView,
        heartbeat_interval_seconds: u64,
        lease_timeout_seconds: u64,
    ) -> Result<(), GardenerError> {
        start_key_listener_if_needed();
        draw_dashboard_live(
            workers,
            stats,
            backlog,
            heartbeat_interval_seconds,
            lease_timeout_seconds,
        )
    }

    fn draw_report(&self, report_path: &str, report: &str) -> Result<(), GardenerError> {
        start_key_listener_if_needed();
        draw_report_live(report_path, report)
    }

    fn draw_triage(&self, activity: &[String], artifacts: &[String]) -> Result<(), GardenerError> {
        start_key_listener_if_needed();
        draw_triage_live(activity, artifacts)
    }

    fn draw_shutdown_screen(&self, title: &str, message: &str) -> Result<(), GardenerError> {
        start_key_listener_if_needed();
        crate::tui::draw_shutdown_screen_live(title, message)
    }

    fn close_ui(&self) -> Result<(), GardenerError> {
        stop_key_listener();
        close_live_terminal()
    }

    fn poll_key(&self, timeout_ms: u64) -> Result<Option<char>, GardenerError> {
        if !self.stdin_is_tty() {
            return Ok(None);
        }
        if KEY_LISTENER_ACTIVE.load(Ordering::SeqCst) {
            if timeout_ms == 0 {
                return Ok(dequeue_key());
            }
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
            loop {
                if let Some(key) = dequeue_key() {
                    return Ok(Some(key));
                }
                if std::time::Instant::now() >= deadline {
                    return Ok(None);
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
        let polled = crossterm::event::poll(std::time::Duration::from_millis(timeout_ms))
            .map_err(|e| GardenerError::Io(e.to_string()))?;
        if !polled {
            return Ok(None);
        }
        match crossterm::event::read().map_err(|e| GardenerError::Io(e.to_string()))? {
            crossterm::event::Event::Key(key) => match key.code {
                crossterm::event::KeyCode::Char('c')
                    if key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::CONTROL) =>
                {
                    Ok(Some('q'))
                }
                crossterm::event::KeyCode::Char(c) => Ok(Some(c)),
                _ => Ok(None),
            },
            crossterm::event::Event::Resize(_, _) => Ok(Some(RESIZE_SENTINEL_KEY)),
            _ => Ok(None),
        }
    }
}

pub struct ProductionRuntime {
    pub clock: Arc<dyn Clock>,
    pub file_system: Arc<dyn FileSystem>,
    pub process_runner: Arc<dyn ProcessRunner>,
    pub terminal: Arc<dyn Terminal>,
}

impl ProductionRuntime {
    pub fn new() -> Self {
        append_run_log("info", "runtime.initialized", json!({}));
        Self {
            clock: Arc::new(ProductionClock),
            file_system: Arc::new(ProductionFileSystem),
            process_runner: Arc::new(ProductionProcessRunner::new()),
            terminal: Arc::new(ProductionTerminal),
        }
    }
}

impl Default for ProductionRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
pub struct FakeClock {
    now: Arc<Mutex<SystemTime>>,
    sleeps: Arc<Mutex<Vec<SystemTime>>>,
}

impl FakeClock {
    pub fn new(now: SystemTime) -> Self {
        Self {
            now: Arc::new(Mutex::new(now)),
            sleeps: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn sleeps(&self) -> Vec<SystemTime> {
        self.sleeps.lock().expect("sleep lock").clone()
    }
}

impl Default for FakeClock {
    fn default() -> Self {
        Self::new(SystemTime::UNIX_EPOCH)
    }
}

impl Clock for FakeClock {
    fn now(&self) -> SystemTime {
        *self.now.lock().expect("clock lock")
    }

    fn sleep_until(&self, deadline: SystemTime) -> Result<(), GardenerError> {
        self.sleeps.lock().expect("sleep lock").push(deadline);
        *self.now.lock().expect("clock lock") = deadline;
        Ok(())
    }
}

#[derive(Default, Clone)]
pub struct FakeFileSystem {
    files: Arc<Mutex<HashMap<PathBuf, String>>>,
    dirs: Arc<Mutex<Vec<PathBuf>>>,
    fail_next: Arc<Mutex<Option<GardenerError>>>,
}

impl FakeFileSystem {
    pub fn with_file(path: impl Into<PathBuf>, contents: impl Into<String>) -> Self {
        let mut map = HashMap::new();
        map.insert(path.into(), contents.into());
        Self {
            files: Arc::new(Mutex::new(map)),
            dirs: Arc::new(Mutex::new(Vec::new())),
            fail_next: Arc::new(Mutex::new(None)),
        }
    }

    pub fn set_fail_next(&self, error: GardenerError) {
        *self.fail_next.lock().expect("fail lock") = Some(error);
    }

    fn maybe_fail(&self) -> Result<(), GardenerError> {
        if let Some(err) = self.fail_next.lock().expect("fail lock").take() {
            return Err(err);
        }
        Ok(())
    }
}

impl FileSystem for FakeFileSystem {
    fn read_to_string(&self, path: &Path) -> Result<String, GardenerError> {
        self.maybe_fail()?;
        self.files
            .lock()
            .expect("files lock")
            .get(path)
            .cloned()
            .ok_or_else(|| GardenerError::Io(format!("missing file {}", path.display())))
    }

    fn write_string(&self, path: &Path, contents: &str) -> Result<(), GardenerError> {
        self.maybe_fail()?;
        self.files
            .lock()
            .expect("files lock")
            .insert(path.to_path_buf(), contents.to_string());
        Ok(())
    }

    fn create_dir_all(&self, path: &Path) -> Result<(), GardenerError> {
        self.maybe_fail()?;
        self.dirs
            .lock()
            .expect("dirs lock")
            .push(path.to_path_buf());
        Ok(())
    }

    fn remove_file(&self, path: &Path) -> Result<(), GardenerError> {
        self.maybe_fail()?;
        self.files.lock().expect("files lock").remove(path);
        Ok(())
    }

    fn exists(&self, path: &Path) -> bool {
        self.files.lock().expect("files lock").contains_key(path)
    }
}

#[derive(Default, Clone)]
pub struct FakeTerminal {
    pub is_tty: bool,
    writes: Arc<Mutex<Vec<String>>>,
    draws: Arc<Mutex<Vec<String>>>,
    dashboard_draws: Arc<Mutex<usize>>,
    report_draws: Arc<Mutex<Vec<(String, String)>>>,
    shutdown_screens: Arc<Mutex<Vec<(String, String)>>>,
    key_queue: Arc<Mutex<Vec<char>>>,
}

impl FakeTerminal {
    pub fn new(is_tty: bool) -> Self {
        Self {
            is_tty,
            ..Self::default()
        }
    }

    pub fn written_lines(&self) -> Vec<String> {
        self.writes.lock().expect("writes lock").clone()
    }

    pub fn drawn_frames(&self) -> Vec<String> {
        self.draws.lock().expect("draw lock").clone()
    }

    pub fn dashboard_draw_count(&self) -> usize {
        *self.dashboard_draws.lock().expect("dashboard draw lock")
    }

    pub fn report_draws(&self) -> Vec<(String, String)> {
        self.report_draws.lock().expect("report draw lock").clone()
    }

    pub fn enqueue_keys(&self, keys: impl IntoIterator<Item = char>) {
        self.key_queue.lock().expect("key lock").extend(keys);
    }

    pub fn shutdown_screens(&self) -> Vec<(String, String)> {
        self.shutdown_screens.lock().expect("shutdown lock").clone()
    }
}

impl Terminal for FakeTerminal {
    fn stdin_is_tty(&self) -> bool {
        self.is_tty
    }

    fn write_line(&self, line: &str) -> Result<(), GardenerError> {
        self.writes
            .lock()
            .expect("writes lock")
            .push(line.to_string());
        Ok(())
    }

    fn draw(&self, frame: &str) -> Result<(), GardenerError> {
        self.draws
            .lock()
            .expect("draw lock")
            .push(frame.to_string());
        Ok(())
    }

    fn draw_dashboard(
        &self,
        workers: &[WorkerRow],
        stats: &QueueStats,
        backlog: &BacklogView,
    ) -> Result<(), GardenerError> {
        let frame = render_dashboard(workers, stats, backlog, 120, 30);
        self.draw(&frame)?;
        let mut count = self.dashboard_draws.lock().expect("dashboard draw lock");
        *count = count.saturating_add(1);
        Ok(())
    }

    fn draw_report(&self, report_path: &str, report: &str) -> Result<(), GardenerError> {
        self.report_draws
            .lock()
            .expect("report draw lock")
            .push((report_path.to_string(), report.to_string()));
        let frame = crate::tui::render_report_view(report_path, report, 120, 30);
        self.draw(&frame)
    }

    fn draw_shutdown_screen(&self, title: &str, message: &str) -> Result<(), GardenerError> {
        self.shutdown_screens
            .lock()
            .expect("shutdown lock")
            .push((title.to_string(), message.to_string()));
        Ok(())
    }

    fn poll_key(&self, _timeout_ms: u64) -> Result<Option<char>, GardenerError> {
        if !self.is_tty {
            return Ok(None);
        }
        let mut queue = self.key_queue.lock().expect("key lock");
        if queue.is_empty() {
            return Ok(None);
        }
        Ok(Some(queue.remove(0)))
    }
}

#[derive(Default, Clone)]
pub struct FakeProcessRunner {
    responses: Arc<Mutex<Vec<Result<ProcessOutput, GardenerError>>>>,
    spawned: Arc<Mutex<Vec<ProcessRequest>>>,
    waits: Arc<Mutex<Vec<u64>>>,
    kills: Arc<Mutex<Vec<u64>>>,
    next_handle: Arc<Mutex<u64>>,
}

impl FakeProcessRunner {
    pub fn push_response(&self, output: Result<ProcessOutput, GardenerError>) {
        self.responses.lock().expect("responses lock").push(output);
    }

    pub fn spawned(&self) -> Vec<ProcessRequest> {
        self.spawned.lock().expect("spawned lock").clone()
    }

    pub fn waits(&self) -> Vec<u64> {
        self.waits.lock().expect("waits lock").clone()
    }

    pub fn kills(&self) -> Vec<u64> {
        self.kills.lock().expect("kills lock").clone()
    }
}

impl ProcessRunner for FakeProcessRunner {
    fn spawn(&self, request: ProcessRequest) -> Result<u64, GardenerError> {
        self.spawned.lock().expect("spawned lock").push(request);
        let mut next = self.next_handle.lock().expect("next lock");
        let handle = *next;
        *next += 1;
        Ok(handle)
    }

    fn wait(&self, handle: u64) -> Result<ProcessOutput, GardenerError> {
        self.waits.lock().expect("waits lock").push(handle);
        let mut responses = self.responses.lock().expect("responses lock");
        if responses.is_empty() {
            return Err(GardenerError::Process(
                "no fake response queued".to_string(),
            ));
        }
        responses.remove(0)
    }

    fn kill(&self, handle: u64) -> Result<(), GardenerError> {
        self.kills.lock().expect("kills lock").push(handle);
        Ok(())
    }
}
