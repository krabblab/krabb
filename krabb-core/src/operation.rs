use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Error, Result, types::ToolResult};

pub const LIST: &str = "operation.list";
pub const START: &str = "operation.start";
pub const INSPECT: &str = "operation.inspect";
pub const CANCEL: &str = "operation.cancel";
pub const RESULT: &str = "operation.result";
pub const LIMIT: usize = 256 * 1024;
const PAYLOAD_LIMIT: usize = 64 * 1024;
pub const HANDLE_LIMIT: usize = 4_096;
pub const DETAIL_LIMIT: usize = 4_096;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub name: String,
    pub description: String,
    pub input: Value,
    pub cancel: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Start {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Query {
    pub name: String,
    pub handle: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Pending,
    Running,
    Waiting,
    Completed,
    Failed,
    Cancelled,
    Unknown,
}

impl State {
    pub fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub state: State,
    pub handle: Option<String>,
    pub result: Option<ToolResult>,
    pub detail: Option<String>,
}

fn identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn invalid() -> Error {
    Error::Protocol("Invalid or oversized operation contract.".into())
}

impl Spec {
    pub fn validate(&self) -> Result<()> {
        if !identity(&self.name)
            || self.description.len() > 2_048
            || self.input["type"] != "object"
            || !self.input["properties"].is_object()
            || serde_json::to_vec(&self.input)?.len() > PAYLOAD_LIMIT
        {
            return Err(invalid());
        }

        Ok(())
    }
}

impl Start {
    pub fn validate(&self) -> Result<()> {
        if !identity(&self.id)
            || !identity(&self.name)
            || !self.arguments.is_object()
            || serde_json::to_vec(&self.arguments)?.len() > PAYLOAD_LIMIT
        {
            return Err(invalid());
        }

        Ok(())
    }
}

impl Query {
    pub fn validate(&self) -> Result<()> {
        if !identity(&self.name) || self.handle.is_empty() || self.handle.len() > HANDLE_LIMIT {
            return Err(invalid());
        }

        Ok(())
    }
}

impl Receipt {
    pub fn validate(&self) -> Result<()> {
        if self
            .handle
            .as_ref()
            .is_some_and(|handle| handle.is_empty() || handle.len() > HANDLE_LIMIT)
            || (!self.state.terminal() && self.state != State::Unknown && self.handle.is_none())
            || (self.state == State::Completed && self.result.is_none())
            || (!self.state.terminal() && self.result.is_some())
            || self.detail.as_ref().is_some_and(|detail| detail.len() > DETAIL_LIMIT)
            || serde_json::to_vec(self)?.len() > LIMIT
        {
            return Err(invalid());
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_work_requires_a_handle_and_completion_requires_a_result() {
        let mut receipt = Receipt {
            state: State::Running,
            handle: Some("opaque-handle".into()),
            result: None,
            detail: None,
        };

        receipt.validate().unwrap();

        assert_eq!(
            serde_json::from_value::<Receipt>(serde_json::to_value(&receipt).unwrap()).unwrap(),
            receipt
        );

        receipt.handle = None;

        assert!(receipt.validate().is_err());

        receipt.state = State::Completed;

        assert!(receipt.validate().is_err());

        receipt.result = Some(ToolResult::text("Saved artifact"));
        receipt.validate().unwrap();

        assert!(receipt.state.terminal());

        receipt.detail = Some("x".repeat(4_097));

        assert!(receipt.validate().is_err());
    }

    #[test]
    fn requests_are_bounded_and_cannot_carry_unrecognized_authority() {
        let start = Start {
            id: "task-1-operation-1".into(),
            name: "delegate".into(),
            arguments: serde_json::json!({"prompt":"Prepare the report"}),
        };

        start.validate().unwrap();

        assert!(Start { id: "../operation".into(), ..start.clone() }.validate().is_err());

        assert!(
            Start { arguments: serde_json::json!({"prompt":"x".repeat(64 * 1024)}), ..start }
                .validate()
                .is_err()
        );

        assert!(
            serde_json::from_value::<Query>(serde_json::json!({
                "name":"delegate", "handle":"opaque", "owner":"someone-else"
            }))
            .is_err()
        );

        assert!(Query { name: "delegate".into(), handle: "x".repeat(4_097) }.validate().is_err());
    }
}
