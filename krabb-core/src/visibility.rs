use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::scope::Principal;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Data {
    Files,
    Context,
    Memory,
    #[serde(alias = "data")]
    Records,
    Permissions,
    Approvals,
    Schedules,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub enum Resource {
    Global,
    Isolated,
    Channels,
    Clients,
    Agents,
    Principal(Principal),
}

impl TryFrom<String> for Resource {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Ok(match value.as_str() {
            "global" => Self::Global,
            "isolated" => Self::Isolated,
            "channels" => Self::Channels,
            "clients" => Self::Clients,
            "agents" => Self::Agents,
            _ => Self::Principal(value.try_into()?),
        })
    }
}

impl From<Resource> for String {
    fn from(value: Resource) -> Self {
        match value {
            Resource::Global => "global".into(),
            Resource::Isolated => "isolated".into(),
            Resource::Channels => "channels".into(),
            Resource::Clients => "clients".into(),
            Resource::Agents => "agents".into(),
            Resource::Principal(value) => value.into(),
        }
    }
}

impl Resource {
    pub fn matches(&self, recipient: &Principal) -> bool {
        match self {
            Self::Global => true,
            Self::Isolated => false,
            Self::Channels => matches!(recipient, Principal::Channel(_)),
            Self::Clients => matches!(recipient, Principal::Client(_)),
            Self::Agents => matches!(recipient, Principal::Agent(_)),
            Self::Principal(value) => value == recipient,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub data: Vec<Data>,
    pub resources: Vec<Resource>,
}

impl Rule {
    pub fn grants(&self, data: Data, recipient: &Principal) -> bool {
        self.data.contains(&data)
            && self.resources.iter().any(|resource| resource.matches(recipient))
    }
}

pub fn validate(rules: &[Rule]) -> Result<(), String> {
    if rules.len() > 32 {
        return Err("Visibility supports at most 32 rules per source.".into());
    }

    let mut isolated = BTreeSet::new();
    let mut exported = BTreeSet::new();

    for rule in rules {
        if rule.data.is_empty()
            || rule.data.len() > 7
            || rule.data.iter().collect::<BTreeSet<_>>().len() != rule.data.len()
            || rule.resources.len() > 64
            || rule.resources.iter().collect::<BTreeSet<_>>().len() != rule.resources.len()
        {
            return Err(
                "Visibility needs distinct data kinds and at most 64 distinct resources.".into()
            );
        }

        if rule.resources.contains(&Resource::Isolated) && rule.resources.len() != 1 {
            return Err("Use isolated alone, or an empty resources list.".into());
        }

        if rule.resources.is_empty() || rule.resources == [Resource::Isolated] {
            isolated.extend(rule.data.iter().copied());
        } else {
            exported.extend(rule.data.iter().copied());
        }
    }

    if !isolated.is_disjoint(&exported) {
        return Err("A data kind cannot be both isolated and exported by the same policy.".into());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn selectors_are_typed_and_rules_reject_ambiguous_grants() {
        let telegram = Principal::Channel("telegram".into());
        let tui = Principal::Client("tui".into());
        let agent = Principal::Agent("foo".into());

        let rules: Vec<Rule> = serde_json::from_value(json!([
            {"data":["files","context"],"resources":["channels","agents.foo"]},
            {"data":["memory"],"resources":["clients"]}
        ]))
        .unwrap();

        validate(&rules).unwrap();

        assert!(rules[0].grants(Data::Files, &telegram));
        assert!(rules[0].grants(Data::Context, &agent));
        assert!(!rules[0].grants(Data::Files, &tui));
        assert!(rules[1].grants(Data::Memory, &tui));

        for value in [
            json!([{"data":[],"resources":["global"]}]),
            json!([{"data":["files"],"resources":["isolated","global"]}]),
            json!([{"data":["files","files"],"resources":[]}]),
            json!([{"data":["files"],"resources":[]},{"data":["files"],"resources":["global"]}]),
        ] {
            let rules: Vec<Rule> = serde_json::from_value(value).unwrap();

            assert!(validate(&rules).is_err());
        }

        for value in [
            "global",
            "isolated",
            "channels",
            "clients",
            "agents",
            "agents.foo",
            "channels.discord",
        ] {
            let resource: Resource = serde_json::from_value(json!(value)).unwrap();

            assert_eq!(serde_json::to_value(resource).unwrap(), json!(value));
        }

        assert!(serde_json::from_value::<Resource>(json!("agents.../escape")).is_err());

        assert!(
            serde_json::from_value::<Rule>(json!({"data":["files"],"resource":["global"]}))
                .is_err()
        );
    }
}
