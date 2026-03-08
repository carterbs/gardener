use crate::agent::factory::AdapterFactory;
use crate::agent_turn::{run_agent_turn, AgentTurnInput};
use crate::config::AppConfig;
use crate::errors::GardenerError;
use crate::fsm::DoingOutput;
use crate::git::GitClient;
use crate::learning_loop::LearningLoop;
use crate::logging::append_run_log;
use crate::prompt_registry::PromptRegistry;
use crate::protocol::AgentTerminal;
use crate::runtime::ProcessRunner;
use crate::types::{RuntimeScope, WorkerState};
use crate::worker_identity::WorkerIdentity;
use serde_json::json;
use std::path::Path;

pub struct DoContext<'a> {
    pub cfg: &'a AppConfig,
    pub process_runner: &'a dyn ProcessRunner,
    pub scope: &'a RuntimeScope,
    pub worktree_path: &'a Path,
    pub factory: &'a AdapterFactory,
    pub registry: &'a PromptRegistry,
    pub learning_loop: &'a LearningLoop,
    pub identity: &'a WorkerIdentity,
    pub task_summary: &'a str,
    pub attempt_count: i64,
    /// If provided alongside `pre_doing_sha`, enables git-commit fallback when
    /// the doing payload cannot be parsed or the agent reports failure.
    pub git: Option<&'a GitClient<'a>>,
    /// HEAD SHA captured *before* the agent turn, used for git fallback.
    pub pre_doing_sha: Option<String>,
    #[allow(clippy::type_complexity)]
    pub on_step: Option<&'a dyn Fn(&str, &str)>,
    pub on_agent_event: Option<&'a dyn Fn(&crate::protocol::AgentEvent)>,
}

pub struct DoOutcome {
    pub summary: String,
    pub prompt_version: String,
    pub context_manifest_hash: String,
}

pub fn run_do(ctx: &DoContext<'_>) -> Result<DoOutcome, GardenerError> {
    append_run_log(
        "info",
        "do_phase.started",
        json!({ "worker_id": ctx.identity.worker_id, "task_summary": ctx.task_summary }),
    );
    if let Some(on_step) = ctx.on_step {
        on_step("DO", "starting do phase");
    }

    let result = match run_agent_turn(AgentTurnInput {
        cfg: ctx.cfg,
        process_runner: ctx.process_runner,
        scope: ctx.scope,
        worktree_path: ctx.worktree_path,
        factory: ctx.factory,
        registry: ctx.registry,
        learning_loop: ctx.learning_loop,
        identity: ctx.identity,
        state: WorkerState::Doing,
        task_summary: ctx.task_summary,
        attempt_count: ctx.attempt_count,
        prompt_override: None,
        on_event: ctx.on_agent_event,
    }) {
        Ok(r) => r,
        Err(agent_err) => {
            append_run_log(
                "error",
                "do_phase.agent_crash",
                json!({
                    "worker_id": ctx.identity.worker_id,
                    "error": agent_err.to_string()
                }),
            );
            if let Some(salvaged) = try_git_salvage(ctx)? {
                return Ok(salvaged);
            }
            return Err(agent_err);
        }
    };

    if result.terminal == AgentTerminal::Failure {
        let reason = result
            .payload
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("do phase failed")
            .to_string();
        if let Some(salvaged) = try_git_salvage(ctx)? {
            append_run_log(
                "warn",
                "do_phase.agent_failure_salvaged",
                json!({
                    "worker_id": ctx.identity.worker_id,
                    "reason": reason,
                    "summary": salvaged.summary
                }),
            );
            return Ok(salvaged);
        }
        return Err(GardenerError::Process(format!(
            "do phase agent failure: {reason}"
        )));
    }

    match parse_doing_output(&result.payload, &ctx.identity.worker_id, ctx.task_summary) {
        Ok(doing_output) => {
            if let Some(on_step) = ctx.on_step {
                on_step("DO", "do phase complete");
            }
            Ok(DoOutcome {
                summary: doing_output.summary,
                prompt_version: result.prompt_version,
                context_manifest_hash: result.context_manifest_hash,
            })
        }
        Err(parse_err) => {
            if let Some(salvaged) = try_git_salvage(ctx)? {
                append_run_log(
                    "warn",
                    "do_phase.payload_salvaged",
                    json!({
                        "worker_id": ctx.identity.worker_id,
                        "parse_error": parse_err.to_string(),
                        "summary": salvaged.summary
                    }),
                );
                return Ok(salvaged);
            }
            Err(parse_err)
        }
    }
}

