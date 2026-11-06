use serde::{Deserialize, Serialize};

pub const DESTINATION_LIMIT: usize = 16;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Destination {
    pub channel: String,
    pub chat: String,
    pub private: bool,
    pub thread: Option<String>,
}

impl Destination {
    pub fn validate(&self) -> Result<(), String> {
        let identifier = |value: &str| {
            !value.is_empty()
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        };

        let address = |value: &str| {
            !value.trim().is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
        };

        if !identifier(&self.channel)
            || !address(&self.chat)
            || self.thread.as_deref().is_some_and(|value| !address(value))
        {
            return Err("Schedule destination is invalid.".into());
        }

        if self.channel == "tui"
            && (!self.chat.starts_with("tui-")
                || !identifier(&self.chat)
                || self.private
                || self.thread.is_some())
        {
            return Err("TUI destinations require a local session ID without a thread.".into());
        }

        Ok(())
    }
}

impl std::str::FromStr for Destination {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let parts: Vec<_> = value.splitn(4, ':').collect();

        if parts.len() < 3 || !matches!(parts[1], "private" | "group") {
            return Err("Use channel:private|group:chat[:thread] for each destination.".into());
        }

        let destination = Destination {
            channel: parts[0].into(),
            chat: parts[2].into(),
            private: parts[1] == "private",
            thread: parts.get(3).map(|value| (*value).to_owned()),
        };

        destination.validate()?;

        Ok(destination)
    }
}

pub fn validate_destinations(destinations: &[Destination]) -> Result<(), String> {
    if destinations.len() > DESTINATION_LIMIT {
        return Err(format!("A schedule supports at most {DESTINATION_LIMIT} destinations."));
    }

    for (index, destination) in destinations.iter().enumerate() {
        destination.validate()?;

        if destinations[..index].iter().any(|previous| {
            previous.channel == destination.channel
                && previous.chat == destination.chat
                && previous.thread == destination.thread
        }) {
            return Err("Schedule destinations must be distinct.".into());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destination_contract_is_bounded_and_unambiguous() {
        let destination = Destination {
            channel: "telegram".into(),
            chat: "123".into(),
            private: true,
            thread: None,
        };

        validate_destinations(std::slice::from_ref(&destination)).unwrap();

        assert_eq!(
            serde_json::from_value::<Destination>(serde_json::to_value(&destination).unwrap())
                .unwrap(),
            destination
        );

        assert!(validate_destinations(&vec![destination.clone(); DESTINATION_LIMIT + 1]).is_err());

        let mut same = destination.clone();

        same.private = false;

        assert!(validate_destinations(&[destination.clone(), same]).is_err());

        for (channel, chat, thread) in [
            ("../escape", "123", None),
            ("telegram", "\n123", None),
            ("telegram", "123", Some("")),
            ("tui", "wrong", None),
        ] {
            let value = Destination {
                channel: channel.into(),
                chat: chat.into(),
                private: false,
                thread: thread.map(str::to_owned),
            };

            assert!(value.validate().is_err());
        }
    }
}
