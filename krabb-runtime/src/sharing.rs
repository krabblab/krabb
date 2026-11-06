use super::*;

pub use krabb_core::scope::Principal;
pub use krabb_core::visibility::{Data, Rule};

pub fn principal(channel: &str) -> Principal {
    if channel == "tui" {
        Principal::Client(channel.into())
    } else {
        Principal::Channel(channel.into())
    }
}

pub fn session_principal(session: &state::Session) -> Option<Principal> {
    if session.scheduled {
        return None;
    }

    if let Some(channel) = &session.channel {
        return Some(principal(channel));
    }

    session
        .messages
        .iter()
        .any(|message| message.sender.as_deref() == Some("tui"))
        .then(|| Principal::Client("tui".into()))
}

pub fn isolated(config: &Config, principal: &Principal) -> bool {
    match principal {
        Principal::Channel(id) => config.channels.get(id).is_some_and(|value| value.worktree),
        Principal::Client(id) => config.clients.get(id).is_some_and(|value| value.worktree),
        Principal::Agent(_) => false,
    }
}

pub fn configured(config: &Config, principal: &Principal) -> bool {
    match principal {
        Principal::Channel(id) => config.channels.contains_key(id),
        Principal::Client(id) => id == "tui" || config.clients.contains_key(id),
        Principal::Agent(_) => false,
    }
}

pub fn rules<'a>(config: &'a Config, source: &Principal) -> &'a [Rule] {
    let local = match source {
        Principal::Channel(id) => {
            config.channels.get(id).and_then(|value| value.visibility.as_deref())
        }

        Principal::Client(id) => {
            config.clients.get(id).and_then(|value| value.visibility.as_deref())
        }

        Principal::Agent(_) => None,
    };

    local.unwrap_or(&config.visibility)
}

