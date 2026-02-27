use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum TaskKind {
    QualityGap,
    MergeConflict,
    PrCollision,
    Feature,
    Bugfix,
    #[default]
    Maintenance,
    Infra,
}

impl TaskKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::QualityGap => "quality_gap",
            Self::MergeConflict => "merge_conflict",
            Self::PrCollision => "pr_collision",
            Self::Feature => "feature",
            Self::Bugfix => "bugfix",
            Self::Maintenance => "maintenance",
            Self::Infra => "infra",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskIdentity {
    pub kind: TaskKind,
    pub title: String,
    pub scope_key: String,
    pub related_pr: Option<i64>,
    pub related_branch: Option<String>,
}

impl TaskIdentity {
    pub fn canonical(self) -> CanonicalTaskIdentity {
        CanonicalTaskIdentity {
            kind: self.kind,
            title: normalize_text(&self.title),
            scope_key: normalize_text(&self.scope_key),
            related_pr: self.related_pr,
            related_branch: self.related_branch.as_deref().map(normalize_text),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalTaskIdentity {
    pub kind: TaskKind,
    pub title: String,
    pub scope_key: String,
    pub related_pr: Option<i64>,
    pub related_branch: Option<String>,
}

impl CanonicalTaskIdentity {
    pub fn canonical_json(&self) -> String {
        let related_pr = self
            .related_pr
            .map_or_else(|| "null".to_string(), |value| value.to_string());
        let related_branch = self.related_branch.as_ref().map_or_else(
            || "null".to_string(),
            |value| format!("\"{}\"", escape_json(value)),
        );

        format!(
            "{{\"kind\":\"{}\",\"title\":\"{}\",\"scope_key\":\"{}\",\"related_pr\":{},\"related_branch\":{}}}",
            self.kind.as_str(),
            escape_json(&self.title),
            escape_json(&self.scope_key),
            related_pr,
            related_branch
        )
    }

    pub fn task_id(&self) -> String {
        let canonical = self.canonical_json();
        let mut digest = Sha256::new();
        digest.update(canonical.as_bytes());
        let bytes = digest.finalize();
        let mut out = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            out.push_str(&format!("{byte:02x}"));
        }
        out
    }
}

pub fn compute_task_id(identity: TaskIdentity) -> String {
    identity.canonical().task_id()
}

pub fn normalize_text(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_lowercase()
}

fn escape_json(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

#[cfg(test)]
mod tests {
    use super::{compute_task_id, normalize_text, CanonicalTaskIdentity, TaskIdentity, TaskKind};

    #[test]
    fn normalization_contract_is_stable() {
        assert_eq!(normalize_text("  Build   API   Layer  "), "build api layer");
        assert_eq!(normalize_text("\nDomain:Payments\t"), "domain:payments");
    }

    #[test]
    fn task_id_is_stable_for_logically_identical_inputs() {
        let first = compute_task_id(TaskIdentity {
            kind: TaskKind::Feature,
            title: "  Add New Checkout Flow".to_string(),
            scope_key: " Domain:Payments ".to_string(),
            related_pr: Some(42),
            related_branch: Some(" Feature/Checkout ".to_string()),
        });

        let second = compute_task_id(TaskIdentity {
            kind: TaskKind::Feature,
            title: "add new   checkout flow".to_string(),
            scope_key: "domain:payments".to_string(),
            related_pr: Some(42),
            related_branch: Some("feature/checkout".to_string()),
        });

        assert_eq!(first, second);
    }

    #[test]
    fn canonical_json_field_order_is_deterministic() {
        let identity = CanonicalTaskIdentity {
            kind: TaskKind::Bugfix,
            title: "fix scheduler".to_string(),
            scope_key: "domain:orchestrator".to_string(),
            related_pr: None,
            related_branch: None,
        };
        let json = identity.canonical_json();
        assert_eq!(
            json,
            "{\"kind\":\"bugfix\",\"title\":\"fix scheduler\",\"scope_key\":\"domain:orchestrator\",\"related_pr\":null,\"related_branch\":null}"
        );
    }

    #[test]
    fn task_kind_strings_match_contract() {
        assert_eq!(TaskKind::QualityGap.as_str(), "quality_gap");
        assert_eq!(TaskKind::MergeConflict.as_str(), "merge_conflict");
        assert_eq!(TaskKind::PrCollision.as_str(), "pr_collision");
        assert_eq!(TaskKind::Feature.as_str(), "feature");
        assert_eq!(TaskKind::Bugfix.as_str(), "bugfix");
        assert_eq!(TaskKind::Maintenance.as_str(), "maintenance");
        assert_eq!(TaskKind::Infra.as_str(), "infra");
    }
}