/// Attempt to salvage a `DoOutcome` from git history when the agent fails or
/// produces an unparseable payload. Returns `Ok(None)` when no salvageable
/// work is found.
fn try_git_salvage(ctx: &DoContext<'_>) -> Result<Option<DoOutcome>, GardenerError> {
    let (git, pre_sha) = match (ctx.git, ctx.pre_doing_sha.as_deref()) {
        (Some(g), Some(sha)) => (g, sha),
        _ => return Ok(None),
    };

    let commits = git.commits_since(pre_sha).unwrap_or_default();
    if let Some(subject) = commits.into_iter().next() {
        append_run_log(
            "warn",
            "do_phase.salvage_from_commits",
            json!({
                "worker_id": ctx.identity.worker_id,
                "commit_subject": &subject
            }),
        );
        return Ok(Some(DoOutcome {
            summary: subject,
            prompt_version: String::new(),
            context_manifest_hash: String::new(),
        }));
    }

    if !git.worktree_is_clean()? {
        let msg = fallback_commit_message(ctx.task_summary);
        git.commit_all(&msg)?;
        append_run_log(
            "warn",
            "do_phase.salvage_from_dirty_worktree",
            json!({
                "worker_id": ctx.identity.worker_id,
                "commit_message": &msg
            }),
        );
        return Ok(Some(DoOutcome {
            summary: msg,
            prompt_version: String::new(),
            context_manifest_hash: String::new(),
        }));
    }

    Ok(None)
}

pub(crate) fn parse_doing_output(
    payload: &serde_json::Value,
    worker_id: &str,
    task_summary: &str,
) -> Result<DoingOutput, GardenerError> {
    if let Ok(parsed) = serde_json::from_value::<DoingOutput>(payload.clone()) {
        return Ok(parsed);
    }
    append_run_log(
        "error",
        "worker.doing.payload_invalid",
        json!({
            "worker_id": worker_id, "task_summary": task_summary, "payload": payload,
        }),
    );
    Err(GardenerError::Process(format!(
        "doing phase produced invalid payload (task will be marked unresolved): {payload}"
    )))
}

pub(crate) fn fallback_commit_message(task_summary: &str) -> String {
    let first_line = task_summary.lines().next().unwrap_or_default().trim();
    if first_line.is_empty() {
        return "feat: implement requested changes".to_string();
    }
    let normalized = first_line.split_whitespace().collect::<Vec<_>>().join(" ");
    let max_desc_len = 72usize.saturating_sub("feat: ".len());
    let desc = normalized.chars().take(max_desc_len).collect::<String>();
    if desc.is_empty() {
        "feat: implement requested changes".to_string()
    } else {
        format!("feat: {desc}")
    }
}

#[cfg(test)]
mod tests {
    use super::{fallback_commit_message, parse_doing_output, try_git_salvage, DoContext};
    use crate::agent::factory::AdapterFactory;
    use crate::config::AppConfig;
    use crate::git::GitClient;
    use crate::learning_loop::LearningLoop;
    use crate::prompt_registry::PromptRegistry;
    use crate::runtime::{FakeProcessRunner, ProcessOutput};
    use crate::types::RuntimeScope;
    use crate::worker_identity::WorkerIdentity;
    use std::path::PathBuf;

