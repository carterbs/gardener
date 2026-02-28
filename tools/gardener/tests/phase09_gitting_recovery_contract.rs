use gardener::config::GitOutputMode;
use gardener::prompt_registry::PromptRegistry;
use gardener::types::WorkerState;

#[test]
fn commit_only_gitting_prompt_requires_pre_commit_recovery_steps() {
    let template = PromptRegistry::v1()
        .with_gitting_mode(&GitOutputMode::CommitOnly)
        .template_for(WorkerState::Gitting)
        .expect("template must exist");

    assert_eq!(template.version, "v1-gitting-commit-only");
    assert!(template.body.contains("pre-commit"));
    assert!(template.body.contains("git add -A"));
    assert!(template.body.contains("git commit"));
    assert!(template.body.contains("If commit fails"));
    assert!(template.body.contains("git status --porcelain"));
    assert!(template.body.contains("rerun git commit"));
}
