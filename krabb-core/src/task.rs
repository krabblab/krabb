pub mod defer;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const COMMAND_LIMIT: usize = 64;
pub const INSTRUCTION_LIMIT: usize = 64 * 1024;

/// Exact, language-neutral task command prefix; all other text is interpreted by the model.
pub const PREFIX: &str = "/task";

/// Whether `value` is a task ID; earlier releases saved tasks as `job-<digits>`.
pub fn id(value: &str) -> bool {
    value.strip_prefix("task-").or_else(|| value.strip_prefix("job-")).is_some_and(|digits| {
        !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
    })
}

pub fn command(text: &str) -> bool {
    let text = text.trim();

    text == PREFIX
        || text.strip_prefix(PREFIX).is_some_and(|rest| rest.starts_with(char::is_whitespace))
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Queued,
    Running,
    Waiting,
    Pausing,
    Paused,
    Blocked,
    Cancelling,
    Cancelled,
    Reporting,
    Completed,
    Failed,
}

impl State {
    pub fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled | Self::Failed)
    }

    pub fn stopping(self) -> bool {
        matches!(self, Self::Pausing | Self::Cancelling)
    }

    pub fn request(self, action: Action) -> Result<Self, ControlError> {
        use Action::*;
        use State::*;

        match (self, action) {
            (Completed | Reporting, Cancel) => Ok(self),
            (Cancelled, Cancel) => Ok(Cancelled),
            (Completed | Reporting | Cancelled | Failed, _) => Err(ControlError::Terminal),
            (Running | Pausing, Pause | Steer) => Ok(Pausing),
            (Cancelling, Cancel) => Ok(Cancelling),
            (Cancelling, _) => Err(ControlError::Stopping),
            (Running | Pausing, Cancel) => Ok(Cancelling),
            (_, Cancel) => Ok(Cancelled),
            (Queued | Waiting | Blocked | Paused, Pause) => Ok(Paused),
            (Paused | Blocked, Resume) => Ok(Queued),
            (_, Resume) => Err(ControlError::NotPaused),
            (Blocked, Approve | Deny) => Ok(Queued),
            (_, Approve | Deny) => Err(ControlError::Approval),
            (_, Steer) => Ok(self),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Pause,
    Resume,
    Cancel,
    Steer,
    Approve,
    Deny,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Command {
    pub id: String,
    pub action: Action,
    pub revision: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub instruction: String,
}

impl Command {
    pub fn validate(&self) -> Result<(), ControlError> {
        if self.id.is_empty()
            || self.id.len() > COMMAND_LIMIT
            || !self.id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            || self.instruction.len() > INSTRUCTION_LIMIT
        {
            return Err(ControlError::Invalid);
        }

        if self.action == Action::Steer && self.instruction.trim().is_empty() {
            return Err(ControlError::Instruction);
        }

        if matches!(self.action, Action::Pause | Action::Cancel | Action::Approve | Action::Deny)
            && !self.instruction.is_empty()
        {
            return Err(ControlError::Invalid);
        }

        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub command: Command,
    pub revision: u64,
    pub state: State,
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum ControlError {
    #[error("The task command is invalid or exceeds its limits.")]
    Invalid,
    #[error("Include the requested change before steering this task.")]
    Instruction,
    #[error("The task has already finished.")]
    Terminal,
    #[error("The task is still stopping; wait for acknowledgement before resuming.")]
    Stopping,
    #[error("Only paused or blocked tasks can resume.")]
    NotPaused,
    #[error("The task changed; refresh its status before applying this command.")]
    Stale,
    #[error("This command ID was already used with different arguments.")]
    Conflict,
    #[error("An action's outcome is uncertain; review it before resuming work.")]
    Uncertain,
    #[error("The accepted request has conflicting task records. Review them before continuing.")]
    Identity,
    #[error("The saved task input is missing. Restore it before continuing.")]
    Input,
    #[error("This task has no pending approval to decide.")]
    Approval,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Run {
    pub id: String,
    pub generation: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_exact_prefix_is_a_task_command() {
        assert!(command("/task"));
        assert!(command("  /task pause task-3 "));
        assert!(command("/task\tpause task-3"));
        assert!(command("/task\u{a0}pause task-3"));
        assert!(!command("/tasks"));
        assert!(!command("pause the report"));
        assert!(!command("metti in pausa /task"));
    }

    #[test]
    fn task_ids_are_digits_after_the_current_or_earlier_prefix() {
        assert!(id("task-42"));
        assert!(id("job-42"));
        assert!(!id("task-"));
        assert!(!id("task-4a"));
        assert!(!id("schedule-42"));
        assert!(!id("../task-42"));
    }

    #[test]
    fn stop_requests_do_not_claim_an_active_operation_has_finished() {
        assert_eq!(State::Running.request(Action::Pause), Ok(State::Pausing));
        assert_eq!(State::Running.request(Action::Cancel), Ok(State::Cancelling));
        assert_eq!(State::Pausing.request(Action::Resume), Err(ControlError::NotPaused));
        assert_eq!(State::Cancelling.request(Action::Resume), Err(ControlError::Stopping));
        assert_eq!(State::Reporting.request(Action::Cancel), Ok(State::Reporting));
    }

    #[test]
    fn every_action_preserves_terminal_tasks_and_requires_explicit_resume() {
        for state in [State::Queued, State::Waiting, State::Paused, State::Blocked] {
            assert_eq!(state.request(Action::Pause), Ok(State::Paused));
            assert_eq!(state.request(Action::Cancel), Ok(State::Cancelled));
            assert_eq!(state.request(Action::Steer), Ok(state));
        }

        for state in [State::Completed, State::Cancelled, State::Failed, State::Reporting] {
            for action in [Action::Pause, Action::Resume, Action::Steer] {
                assert_eq!(state.request(action), Err(ControlError::Terminal));
            }
        }

        assert_eq!(State::Paused.request(Action::Resume), Ok(State::Queued));
        assert_eq!(State::Blocked.request(Action::Resume), Ok(State::Queued));
    }

    #[test]
    fn approval_decisions_apply_only_to_blocked_tasks_without_instructions() {
        for action in [Action::Approve, Action::Deny] {
            assert_eq!(State::Blocked.request(action), Ok(State::Queued));

            for state in [State::Queued, State::Running, State::Waiting, State::Paused] {
                assert_eq!(state.request(action), Err(ControlError::Approval));
            }

            assert_eq!(State::Pausing.request(action), Err(ControlError::Approval));
            assert_eq!(State::Cancelling.request(action), Err(ControlError::Stopping));

            for state in [State::Completed, State::Cancelled, State::Failed, State::Reporting] {
                assert_eq!(state.request(action), Err(ControlError::Terminal));
            }

            let command =
                Command { id: "decide-1".into(), action, revision: 2, instruction: String::new() };

            assert!(command.validate().is_ok());

            assert_eq!(
                Command { instruction: "run something else".into(), ..command.clone() }.validate(),
                Err(ControlError::Invalid)
            );

            let encoded = serde_json::to_value(&command).unwrap();

            assert_eq!(serde_json::from_value::<Command>(encoded).unwrap(), command);
        }

        assert_eq!(serde_json::to_value(Action::Approve).unwrap(), "approve");
        assert_eq!(serde_json::to_value(Action::Deny).unwrap(), "deny");
    }

    #[test]
    fn command_contract_round_trips_and_rejects_unrecognized_authority() {
        let command = Command {
            id: "request-42".into(),
            action: Action::Steer,
            revision: 3,
            instruction: "Use the corrected dataset.".into(),
        };

        let receipt = Receipt { command: command.clone(), revision: 4, state: State::Pausing };
        let encoded = serde_json::to_value(&receipt).unwrap();

        assert_eq!(serde_json::from_value::<Receipt>(encoded).unwrap(), receipt);
        assert!(command.validate().is_ok());

        assert!(
            serde_json::from_value::<Command>(serde_json::json!({
                "id":"x", "action":"resume", "owner":"someone-else"
            }))
            .is_err()
        );

        for id in ["", "../request", "request\n", &"x".repeat(COMMAND_LIMIT + 1)] {
            assert!(Command { id: id.into(), ..command.clone() }.validate().is_err());
        }

        assert_eq!(
            Command { instruction: " ".into(), ..command.clone() }.validate(),
            Err(ControlError::Instruction)
        );

        assert!(
            Command { instruction: "x".repeat(INSTRUCTION_LIMIT + 1), ..command }
                .validate()
                .is_err()
        );
    }
}
