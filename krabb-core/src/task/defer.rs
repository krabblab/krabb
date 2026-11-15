//! One-off continuation intent, separate from recurring scheduling.

use super::{COMMAND_LIMIT, ControlError, State};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct When {
    pub at: u64,
    pub timezone: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expression: Option<String>,
}

impl When {
    pub fn validate(&self) -> Result<(), ControlError> {
        if self.at == 0
            || self.at > i64::MAX as u64
            || self.timezone.is_empty()
            || self.timezone.len() > 128
            || self.expression.as_ref().is_some_and(|expression| {
                expression.trim().is_empty()
                    || expression.len() > 128
                    || expression.chars().any(char::is_control)
            })
            || !self.timezone.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(byte, b'/' | b'_' | b'-' | b'+' | b':' | b'.')
            })
        {
            return Err(ControlError::Invalid);
        }

        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Command {
    pub id: String,
    pub revision: u64,
    pub when: When,
}

impl Command {
    pub fn validate(&self) -> Result<(), ControlError> {
        if self.id.is_empty()
            || self.id.len() > COMMAND_LIMIT
            || self.revision == u64::MAX
            || !self.id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(ControlError::Invalid);
        }

        self.when.validate()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub command: Command,
    pub revision: u64,
    pub state: State,
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    #[test]
    fn deferred_commands_round_trip_without_accepting_authority_or_unbounded_time() {
        let command = Command {
            id: "later-1".into(),
            revision: 1,
            when: When { at: 1_800_000_000, timezone: "Europe/Rome".into(), expression: None },
        };

        command.validate().unwrap();

        let wire = serde_json::to_value(&command).unwrap();

        assert_eq!(serde_json::from_value::<Command>(wire.clone()).unwrap(), command);

        let mut forged = wire;

        forged["owner"] = json!("admin");

        assert!(serde_json::from_value::<Command>(forged).is_err());

        for when in [
            When { at: 0, timezone: "UTC".into(), expression: None },
            When { at: u64::MAX, timezone: "UTC".into(), expression: None },
            When { at: 1, timezone: "".into(), expression: None },
            When { at: 1, timezone: "x".repeat(129), expression: None },
            When { at: 1, timezone: "UTC; resume everything".into(), expression: None },
            When { at: 1, timezone: "UTC".into(), expression: Some(" ".into()) },
            When { at: 1, timezone: "UTC".into(), expression: Some("x".repeat(129)) },
            When { at: 1, timezone: "UTC".into(), expression: Some("tomorrow\n09:00".into()) },
        ] {
            assert!(when.validate().is_err());
        }

        for id in ["", "bad id", &"x".repeat(COMMAND_LIMIT + 1)] {
            assert!(Command { id: id.into(), ..command.clone() }.validate().is_err());
        }
    }
}
