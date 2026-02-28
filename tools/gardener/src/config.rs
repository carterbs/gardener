use crate::errors::GardenerError;
use crate::logging::append_run_log;
use crate::runtime::{FileSystem, ProcessRequest, ProcessRunner};
use crate::types::{AgentKind, RuntimeScope, ValidationCommandResolution, WorkerState};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const DEFAULT_CONFIG_FILE: &str = "gardener.toml";

#[derive(Debug, Clone, Default)]
pub struct CliOverrides {
    pub config_path: Option<PathBuf>,
    pub working_dir: Option<PathBuf>,
    pub parallelism: Option<u32>,
    pub task: Option<String>,
    pub target: Option<u32>,
    pub prune_only: bool,
    pub backlog_only: bool,
    pub quality_grades_only: bool,
    pub validation_command: Option<String>,
    pub agent: Option<AgentKind>,
    pub retriage: bool,
    pub triage_only: bool,
    pub sync_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppConfig {
    pub orchestrator: OrchestratorConfig,
    pub scope: ScopeConfig,
    pub startup: StartupConfig,
    pub validation: ValidationConfig,
    pub agent: AgentConfig,
    pub states: BTreeMap<String, StateConfig>,
    pub scheduler: SchedulerConfig,
    pub prompts: PromptsConfig,
    pub learning: LearningConfig,
    pub seeding: SeedingConfig,
    pub execution: ExecutionConfig,
    pub triage: TriageConfig,
    pub quality_report: QualityReportConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrchestratorConfig {
    pub parallelism: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScopeConfig {
    pub working_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StartupConfig {
    pub validate_on_boot: bool,
    pub validation_command: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ValidationConfig {
    pub command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentConfig {
    pub default: Option<AgentKind>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StateConfig {
    pub backend: Option<AgentKind>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SchedulerConfig {
    pub lease_timeout_seconds: u64,
    pub heartbeat_interval_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PromptsConfig {
    pub turn_budget: TurnBudgetConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnBudgetConfig {
    pub understand: u32,
    pub planning: u32,
    pub doing: u32,
    pub gitting: u32,
    pub reviewing: u32,
    pub merging: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LearningConfig {
    pub confidence_decay_per_day: f64,
    pub deactivate_below_confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SeedingConfig {
    pub backend: AgentKind,
    pub model: String,
    pub max_turns: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum GitOutputMode {
    CommitOnly,
    Push,
    #[default]
    PullRequest,
}

impl GitOutputMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CommitOnly => "commit_only",
            Self::Push => "push",
            Self::PullRequest => "pull_request",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionConfig {
    pub permissions_mode: String,
    pub worker_mode: String,
    pub test_mode: bool,
    pub git_output_mode: GitOutputMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TriageConfig {
    pub output_path: String,
    pub stale_after_commits: u64,
    pub discovery_max_turns: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QualityReportConfig {
    pub path: String,
    pub stale_after_days: u64,
    pub stale_if_head_commit_differs: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            orchestrator: OrchestratorConfig { parallelism: 3 },
            scope: ScopeConfig { working_dir: None },
            startup: StartupConfig {
                validate_on_boot: false,
                validation_command: Some("npm run validate".to_string()),
            },
            validation: ValidationConfig {
                command: "npm run validate".to_string(),
            },
            agent: AgentConfig {
                default: Some(AgentKind::Codex),
            },
            states: BTreeMap::new(),
            scheduler: SchedulerConfig {
                lease_timeout_seconds: 900,
                heartbeat_interval_seconds: 15,
            },
            prompts: PromptsConfig {
                turn_budget: TurnBudgetConfig {
                    understand: 100,
                    planning: 100,
                    doing: 100,
                    gitting: 100,
                    reviewing: 100,
                    merging: 100,
                },
            },
            learning: LearningConfig {
                confidence_decay_per_day: 0.01,
                deactivate_below_confidence: 0.20,
            },
            seeding: SeedingConfig {
                backend: AgentKind::Codex,
                model: "gpt-5-codex".to_string(),
                max_turns: 12,
            },
            execution: ExecutionConfig {
                permissions_mode: "permissive_v1".to_string(),
                worker_mode: "normal".to_string(),
                test_mode: false,
                git_output_mode: GitOutputMode::PullRequest,
            },
            triage: TriageConfig {
                output_path: ".gardener/repo-intelligence.toml".to_string(),
                stale_after_commits: 50,
                discovery_max_turns: 12,
            },
            quality_report: QualityReportConfig {
                path: "docs/quality-grades.md".to_string(),
                stale_after_days: 7,
                stale_if_head_commit_differs: true,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PartialAppConfig {
    orchestrator: Option<PartialOrchestratorConfig>,
    scope: Option<PartialScopeConfig>,
    startup: Option<PartialStartupConfig>,
    validation: Option<PartialValidationConfig>,
    agent: Option<AgentConfig>,
    states: Option<BTreeMap<String, StateConfig>>,
    scheduler: Option<PartialSchedulerConfig>,
    prompts: Option<PartialPromptsConfig>,
    learning: Option<PartialLearningConfig>,
    seeding: Option<PartialSeedingConfig>,
    execution: Option<PartialExecutionConfig>,
    triage: Option<PartialTriageConfig>,
    quality_report: Option<PartialQualityReportConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PartialOrchestratorConfig {
    parallelism: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PartialScopeConfig {
    working_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PartialStartupConfig {
    validate_on_boot: Option<bool>,
    validation_command: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PartialValidationConfig {
    command: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PartialSchedulerConfig {
    lease_timeout_seconds: Option<u64>,
    heartbeat_interval_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PartialPromptsConfig {
    turn_budget: Option<PartialTurnBudgetConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PartialTurnBudgetConfig {
    understand: Option<u32>,
    planning: Option<u32>,
    doing: Option<u32>,
    gitting: Option<u32>,
    reviewing: Option<u32>,
    merging: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PartialLearningConfig {
    confidence_decay_per_day: Option<f64>,
    deactivate_below_confidence: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PartialSeedingConfig {
    backend: Option<AgentKind>,
    model: Option<String>,
    max_turns: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PartialExecutionConfig {
    permissions_mode: Option<String>,
    worker_mode: Option<String>,
    test_mode: Option<bool>,
    git_output_mode: Option<GitOutputMode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PartialTriageConfig {
    output_path: Option<String>,
    stale_after_commits: Option<u64>,
    discovery_max_turns: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PartialQualityReportConfig {
    path: Option<String>,
    stale_after_days: Option<u64>,
    stale_if_head_commit_differs: Option<bool>,
}

pub fn load_config(
    overrides: &CliOverrides,
    process_cwd: &Path,
    fs: &dyn FileSystem,
    process_runner: &dyn ProcessRunner,
) -> Result<(AppConfig, RuntimeScope), GardenerError> {
    append_run_log(
        "info",
        "config.load.started",
        json!({
            "process_cwd": process_cwd.display().to_string(),
            "config_path_override": overrides.config_path.as_ref().map(|p| p.display().to_string()),
            "parallelism_override": overrides.parallelism,
            "agent_override": overrides.agent.map(|a| format!("{:?}", a))
        }),
    );

    let mut cfg = AppConfig::default();

    if let Some(path) = resolve_config_path(overrides, process_cwd, fs, process_runner) {
        append_run_log(
            "info",
            "config.file.found",
            json!({
                "path": path.display().to_string()
            }),
        );
        let file_contents = fs.read_to_string(&path)?;
        let partial: PartialAppConfig = toml::from_str(&file_contents).map_err(|e| {
            append_run_log(
                "error",
                "config.file.parse_error",
                json!({
                    "path": path.display().to_string(),
                    "error": e.to_string()
                }),
            );
            GardenerError::ConfigParse(e.to_string())
        })?;
        merge_partial_config(&mut cfg, partial);
    } else {
        append_run_log(
            "info",
            "config.file.not_found",
            json!({
                "process_cwd": process_cwd.display().to_string(),
                "using_defaults": true
            }),
        );
    }

    apply_cli_overrides(&mut cfg, overrides);

    let scope = resolve_scope(process_cwd, &cfg, overrides, process_runner);

    if let Err(e) = validate_config(&cfg) {
        append_run_log(
            "error",
            "config.validation.failed",
            json!({
                "error": e.to_string()
            }),
        );
        return Err(e);
    }

    append_run_log(
        "info",
        "config.loaded",
        json!({
            "parallelism": cfg.orchestrator.parallelism,
            "agent_default": cfg.agent.default.map(|a| format!("{:?}", a)),
            "validation_command": cfg.validation.command,
            "working_dir": scope.working_dir.display().to_string(),
            "repo_root": scope.repo_root.as_ref().map(|p| p.display().to_string()),
            "test_mode": cfg.execution.test_mode,
            "seeding_backend": format!("{:?}", cfg.seeding.backend),
            "quality_report_path": cfg.quality_report.path
        }),
    );

    Ok((cfg, scope))
}

fn resolve_config_path<'a>(
    overrides: &'a CliOverrides,
    process_cwd: &'a Path,
    fs: &dyn FileSystem,
    process_runner: &dyn ProcessRunner,
) -> Option<PathBuf> {
    if let Some(path) = &overrides.config_path {
        return Some(absolutize_path(process_cwd, path));
    }

    let repo_root = detect_repo_root(process_cwd, process_runner);
    if let Some(root) = repo_root {
        let candidate = root.join(DEFAULT_CONFIG_FILE);
        if fs.exists(&candidate) {
            return Some(candidate);
        }
    }

    let cwd_candidate = process_cwd.join(DEFAULT_CONFIG_FILE);
    if fs.exists(&cwd_candidate) {
        return Some(cwd_candidate);
    }

    None
}

fn merge_partial_config(cfg: &mut AppConfig, partial: PartialAppConfig) {
    if let Some(orchestrator) = partial.orchestrator {
        if let Some(parallelism) = orchestrator.parallelism {
            cfg.orchestrator.parallelism = parallelism;
        }
    }

    if let Some(scope) = partial.scope {
        cfg.scope.working_dir = scope.working_dir;
    }

    if let Some(startup) = partial.startup {
        if let Some(validate_on_boot) = startup.validate_on_boot {
            cfg.startup.validate_on_boot = validate_on_boot;
        }
        if let Some(validation_command) = startup.validation_command {
            cfg.startup.validation_command = Some(validation_command);
        }
    }

    if let Some(validation) = partial.validation {
        if let Some(command) = validation.command {
            cfg.validation.command = command;
        }
    }

    if let Some(agent) = partial.agent {
        cfg.agent = agent;
    }

    if let Some(states) = partial.states {
        cfg.states = states;
    }

    if let Some(scheduler) = partial.scheduler {
        if let Some(value) = scheduler.lease_timeout_seconds {
            cfg.scheduler.lease_timeout_seconds = value;
        }
        if let Some(value) = scheduler.heartbeat_interval_seconds {
            cfg.scheduler.heartbeat_interval_seconds = value;
        }
    }

    if let Some(prompts) = partial.prompts {
        if let Some(turn_budget) = prompts.turn_budget {
            if let Some(value) = turn_budget.understand {
                cfg.prompts.turn_budget.understand = value;
            }
            if let Some(value) = turn_budget.planning {
                cfg.prompts.turn_budget.planning = value;
            }
            if let Some(value) = turn_budget.doing {
                cfg.prompts.turn_budget.doing = value;
            }
            if let Some(value) = turn_budget.gitting {
                cfg.prompts.turn_budget.gitting = value;
            }
            if let Some(value) = turn_budget.reviewing {
                cfg.prompts.turn_budget.reviewing = value;
            }
            if let Some(value) = turn_budget.merging {
                cfg.prompts.turn_budget.merging = value;
            }
        }
    }

    if let Some(learning) = partial.learning {
        if let Some(value) = learning.confidence_decay_per_day {
            cfg.learning.confidence_decay_per_day = value;
        }
        if let Some(value) = learning.deactivate_below_confidence {
            cfg.learning.deactivate_below_confidence = value;
        }
    }

    if let Some(seeding) = partial.seeding {
        if let Some(backend) = seeding.backend {
            cfg.seeding.backend = backend;
        }
        if let Some(model) = seeding.model {
            cfg.seeding.model = model;
        }
        if let Some(max_turns) = seeding.max_turns {
            cfg.seeding.max_turns = max_turns;
        }
    }

    if let Some(execution) = partial.execution {
        if let Some(value) = execution.permissions_mode {
            cfg.execution.permissions_mode = value;
        }
        if let Some(value) = execution.worker_mode {
            cfg.execution.worker_mode = value;
        }
        if let Some(value) = execution.test_mode {
            cfg.execution.test_mode = value;
        }
        if let Some(value) = execution.git_output_mode {
            cfg.execution.git_output_mode = value;
        }
    }

    if let Some(triage) = partial.triage {
        if let Some(value) = triage.output_path {
            cfg.triage.output_path = value;
        }
        if let Some(value) = triage.stale_after_commits {
            cfg.triage.stale_after_commits = value;
        }
        if let Some(value) = triage.discovery_max_turns {
            cfg.triage.discovery_max_turns = value;
        }
    }

    if let Some(quality) = partial.quality_report {
        if let Some(value) = quality.path {
            cfg.quality_report.path = value;
        }
        if let Some(value) = quality.stale_after_days {
            cfg.quality_report.stale_after_days = value;
        }
        if let Some(value) = quality.stale_if_head_commit_differs {
            cfg.quality_report.stale_if_head_commit_differs = value;
        }
    }
}

fn apply_cli_overrides(cfg: &mut AppConfig, overrides: &CliOverrides) {
    if let Some(parallelism) = overrides.parallelism {
        cfg.orchestrator.parallelism = parallelism;
    }
    if let Some(agent) = overrides.agent {
        cfg.agent.default = Some(agent);
    }
    if let Some(validation_command) = &overrides.validation_command {
        cfg.validation.command = validation_command.clone();
    }
}

pub fn resolve_scope(
    process_cwd: &Path,
    cfg: &AppConfig,
    overrides: &CliOverrides,
    process_runner: &dyn ProcessRunner,
) -> RuntimeScope {
    let process_cwd = process_cwd.to_path_buf();
    let repo_root = detect_repo_root(&process_cwd, process_runner);

    let working_dir = if let Some(path) = &overrides.working_dir {
        absolutize_path(&process_cwd, path)
    } else if let Some(path) = &cfg.scope.working_dir {
        absolutize_path(&process_cwd, path)
    } else if let Some(root) = &repo_root {
        root.clone()
    } else {
        process_cwd.clone()
    };

    append_run_log(
        "info",
        "config.scope.resolved",
        json!({
            "process_cwd": process_cwd.display().to_string(),
            "repo_root": repo_root.as_ref().map(|p| p.display().to_string()),
            "working_dir": working_dir.display().to_string()
        }),
    );

    RuntimeScope {
        process_cwd,
        repo_root,
        working_dir,
    }
}

fn absolutize_path(base: &Path, value: &Path) -> PathBuf {
    if value.is_absolute() {
        value.to_path_buf()
    } else {
        base.join(value)
    }
}

fn detect_repo_root(process_cwd: &Path, process_runner: &dyn ProcessRunner) -> Option<PathBuf> {
    let output = process_runner
        .run(ProcessRequest {
            program: "git".to_string(),
            args: vec!["rev-parse".to_string(), "--show-toplevel".to_string()],
            cwd: Some(process_cwd.to_path_buf()),
        })
        .ok()?;

    if output.exit_code != 0 {
        append_run_log(
            "debug",
            "config.repo_root.not_found",
            json!({
                "process_cwd": process_cwd.display().to_string(),
                "exit_code": output.exit_code
            }),
        );
        return None;
    }

    let trimmed = output.stdout.trim();
    if trimmed.is_empty() {
        append_run_log(
            "debug",
            "config.repo_root.empty_output",
            json!({
                "process_cwd": process_cwd.display().to_string()
            }),
        );
        return None;
    }

    let root = PathBuf::from(trimmed);
    append_run_log(
        "debug",
        "config.repo_root.detected",
        json!({
            "repo_root": root.display().to_string()
        }),
    );
    Some(root)
}

pub fn resolve_validation_command(
    cfg: &AppConfig,
    cli_override: Option<&str>,
) -> ValidationCommandResolution {
    if let Some(cli) = cli_override {
        append_run_log(
            "info",
            "config.validation_command.resolved",
            json!({
                "source": "cli_override",
                "command": cli
            }),
        );
        return ValidationCommandResolution {
            command: cli.to_string(),
            startup_validate_on_boot: cfg.startup.validate_on_boot,
            startup_validation_command: cfg.startup.validation_command.clone(),
        };
    }

    if !cfg.validation.command.trim().is_empty() {
        append_run_log(
            "info",
            "config.validation_command.resolved",
            json!({
                "source": "config.validation",
                "command": cfg.validation.command
            }),
        );
        return ValidationCommandResolution {
            command: cfg.validation.command.clone(),
            startup_validate_on_boot: cfg.startup.validate_on_boot,
            startup_validation_command: cfg.startup.validation_command.clone(),
        };
    }

    if let Some(startup) = &cfg.startup.validation_command {
        append_run_log(
            "info",
            "config.validation_command.resolved",
            json!({
                "source": "config.startup",
                "command": startup
            }),
        );
        return ValidationCommandResolution {
            command: startup.clone(),
            startup_validate_on_boot: cfg.startup.validate_on_boot,
            startup_validation_command: cfg.startup.validation_command.clone(),
        };
    }

    append_run_log(
        "warn",
        "config.validation_command.resolved",
        json!({
            "source": "fallback",
            "command": "true",
            "reason": "no validation command configured"
        }),
    );
    ValidationCommandResolution {
        command: "true".to_string(),
        startup_validate_on_boot: cfg.startup.validate_on_boot,
        startup_validation_command: cfg.startup.validation_command.clone(),
    }
}

fn validate_config(cfg: &AppConfig) -> Result<(), GardenerError> {
    append_run_log(
        "debug",
        "config.validate.started",
        json!({
            "parallelism": cfg.orchestrator.parallelism,
            "states": cfg.states.len(),
        }),
    );
    if cfg.orchestrator.parallelism == 0 {
        return Err(GardenerError::InvalidConfig(
            "orchestrator.parallelism must be greater than zero".to_string(),
        ));
    }

    if cfg.agent.default.is_none() {
        for state_cfg in cfg.states.values() {
            if state_cfg.backend.is_none() {
                return Err(GardenerError::InvalidConfig(
                    "agent.default is required when any state backend is omitted".to_string(),
                ));
            }
        }
    }

    if model_is_invalid(&cfg.seeding.model) {
        return Err(GardenerError::InvalidConfig(
            "seeding.model must be a real model id".to_string(),
        ));
    }

    for (state_name, state_cfg) in &cfg.states {
        if let Some(model) = &state_cfg.model {
            if model_is_invalid(model) {
                return Err(GardenerError::InvalidConfig(format!(
                    "states.{state_name}.model must be a real model id"
                )));
            }
        }
    }

    Ok(())
}

pub fn effective_agent_for_state(cfg: &AppConfig, state: WorkerState) -> Option<AgentKind> {
    let key = state_key(state);
    if let Some(state_cfg) = cfg.states.get(key) {
        if let Some(backend) = state_cfg.backend {
            return Some(backend);
        }
    }
    cfg.agent.default
}

pub fn effective_model_for_state(cfg: &AppConfig, state: WorkerState) -> String {
    let key = state_key(state);
    if let Some(state_cfg) = cfg.states.get(key) {
        if let Some(model) = &state_cfg.model {
            return model.clone();
        }
    }
    cfg.seeding.model.clone()
}

fn state_key(state: WorkerState) -> &'static str {
    match state {
        WorkerState::Understand => "understand",
        WorkerState::Planning => "planning",
        WorkerState::Doing => "doing",
        WorkerState::Gitting => "gitting",
        WorkerState::Reviewing => "reviewing",
        WorkerState::Merging => "merging",
        WorkerState::Seeding => "seeding",
        WorkerState::Complete => "complete",
        WorkerState::Failed => "failed",
        WorkerState::Parked => "parked",
    }
}

fn model_is_invalid(model: &str) -> bool {
    model.trim().is_empty() || model == "..." || model.eq_ignore_ascii_case("todo")
}
