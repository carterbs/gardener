use crate::errors::GardenerError;
use crate::logging::append_run_log;
use crate::types::WorkerState;
use serde::{Deserialize, Serialize};
use serde_json::json;

pub const MAX_DOING_TURNS: u32 = 100;
pub const MAX_REVIEW_LOOPS: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskCategory {
    Task,
    Chore,
    Infra,
    Feature,
    Bugfix,
    Refactor,
}

impl TaskCategory {
    pub fn requires_planning(self) -> bool {
        matches!(self, Self::Feature | Self::Bugfix | Self::Refactor)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnderstandOutput {
    pub task_type: TaskCategory,
    pub reasoning: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoingOutput {
    pub summary: String,
    pub files_changed: Vec<String>,
    pub commit_message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GittingOutput {
    pub branch: String,
    pub pr_number: u64,
    pub pr_url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReviewVerdict {
    Approve,
    NeedsChanges,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewingOutput {
    pub verdict: ReviewVerdict,
    pub suggestions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergingOutput {
    pub merged: bool,
    pub merge_sha: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsmSnapshot {
    pub state: WorkerState,
    pub category: Option<TaskCategory>,
    pub doing_turns: u32,
    pub review_loops: u32,
    pub failure_reason: Option<String>,
}

impl Default for FsmSnapshot {
    fn default() -> Self {
        Self {
            state: WorkerState::Understand,
            category: None,
            doing_turns: 0,
            review_loops: 0,
            failure_reason: None,
        }
    }
}

impl FsmSnapshot {
    pub fn transition(&mut self, next: WorkerState) -> Result<(), GardenerError> {
        let from = self.state;
        validate_transition(from, next)?;
        append_run_log(
            "info",
            "fsm.transition",
            json!({
                "from": from.as_str(),
                "to": next.as_str(),
                "doing_turns": self.doing_turns,
                "review_loops": self.review_loops
            }),
        );
        self.state = next;
        Ok(())
    }

    pub fn apply_understand(&mut self, output: &UnderstandOutput) -> Result<(), GardenerError> {
        if self.state != WorkerState::Understand {
            append_run_log(
                "error",
                "fsm.apply_understand.invalid_state",
                json!({
                    "current_state": self.state.as_str()
                }),
            );
            return Err(GardenerError::InvalidConfig(
                "understand output can only be applied in UNDERSTAND state".to_string(),
            ));
        }
        self.category = Some(output.task_type);
        let requires_planning = output.task_type.requires_planning();
        let next = if requires_planning {
            WorkerState::Planning
        } else {
            WorkerState::Doing
        };
        append_run_log(
            "info",
            "fsm.understand.applied",
            json!({
                "task_type": format!("{:?}", output.task_type),
                "requires_planning": requires_planning,
                "next_state": next.as_str()
            }),
        );
        self.transition(next)
    }

    pub fn on_doing_turn_completed(&mut self) -> Result<(), GardenerError> {
        self.doing_turns = self.doing_turns.saturating_add(1);
        append_run_log(
            "debug",
            "fsm.doing.turn_completed",
            json!({
                "doing_turns": self.doing_turns,
                "max": MAX_DOING_TURNS
            }),
        );
        if self.doing_turns > MAX_DOING_TURNS {
            let reason = "doing turn limit exceeded (100)".to_string();
            append_run_log(
                "warn",
                "fsm.doing.turn_limit_exceeded",
                json!({
                    "doing_turns": self.doing_turns,
                    "limit": MAX_DOING_TURNS,
                    "reason": reason
                }),
            );
            self.failure_reason = Some(reason);
            self.transition(WorkerState::Parked)?;
        }
        Ok(())
    }

    pub fn on_review_loop_back(&mut self) -> Result<(), GardenerError> {
        self.review_loops = self.review_loops.saturating_add(1);
        append_run_log(
            "debug",
            "fsm.review.loop_back",
            json!({
                "review_loops": self.review_loops,
                "max": MAX_REVIEW_LOOPS
            }),
        );
        if self.review_loops > MAX_REVIEW_LOOPS {
            let reason = "review loop cap exceeded (3)".to_string();
            append_run_log(
                "warn",
                "fsm.review.loop_cap_exceeded",
                json!({
                    "review_loops": self.review_loops,
                    "limit": MAX_REVIEW_LOOPS,
                    "reason": reason
                }),
            );
            self.failure_reason = Some(reason);
            self.transition(WorkerState::Parked)?;
        }
        Ok(())
    }
}

pub fn validate_transition(from: WorkerState, to: WorkerState) -> Result<(), GardenerError> {
    use WorkerState as S;

    let allowed = match from {
        S::Understand => matches!(to, S::Planning | S::Doing | S::Failed | S::Parked),
        S::Planning => matches!(to, S::Doing | S::Failed | S::Parked),
        S::Doing => matches!(to, S::Gitting | S::Failed | S::Parked),
        S::Gitting => matches!(to, S::Reviewing | S::Failed | S::Parked),
        S::Reviewing => matches!(to, S::Doing | S::Merging | S::Failed | S::Parked),
        S::Merging => matches!(to, S::Complete | S::Failed | S::Parked),
        S::Complete | S::Failed | S::Parked | S::Seeding => false,
    };

    if !allowed {
        append_run_log(
            "error",
            "fsm.transition.invalid",
            json!({
                "from": from.as_str(),
                "to": to.as_str()
            }),
        );
        return Err(GardenerError::InvalidConfig(format!(
            "illegal transition: {:?} -> {:?}",
            from, to
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planning_skip_mapping_is_deterministic() {
        for category in [TaskCategory::Task, TaskCategory::Chore, TaskCategory::Infra] {
            let mut fsm = FsmSnapshot::default();
            fsm.apply_understand(&UnderstandOutput {
                task_type: category,
                reasoning: "x".to_string(),
            })
            .expect("skip planning");
            assert_eq!(fsm.state, WorkerState::Doing);
        }

        for category in [
            TaskCategory::Feature,
            TaskCategory::Bugfix,
            TaskCategory::Refactor,
        ] {
            let mut fsm = FsmSnapshot::default();
            fsm.apply_understand(&UnderstandOutput {
                task_type: category,
                reasoning: "x".to_string(),
            })
            .expect("requires planning");
            assert_eq!(fsm.state, WorkerState::Planning);
        }
    }

    #[test]
    fn transition_validator_rejects_invalid_edges() {
        let err = validate_transition(WorkerState::Understand, WorkerState::Merging)
            .expect_err("must reject");
        assert!(
            matches!(err, GardenerError::InvalidConfig(message) if message.contains("illegal transition"))
        );
    }

    #[test]
    fn turn_and_review_caps_park_the_worker() {
        let mut fsm = FsmSnapshot {
            state: WorkerState::Doing,
            ..FsmSnapshot::default()
        };

        for _ in 0..MAX_DOING_TURNS {
            fsm.on_doing_turn_completed().expect("turn ok");
            assert_eq!(fsm.state, WorkerState::Doing);
        }

        fsm.on_doing_turn_completed().expect("parked");
        assert_eq!(fsm.state, WorkerState::Parked);

        let mut review = FsmSnapshot {
            state: WorkerState::Reviewing,
            ..FsmSnapshot::default()
        };

        for _ in 0..MAX_REVIEW_LOOPS {
            review.on_review_loop_back().expect("loop ok");
            assert_eq!(review.state, WorkerState::Reviewing);
        }

        review.on_review_loop_back().expect("parked");
        assert_eq!(review.state, WorkerState::Parked);
        assert!(review
            .failure_reason
            .as_deref()
            .unwrap_or_default()
            .contains("review loop cap"));
    }
}
