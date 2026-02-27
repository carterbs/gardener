use crate::errors::GardenerError;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

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
        std::fs::write(path, contents).map_err(|e| GardenerError::Io(e.to_string()))
    }

    fn create_dir_all(&self, path: &Path) -> Result<(), GardenerError> {
        std::fs::create_dir_all(path).map_err(|e| GardenerError::Io(e.to_string()))
    }

    fn remove_file(&self, path: &Path) -> Result<(), GardenerError> {
        std::fs::remove_file(path).map_err(|e| GardenerError::Io(e.to_string()))
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
        cmd.stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let child = cmd
            .spawn()
            .map_err(|e| GardenerError::Process(e.to_string()))?;
        let mut state = self.state.lock().expect("process lock poisoned");
        let handle = state.next_handle;
        state.next_handle += 1;
        state.children.insert(handle, child);
        Ok(handle)
    }

    fn wait(&self, handle: u64) -> Result<ProcessOutput, GardenerError> {
        let child = {
            let mut state = self.state.lock().expect("process lock poisoned");
            state.children.remove(&handle)
        };
        let child =
            child.ok_or_else(|| GardenerError::Process(format!("unknown handle {handle}")))?;
        let output = child
            .wait_with_output()
            .map_err(|e| GardenerError::Process(e.to_string()))?;
        Ok(ProcessOutput {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }

    fn kill(&self, handle: u64) -> Result<(), GardenerError> {
        let mut child = {
            let mut state = self.state.lock().expect("process lock poisoned");
            state.children.remove(&handle)
        }
        .ok_or_else(|| GardenerError::Process(format!("unknown handle {handle}")))?;

        child
            .kill()
            .map_err(|e| GardenerError::Process(e.to_string()))
    }
}

pub struct ProductionTerminal;

impl Terminal for ProductionTerminal {
    fn stdin_is_tty(&self) -> bool {
        std::io::IsTerminal::is_terminal(&std::io::stdin())
    }

    fn write_line(&self, line: &str) -> Result<(), GardenerError> {
        use std::io::Write;
        let mut out = std::io::stdout();
        writeln!(out, "{line}").map_err(|e| GardenerError::Io(e.to_string()))
    }

    fn draw(&self, frame: &str) -> Result<(), GardenerError> {
        self.write_line(frame)
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
