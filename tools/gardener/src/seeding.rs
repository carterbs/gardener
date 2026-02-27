use crate::config::AppConfig;
use crate::errors::GardenerError;
use crate::protocol::AgentEvent;
use crate::repo_intelligence::RepoIntelligenceProfile;
use crate::runtime::ProcessRunner;
use crate::seed_runner::{run_legacy_seed_runner_v1_with_events, SeedTask};
use crate::types::RuntimeScope;

pub fn build_seed_prompt(profile: &RepoIntelligenceProfile, quality_doc: &str) -> String {
    format!(
        "Seed backlog tasks for primary_gap={} with readiness_score={}.\nUse evidence:\n{}",
        profile.agent_readiness.primary_gap, profile.agent_readiness.readiness_score, quality_doc
    )
}

pub fn seed_backlog_if_needed(
    process_runner: &dyn ProcessRunner,
    scope: &RuntimeScope,
    cfg: &AppConfig,
    profile: &RepoIntelligenceProfile,
    quality_doc: &str,
) -> Result<Vec<SeedTask>, GardenerError> {
    seed_backlog_if_needed_with_events(process_runner, scope, cfg, profile, quality_doc, None)
}

pub fn seed_backlog_if_needed_with_events(
    process_runner: &dyn ProcessRunner,
    scope: &RuntimeScope,
    cfg: &AppConfig,
    profile: &RepoIntelligenceProfile,
    quality_doc: &str,
    mut on_event: Option<&mut dyn FnMut(&AgentEvent)>,
) -> Result<Vec<SeedTask>, GardenerError> {
    let prompt = build_seed_prompt(profile, quality_doc);
    if let Some(sink) = on_event.as_mut() {
        run_legacy_seed_runner_v1_with_events(
            process_runner,
            scope,
            cfg.seeding.backend,
            &cfg.seeding.model,
            &prompt,
            Some(*sink),
        )
    } else {
        run_legacy_seed_runner_v1_with_events(
            process_runner,
            scope,
            cfg.seeding.backend,
            &cfg.seeding.model,
            &prompt,
            None,
        )
    }
}
