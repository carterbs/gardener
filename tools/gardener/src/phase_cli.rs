use crate::agent::factory::AdapterFactory;
use crate::config::{load_config, resolve_validation_command, AppConfig, CliOverrides};
use crate::errors::GardenerError;
use crate::gh::GhClient;
use crate::learning_loop::LearningLoop;
use crate::logging::{default_run_log_path, init_run_logger};
use crate::prompt_registry::PromptRegistry;
use crate::protocol::AgentEvent;
use crate::runtime::{ProcessRequest, ProcessRunner, ProductionProcessRunner};
use crate::types::RuntimeScope;
use crate::worker_identity::WorkerIdentity;
use crate::worktree::WorktreeClient;
use std::path::PathBuf;
use std::time::SystemTime;

pub struct PhaseRuntime {
    pub cfg: AppConfig,
    pub scope: RuntimeScope,
    pub runner: ProductionProcessRunner,
    pub factory: AdapterFactory,
    pub registry: PromptRegistry,
    pub learning_loop: LearningLoop,
    pub identity: WorkerIdentity,
    pub validation_command: String,
}

impl PhaseRuntime {
    pub fn init(binary_name: &str) -> Result<Self, GardenerError> {
        crate::logging::append_run_log("info", "phase_cli.init", serde_json::json!({ "binary": binary_name }));
        let cwd = std::env::current_dir().map_err(|e| GardenerError::Io(e.to_string()))?;
        let log_path = default_run_log_path(&cwd);
        let _run_id = init_run_logger(&log_path, &cwd);

        let runner = ProductionProcessRunner::new();

        step(binary_name, "CONFIG", "loading gardener.toml");
        let fs = crate::runtime::ProductionFileSystem;
        let overrides = CliOverrides {
            config_path: None,
            working_dir: None,
            parallelism: None,
            task: None,
            target: None,
            prune_only: false,
            backlog_only: false,
            quality_grades_only: false,
            validation_command: None,
            agent: None,
            retriage: false,
            triage_only: false,
            sync_only: false,
        };
        let (cfg, scope) = load_config(&overrides, &cwd, &fs, &runner)?;
        let validation = resolve_validation_command(&cfg, None);
        step(
            binary_name,
            "CONFIG",
            &format!(
                "working_dir={} validation={}",
                scope.working_dir.display(),
                validation.command
            ),
        );

        let factory = AdapterFactory::with_defaults();
        let registry = PromptRegistry::v1();
        let learning_loop = LearningLoop::default();
        let identity = WorkerIdentity::new(binary_name);

        Ok(Self {
            cfg,
            scope,
            runner,
            factory,
            registry,
            learning_loop,
            identity,
            validation_command: validation.command,
        })
    }
}

/// Resolve a PR number to (worktree_path, branch, pr_number).
pub fn resolve_worktree_from_pr(
    runner: &dyn ProcessRunner,
    scope: &RuntimeScope,
    pr: u64,
    binary_name: &str,
) -> Result<(PathBuf, String, u64), GardenerError> {
    step(binary_name, "LOOKUP", &format!("fetching PR #{pr} metadata"));
    let repo_gh = GhClient::new(runner, &scope.working_dir);
    let pr_view = repo_gh.view_pr(pr)?;
    let branch = pr_view.head_ref_name;
    step(binary_name, "LOOKUP", &format!("branch={branch} state={}", pr_view.state));

    if pr_view.state.eq_ignore_ascii_case("MERGED") {
        return Err(GardenerError::Process("PR is already merged".to_string()));
    }
    if pr_view.state.eq_ignore_ascii_case("CLOSED") {
        return Err(GardenerError::Process("PR is closed (not merged)".to_string()));
    }

    let worktree_path = resolve_worktree_from_branch(runner, scope, &branch, pr, binary_name)?;
    Ok((worktree_path, branch, pr))
}