pub fn sources(config: &Config, recipient: &Principal, data: Data) -> Vec<Principal> {
    if isolated(config, recipient) {
        return Vec::new();
    }

    config
        .channels
        .keys()
        .cloned()
        .map(Principal::Channel)
        .chain(config.clients.keys().cloned().map(Principal::Client))
        .chain(std::iter::once(Principal::Client("tui".into())))
        .chain(std::iter::once(recipient.clone()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|source| {
            !isolated(config, source)
                && (source == recipient
                    || rules(config, source).iter().any(|rule| rule.grants(data, recipient)))
        })
        .collect()
}

pub fn approvals(config: &Config, recipient: &Principal) -> Vec<Principal> {
    if isolated(config, recipient) {
        return Vec::new();
    }

    sources(config, recipient, Data::Approvals)
        .into_iter()
        .filter(|source| {
            source != recipient
                || rules(config, source).iter().any(|rule| rule.grants(Data::Approvals, recipient))
        })
        .collect()
}

pub fn workspace(root: &Path) -> Result<Option<PathBuf>, PluginError> {
    Ok(Some(std::fs::canonicalize(root)?))
}

pub fn patterns(source: &Principal) -> Vec<String> {
    match source {
        Principal::Channel(id) => vec![format!("provider:{id}:*")],
        Principal::Client(id) if id == "tui" => vec!["tui:*".into(), "provider:tui:*".into()],
        Principal::Client(id) => vec![format!("client:{id}:*")],
        Principal::Agent(id) => vec![format!("agent:{id}:*")],
    }
}

pub fn params(config: &Config, channel: &str, own: &str, isolated: bool, data: Data) -> Value {
    let mut visibility = vec![own.to_owned()];

    if let Some(id) = own.strip_prefix("tui:") {
        visibility.push(format!("provider:tui:{id}"));
    }

    if !isolated {
        visibility.push("global".into());
        visibility.extend(sources(config, &principal(channel), data).iter().flat_map(patterns));
    }

    serde_json::json!({"scope":own,"visibility":visibility,"excluded":[]})
}

pub fn memory_params(config: &Config, channel: &str, own: &str, isolated: bool) -> Value {
    params(config, channel, own, isolated, Data::Memory)
}

pub fn visible(params: &Value, scope: &str) -> bool {
    let matches = |key: &str| {
        params[key].as_array().into_iter().flatten().filter_map(Value::as_str).any(|value| {
            value.strip_suffix('*').map_or(value == scope, |prefix| scope.starts_with(prefix))
        })
    };

    matches("visibility") && !matches("excluded")
}

pub fn approval(config: &Config, owner: &Principal, fallback: ApprovalMode) -> ApprovalMode {
    let mode = match owner {
        Principal::Channel(id) => config.channels.get(id).and_then(|p| p.approval.as_deref()),
        Principal::Client(id) => config.clients.get(id).and_then(|p| p.approval.as_deref()),
        Principal::Agent(_) => None,
    };

    match mode {
        Some("off") => ApprovalMode::Off,
        Some("auto") => ApprovalMode::Auto,
        Some("prompt") => ApprovalMode::Prompt,
        _ => fallback,
    }
}

pub fn access(
    config: &Config,
    channel: &str,
    session: &str,
    data: Data,
) -> Result<Value, PluginError> {
    use krabb_core::scope::{Access, Owner};
    let owner = principal(channel);

    let access = Access {
        sources: sources(config, &owner, data),
        owner: Owner { source: owner, conversation: session.into() },
        excluded: Vec::new(),
    };

    access.validate()?;
    Ok(serde_json::to_value(access)?)
}

pub fn schedule_access(
    config: &Config,
    channel: &str,
    session: &str,
) -> Result<Value, PluginError> {
    access(config, channel, session, Data::Schedules)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_and_schedule_grants_preserve_direction_and_worktree_exclusion() {
        use krabb_core::scope::{Access, Owner};

        let mut config: Config = toml::from_str(r#"
            [channels.telegram]
            visibility = [{ data = ["approvals", "permissions", "schedules"], resources = ["channels.discord"] }]
            [channels.discord]
        "#).unwrap();

        let telegram = principal("telegram");
        let discord = principal("discord");

        assert_eq!(approvals(&config, &discord), vec![telegram.clone()]);
        assert!(approvals(&config, &telegram).is_empty());
        assert_eq!(sources(&config, &discord, Data::Memory), vec![discord.clone()]);

        let access: Access =
            serde_json::from_value(schedule_access(&config, "discord", "d").unwrap()).unwrap();

        assert!(access.allows(&Owner { source: telegram.clone(), conversation: "t".into() }));

        let access: Access =
            serde_json::from_value(schedule_access(&config, "telegram", "t").unwrap()).unwrap();

        assert!(!access.allows(&Owner { source: discord.clone(), conversation: "d".into() }));

        config.channels.get_mut("telegram").unwrap().worktree = true;

        assert!(approvals(&config, &discord).is_empty());
        assert!(approvals(&config, &telegram).is_empty());

        config.channels.get_mut("telegram").unwrap().worktree = false;
        config.channels.get_mut("discord").unwrap().worktree = true;

        assert!(approvals(&config, &discord).is_empty());
    }

    #[test]
    fn grants_export_to_recipients_and_local_rules_replace_defaults() {
        let config: Config = toml::from_str(
            r#"
            [[visibility]]
            data = ["files", "context"]
            resources = ["clients"]
            [channels.discord]
            [channels.telegram]
            [[channels.telegram.visibility]]
            data = ["files"]
            resources = ["channels.discord"]
        "#,
        )
        .unwrap();

        config.validate().unwrap();

        let telegram = principal("telegram");
        let discord = principal("discord");
        let tui = principal("tui");

        assert!(sources(&config, &discord, Data::Files).contains(&telegram));
        assert!(!sources(&config, &telegram, Data::Files).contains(&discord));
        assert!(!sources(&config, &tui, Data::Files).contains(&telegram));
        assert!(sources(&config, &tui, Data::Context).contains(&discord));
        assert!(!sources(&config, &tui, Data::Context).contains(&telegram));

        let mut config = config;

        config.channels.get_mut("telegram").unwrap().worktree = true;

        assert!(!sources(&config, &discord, Data::Files).contains(&telegram));
        assert!(sources(&config, &telegram, Data::Files).is_empty());

        config.clients.entry("tui".into()).or_default().visibility = Some(Vec::new());

        assert_eq!(rules(&config, &tui), &[]);
    }
}
