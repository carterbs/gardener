use crate::config::AppConfig;
use crate::errors::GardenerError;
use crate::repo_intelligence::RepoIntelligenceProfile;
use crate::runtime::ProcessRunner;
use crate::seed_runner::{run_legacy_seed_runner_v1, SeedTask};
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
    let prompt = build_seed_prompt(profile, quality_doc);
    run_legacy_seed_runner_v1(
        process_runner,
        scope,
        cfg.seeding.backend,
        &cfg.seeding.model,
        &prompt,
    )
}