/// Find or create a worktree for a given branch.
pub fn resolve_worktree_from_branch(
    runner: &dyn ProcessRunner,
    scope: &RuntimeScope,
    branch: &str,
    pr: u64,
    binary_name: &str,
) -> Result<PathBuf, GardenerError> {
    step(binary_name, "WORKTREE", &format!("looking for worktree on branch {branch}"));
    let wt_client = WorktreeClient::new(runner, &scope.working_dir);
    let worktrees = wt_client.list()?;
    let existing_wt = worktrees.iter().find(|w| w.branch.as_deref() == Some(branch));

    if let Some(wt) = existing_wt {
        step(binary_name, "WORKTREE", &format!("reusing {}", wt.path.display()));
        return Ok(wt.path.clone());
    }

    let wt_dir = scope.working_dir.join(".worktrees").join(format!("{binary_name}-{pr}"));
    step(binary_name, "WORKTREE", &format!("creating at {}", wt_dir.display()));
    let fetch_out = runner.run(ProcessRequest {
        program: "git".to_string(),
        args: vec!["fetch".to_string(), "origin".to_string(), branch.to_string()],
        cwd: Some(scope.working_dir.clone()),
    })?;
    if fetch_out.exit_code != 0 {
        return Err(GardenerError::Process(format!(
            "git fetch origin {branch} failed: {}", fetch_out.stderr
        )));
    }
    let add_out = runner.run(ProcessRequest {
        program: "git".to_string(),
        args: vec!["worktree".to_string(), "add".to_string(), wt_dir.display().to_string(), branch.to_string()],
        cwd: Some(scope.working_dir.clone()),
    })?;
    if add_out.exit_code != 0 {
        let add_out2 = runner.run(ProcessRequest {
            program: "git".to_string(),
            args: vec![
                "worktree".to_string(), "add".to_string(), wt_dir.display().to_string(),
                format!("origin/{branch}"), "-b".to_string(), branch.to_string(),
            ],
            cwd: Some(scope.working_dir.clone()),
        })?;
        if add_out2.exit_code != 0 {
            return Err(GardenerError::Process(format!(
                "git worktree add failed: {}", add_out2.stderr
            )));
        }
    }
    Ok(wt_dir)
}

pub fn ts() -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

pub fn step(binary_name: &str, label: &str, detail: &str) {
    eprintln!("[{binary_name} {}] {label}: {detail}", ts());
}

pub fn print_agent_event(binary_name: &str, event: &AgentEvent) {
    let payload = &event.payload;
    let event_type = payload.get("type").and_then(|t| t.as_str()).unwrap_or("");

    match event_type {
        // Streaming text delta — agent is writing/thinking
        "content_block_delta" => {
            if let Some(text) = payload
                .get("delta")
                .and_then(|d| d.get("text"))
                .and_then(|t| t.as_str())
            {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    for line in trimmed.lines() {
                        let line = line.trim();
                        if !line.is_empty() {
                            eprintln!("[{binary_name} {}] AGENT: {line}", ts());
                        }
                    }
                }
            }
        }
        // Standalone tool_use — agent is calling a tool
        "tool_use" => {
            let name = payload.get("name").and_then(|n| n.as_str()).unwrap_or("?");
            let command = payload
                .get("input")
                .and_then(|i| i.get("command"))
                .and_then(|c| c.as_str());
            if let Some(cmd) = command {
                eprintln!("[{binary_name} {}] TOOL: {name}: {cmd}", ts());
            } else {
                eprintln!("[{binary_name} {}] TOOL: {name}", ts());
            }
        }
        // Final result — contains message.content blocks with full output
        "result" => {
            let content = payload.get("message").and_then(|m| m.get("content"));
            let Some(blocks) = content.and_then(|c| c.as_array()) else {
                return;
            };
            for block in blocks {
                match block.get("type").and_then(|t| t.as_str()) {
                    Some("tool_use") => {
                        let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("?");
                        let command = block
                            .get("input")
                            .and_then(|i| i.get("command"))
                            .and_then(|c| c.as_str());
                        if let Some(cmd) = command {
                            eprintln!("[{binary_name} {}] TOOL: {name}: {cmd}", ts());
                        } else {
                            eprintln!("[{binary_name} {}] TOOL: {name}", ts());
                        }
                    }
                    Some("text") => {
                        if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                            let trimmed = text.trim();
                            if !trimmed.is_empty() {
                                for line in trimmed.lines().take(20) {
                                    let line = line.trim();
                                    if !line.is_empty() {
                                        eprintln!("[{binary_name} {}] AGENT: {line}", ts());
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}
