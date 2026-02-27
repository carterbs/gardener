use serde::{Deserialize, Serialize};

use crate::task_identity::TaskKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Priority {
    P0,
    P1,
    P2,
}

impl Priority {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::P0 => "P0",
            Self::P1 => "P1",
            Self::P2 => "P2",
        }
    }

    pub fn rank(self) -> i64 {
        match self {
            Self::P0 => 0,
            Self::P1 => 1,
            Self::P2 => 2,
        }
    }

    pub fn from_db(value: &str) -> Option<Self> {
        match value {
            "P0" => Some(Self::P0),
            "P1" => Some(Self::P1),
            "P2" => Some(Self::P2),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClassifierInput {
    pub kind: TaskKind,
    pub validation_failed: bool,
    pub has_related_open_pr: bool,
    pub merge_conflict: bool,
    pub scheduler_blocked: bool,
}

pub fn classify_priority(input: &ClassifierInput) -> Priority {
    if input.validation_failed
        || input.merge_conflict
        || input.scheduler_blocked
        || input.kind == TaskKind::PrCollision
        || input.kind == TaskKind::MergeConflict
        || input.kind == TaskKind::Infra
        || input.has_related_open_pr
    {
        return Priority::P0;
    }

    if matches!(
        input.kind,
        TaskKind::Feature | TaskKind::Bugfix | TaskKind::QualityGap
    ) {
        return Priority::P1;
    }

    Priority::P2
}

#[cfg(test)]
mod tests {
    use super::{classify_priority, ClassifierInput, Priority};
    use crate::task_identity::TaskKind;

    #[test]
    fn classifier_produces_expected_priority_ordering() {
        let p0 = classify_priority(&ClassifierInput {
            kind: TaskKind::MergeConflict,
            ..ClassifierInput::default()
        });
        let p1 = classify_priority(&ClassifierInput {
            kind: TaskKind::Feature,
            ..ClassifierInput::default()
        });
        let p2 = classify_priority(&ClassifierInput {
            kind: TaskKind::Maintenance,
            ..ClassifierInput::default()
        });

        assert_eq!(p0, Priority::P0);
        assert_eq!(p1, Priority::P1);
        assert_eq!(p2, Priority::P2);
        assert!(p0.rank() < p1.rank());
        assert!(p1.rank() < p2.rank());
    }

    #[test]
    fn classifier_uses_escalation_flags_deterministically() {
        let cases = [
            ClassifierInput {
                kind: TaskKind::Maintenance,
                validation_failed: true,
                ..ClassifierInput::default()
            },
            ClassifierInput {
                kind: TaskKind::Maintenance,
                has_related_open_pr: true,
                ..ClassifierInput::default()
            },
            ClassifierInput {
                kind: TaskKind::Maintenance,
                merge_conflict: true,
                ..ClassifierInput::default()
            },
            ClassifierInput {
                kind: TaskKind::Maintenance,
                scheduler_blocked: true,
                ..ClassifierInput::default()
            },
        ];

        for case in cases {
            assert_eq!(classify_priority(&case), Priority::P0);
        }
    }

    #[test]
    fn db_round_trip_strings_are_stable() {
        for value in [Priority::P0, Priority::P1, Priority::P2] {
            assert_eq!(Priority::from_db(value.as_str()), Some(value));
        }
        assert_eq!(Priority::from_db("bad"), None);
    }
}
