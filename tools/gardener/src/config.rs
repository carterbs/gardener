use crate::errors::GardenerError;
use crate::runtime::{FileSystem, ProcessRequest, ProcessRunner};
use crate::types::{
    AgentKind, RuntimeScope, ValidationCommandResolution, ValidationCommandSource, WorkerState,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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
    pub allow_agent_discovery: bool,
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
    pub starvation_threshold_seconds: u64,
    pub reconcile_interval_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PromptsConfig {
    pub token_budget: TokenBudgetConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenBudgetConfig {
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionConfig {
    pub permissions_mode: String,
    pub worker_mode: String,
    pub test_mode: bool,
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
                allow_agent_discovery: true,
            },
            agent: AgentConfig {
                default: Some(AgentKind::Codex),
            },
            states: BTreeMap::new(),
            scheduler: SchedulerConfig {
                lease_timeout_seconds: 900,
                heartbeat_interval_seconds: 15,
                starvation_threshold_seconds: 180,
                reconcile_interval_seconds: 30,
            },
            prompts: PromptsConfig {
                token_budget: TokenBudgetConfig {
                    understand: 6000,
                    planning: 9000,
                    doing: 12000,
                    gitting: 4000,
                    reviewing: 10000,
                    merging: 5000,
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
    allow_agent_discovery: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PartialSchedulerConfig {
    lease_timeout_seconds: Option<u64>,
    heartbeat_interval_seconds: Option<u64>,
    starvation_threshold_seconds: Option<u64>,
    reconcile_interval_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PartialPromptsConfig {
    token_budget: Option<PartialTokenBudgetConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PartialTokenBudgetConfig {
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
    let mut cfg = AppConfig::default();

    if let Some(path) = &overrides.config_path {
        let file_contents = fs.read_to_string(path)?;
        let partial: PartialAppConfig = toml::from_str(&file_contents)
            .map_err(|e| GardenerError::ConfigParse(e.to_string()))?;
        merge_partial_config(&mut cfg, partial);
    }

    apply_cli_overrides(&mut cfg, overrides);

    let scope = resolve_scope(process_cwd, &cfg, overrides, process_runner);
    validate_config(&cfg)?;
    Ok((cfg, scope))
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
        if let Some(allow_agent_discovery) = validation.allow_agent_discovery {
            cfg.validation.allow_agent_discovery = allow_agent_discovery;
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
        if let Some(value) = scheduler.starvation_threshold_seconds {
            cfg.scheduler.starvation_threshold_seconds = value;
        }
        if let Some(value) = scheduler.reconcile_interval_seconds {
            cfg.scheduler.reconcile_interval_seconds = value;
        }
    }

    if let Some(prompts) = partial.prompts {
        if let Some(token_budget) = prompts.token_budget {
            if let Some(value) = token_budget.understand {
                cfg.prompts.token_budget.understand = value;
            }
            if let Some(value) = token_budget.planning {
                cfg.prompts.token_budget.planning = value;
            }
            if let Some(value) = token_budget.doing {
                cfg.prompts.token_budget.doing = value;
            }
            if let Some(value) = token_budget.gitting {
                cfg.prompts.token_budget.gitting = value;
            }
            if let Some(value) = token_budget.reviewing {
                cfg.prompts.token_budget.reviewing = value;
            }
            if let Some(value) = token_budget.merging {
                cfg.prompts.token_budget.merging = value;
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
        return None;
    }

    let trimmed = output.stdout.trim();
    if trimmed.is_empty() {
        return None;
    }

    Some(PathBuf::from(trimmed))
}

pub fn resolve_validation_command(
    cfg: &AppConfig,
    cli_override: Option<&str>,
) -> ValidationCommandResolution {
    if let Some(cli) = cli_override {
        return ValidationCommandResolution {
            command: cli.to_string(),
            source: ValidationCommandSource::CliOverride,
            startup_validate_on_boot: cfg.startup.validate_on_boot,
            startup_validation_command: cfg.startup.validation_command.clone(),
        };
    }

    if !cfg.validation.command.trim().is_empty() {
        return ValidationCommandResolution {
            command: cfg.validation.command.clone(),
            source: ValidationCommandSource::ConfigValidation,
            startup_validate_on_boot: cfg.startup.validate_on_boot,
            startup_validation_command: cfg.startup.validation_command.clone(),
        };
    }

    if let Some(startup) = &cfg.startup.validation_command {
        return ValidationCommandResolution {
            command: startup.clone(),
            source: ValidationCommandSource::StartupValidation,
            startup_validate_on_boot: cfg.startup.validate_on_boot,
            startup_validation_command: cfg.startup.validation_command.clone(),
        };
    }

    ValidationCommandResolution {
        command: "true".to_string(),
        source: ValidationCommandSource::AutoDiscovery,
        startup_validate_on_boot: cfg.startup.validate_on_boot,
        startup_validation_command: cfg.startup.validation_command.clone(),
    }
}

fn validate_config(cfg: &AppConfig) -> Result<(), GardenerError> {
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

    if cfg.seeding.model.trim().is_empty()
        || cfg.seeding.model == "..."
        || cfg.seeding.model.eq_ignore_ascii_case("todo")
    {
        return Err(GardenerError::InvalidConfig(
            "seeding.model must be a real model id".to_string(),
        ));
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