    fn test_do_ctx<'a>(
        cfg: &'a AppConfig,
        runner: &'a FakeProcessRunner,
        scope: &'a RuntimeScope,
        factory: &'a AdapterFactory,
        registry: &'a PromptRegistry,
        learning_loop: &'a LearningLoop,
        identity: &'a WorkerIdentity,
        git: Option<&'a GitClient<'a>>,
        pre_doing_sha: Option<String>,
    ) -> DoContext<'a> {
        DoContext {
            cfg,
            process_runner: runner,
            scope,
            worktree_path: std::path::Path::new("/repo"),
            factory,
            registry,
            learning_loop,
            identity,
            task_summary: "feature: add recovery",
            attempt_count: 1,
            git,
            pre_doing_sha,
            on_step: None,
            on_agent_event: None,
        }
    }

    #[test]
    fn parse_doing_output_returns_err_when_payload_invalid() {
        let result = parse_doing_output(&serde_json::json!({"foo": "bar"}), "worker-1", "Add test");
        assert!(result.is_err());
    }

    #[test]
    fn parse_doing_output_returns_err_when_payload_null() {
        let result = parse_doing_output(&serde_json::Value::Null, "worker-1", "Add test");
        assert!(result.is_err());
    }

    #[test]
    fn parse_doing_output_succeeds_with_valid_payload() {
        let payload = serde_json::json!({"summary": "did the thing"});
        let result = parse_doing_output(&payload, "worker-1", "Add test");
        assert_eq!(
            result.expect("valid payload should parse").summary,
            "did the thing"
        );
    }

    #[test]
    fn fallback_commit_message_handles_empty_summary() {
        let message = fallback_commit_message("   ");
        assert_eq!(message, "feat: implement requested changes");
    }

    #[test]
    fn try_git_salvage_returns_none_without_git() {
        let cfg = AppConfig::default();
        let runner = FakeProcessRunner::default();
        let scope = RuntimeScope {
            process_cwd: PathBuf::from("/repo"),
            repo_root: Some(PathBuf::from("/repo")),
            working_dir: PathBuf::from("/repo"),
        };
        let factory = AdapterFactory::with_defaults();
        let registry = PromptRegistry::v1();
        let learning_loop = LearningLoop::default();
        let identity = WorkerIdentity::new("worker-1");
        let ctx = test_do_ctx(
            &cfg, &runner, &scope, &factory, &registry, &learning_loop, &identity, None, None,
        );
        let result = try_git_salvage(&ctx).expect("should succeed");
        assert!(result.is_none());
    }

    #[test]
    fn try_git_salvage_prefers_existing_commit_subject() {
        let cfg = AppConfig::default();
        let runner = FakeProcessRunner::default();
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "feat: recovered work\n".to_string(),
            stderr: String::new(),
        }));

        let scope = RuntimeScope {
            process_cwd: PathBuf::from("/repo"),
            repo_root: Some(PathBuf::from("/repo")),
            working_dir: PathBuf::from("/repo"),
        };
        let factory = AdapterFactory::with_defaults();
        let registry = PromptRegistry::v1();
        let learning_loop = LearningLoop::default();
        let identity = WorkerIdentity::new("worker-1");
        let git = GitClient::new(&runner, "/repo");
        let ctx = test_do_ctx(
            &cfg,
            &runner,
            &scope,
            &factory,
            &registry,
            &learning_loop,
            &identity,
            Some(&git),
            Some("abc123".to_string()),
        );
        let salvaged = try_git_salvage(&ctx)
            .expect("salvage should succeed")
            .expect("should salvage");
        assert_eq!(salvaged.summary, "feat: recovered work");
    }

    #[test]
    fn try_git_salvage_commits_dirty_worktree_when_needed() {
        let cfg = AppConfig::default();
        let runner = FakeProcessRunner::default();
        // commits_since returns empty
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        // worktree_is_clean: status --porcelain returns dirty
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: " M src/lib.rs\n".to_string(),
            stderr: String::new(),
        }));
        // commit_all: worktree_is_clean check
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: " M src/lib.rs\n".to_string(),
            stderr: String::new(),
        }));
        // commit_all: git add -A
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        // commit_all: git commit
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: "[main abc123] feat: feature: add recovery\n".to_string(),
            stderr: String::new(),
        }));

        let scope = RuntimeScope {
            process_cwd: PathBuf::from("/repo"),
            repo_root: Some(PathBuf::from("/repo")),
            working_dir: PathBuf::from("/repo"),
        };
        let factory = AdapterFactory::with_defaults();
        let registry = PromptRegistry::v1();
        let learning_loop = LearningLoop::default();
        let identity = WorkerIdentity::new("worker-1");
        let git = GitClient::new(&runner, "/repo");
        let ctx = test_do_ctx(
            &cfg,
            &runner,
            &scope,
            &factory,
            &registry,
            &learning_loop,
            &identity,
            Some(&git),
            Some("abc123".to_string()),
        );
        let salvaged = try_git_salvage(&ctx)
            .expect("salvage should succeed")
            .expect("should salvage");
        assert_eq!(salvaged.summary, "feat: feature: add recovery");
    }

    #[test]
    fn try_git_salvage_returns_none_for_clean_tree_without_commits() {
        let cfg = AppConfig::default();
        let runner = FakeProcessRunner::default();
        // commits_since returns empty
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));
        // worktree_is_clean: status --porcelain returns empty (clean)
        runner.push_response(Ok(ProcessOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }));

        let scope = RuntimeScope {
            process_cwd: PathBuf::from("/repo"),
            repo_root: Some(PathBuf::from("/repo")),
            working_dir: PathBuf::from("/repo"),
        };
        let factory = AdapterFactory::with_defaults();
        let registry = PromptRegistry::v1();
        let learning_loop = LearningLoop::default();
        let identity = WorkerIdentity::new("worker-1");
        let git = GitClient::new(&runner, "/repo");
        let ctx = test_do_ctx(
            &cfg,
            &runner,
            &scope,
            &factory,
            &registry,
            &learning_loop,
            &identity,
            Some(&git),
            Some("abc123".to_string()),
        );
        let result = try_git_salvage(&ctx).expect("salvage should succeed");
        assert!(result.is_none());
    }
}
