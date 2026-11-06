use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub enum Principal {
    Channel(String),
    Client(String),
    Agent(String),
}

impl TryFrom<String> for Principal {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let (kind, id) = value.split_once('.').ok_or("Use channels.<id> or clients.<id>.")?;

        if id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err("Sharing source ID is invalid.".into());
        }

        match kind {
            "channels" => Ok(Self::Channel(id.into())),
            "clients" => Ok(Self::Client(id.into())),
            "agents" => Ok(Self::Agent(id.into())),
            _ => Err("Use channels.<id>, clients.<id>, or agents.<id>.".into()),
        }
    }
}

impl From<Principal> for String {
    fn from(value: Principal) -> Self {
        match value {
            Principal::Channel(id) => format!("channels.{id}"),
            Principal::Client(id) => format!("clients.{id}"),
            Principal::Agent(id) => format!("agents.{id}"),
        }
    }
}

/// Stable ownership is independent of a resource's path or display name.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Owner {
    pub source: Principal,
    pub conversation: String,
}

/// Host-issued access to resources owned by a conversation or named principals.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Access {
    pub owner: Owner,
    #[serde(default)]
    pub sources: Vec<Principal>,
    #[serde(default)]
    pub excluded: Vec<Principal>,
}

impl Access {
    pub fn validate(&self) -> Result<(), String> {
        if self.owner.conversation.is_empty() || self.owner.conversation.len() > 4096 {
            return Err("Resource access requires a bounded conversation identity.".into());
        }

        if self.sources.len() > 256 || self.excluded.len() > 256 {
            return Err("Resource access exceeds the source limit.".into());
        }

        Ok(())
    }

    pub fn allows(&self, owner: &Owner) -> bool {
        self.validate().is_ok()
            && (self.owner == *owner
                || (self.sources.contains(&owner.source) && !self.excluded.contains(&owner.source)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_is_directional_and_excludes_isolated_sources() {
        let owner = Owner { source: Principal::Client("tui".into()), conversation: "first".into() };
        let mut access = Access { owner: owner.clone(), sources: vec![], excluded: vec![] };

        let remote =
            Owner { source: Principal::Channel("telegram".into()), conversation: "second".into() };

        assert!(access.allows(&owner));
        assert!(!access.allows(&remote));

        access.sources.push(remote.source.clone());

        assert!(access.allows(&remote));

        access.excluded.push(remote.source.clone());

        assert!(!access.allows(&remote));
        assert!(access.allows(&owner));

        access.owner.conversation.clear();

        assert!(!access.allows(&remote));
        assert!(access.validate().is_err());
    }
}
