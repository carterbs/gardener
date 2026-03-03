use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::priority::Priority;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssessmentPayload {
    pub domains: Vec<DomainAssessment>,
    pub repo_wide: RepoWideAssessment,
    pub deficiencies: Vec<StructuralDeficiency>,
    pub domain_file_map: BTreeMap<String, Vec<String>>,
    pub primary_gap: String,
    pub languages_detected: Vec<String>,
    /// Per repo-wide dimension rationale (e.g. "agent_steering" -> "2-4 sentence explanation").
    #[serde(default)]
    pub repo_wide_rationale: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainAssessment {
    pub name: String,
    pub languages: Vec<String>,
    pub scores: DomainScores,
    pub notes: Vec<String>,
    /// Per-dimension rationale (e.g. "test_coverage" -> "1-2 sentence explanation").
    #[serde(default)]
    pub dimension_rationale: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainScores {
    pub test_coverage: u8,
    pub test_quality: u8,
    pub risk_exposure: u8,
    pub convention_adherence: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoWideAssessment {
    pub agent_steering: u8,
    pub mechanical_guardrails: u8,
    pub local_feedback_loop: u8,
    pub coverage_infrastructure: u8,
    pub documentation_quality: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuralDeficiency {
    pub description: String,
    pub domain: Option<String>,
    pub category: DeficiencyCategory,
    pub severity: Priority,
    pub suggested_task_title: String,
    pub suggested_task_details: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DeficiencyCategory {
    CoverageGap,
    MissingTooling,
    MissingDocumentation,
    ConventionViolation,
    ObservabilityGap,
    FeedbackLoopGap,
}

impl DeficiencyCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CoverageGap => "coverage-gap",
            Self::MissingTooling => "missing-tooling",
            Self::MissingDocumentation => "missing-documentation",
            Self::ConventionViolation => "convention-violation",
            Self::ObservabilityGap => "observability-gap",
            Self::FeedbackLoopGap => "feedback-loop-gap",
        }
    }
}

/// 9-level grade scale
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Grade {
    A,
    BPlus,
    B,
    BMinus,
    CPlus,
    C,
    CMinus,
    D,
    F,
}

impl Grade {
    pub fn from_score(score: f64) -> Self {
        match score as u8 {
            93..=100 => Grade::A,
            87..=92 => Grade::BPlus,
            80..=86 => Grade::B,
            75..=79 => Grade::BMinus,
            68..=74 => Grade::CPlus,
            60..=67 => Grade::C,
            55..=59 => Grade::CMinus,
            40..=54 => Grade::D,
            _ => Grade::F,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Grade::A => "A",
            Grade::BPlus => "B+",
            Grade::B => "B",
            Grade::BMinus => "B-",
            Grade::CPlus => "C+",
            Grade::C => "C",
            Grade::CMinus => "C-",
            Grade::D => "D",
            Grade::F => "F",
        }
    }
}
