use super::*;

pub use krabb_core::scope::Principal;
pub use krabb_core::visibility::{Data, Rule};

pub fn principal(channel: &str) -> Principal {
    if client::known(channel) {
        Principal::Client(channel.into())
    } else {
        Principal::Channel(channel.into())
    }
}

pub fn session_principal(session: &state::Session) -> Option<Principal> {
    if session.is_work() {
        return None;
    }

    if let Some(source) = &session.source {
        return Some(source.clone());
    }

    if let Some(channel) = &session.channel {
        return Some(principal(channel));
    }

    session
        .messages
        .iter()
        .any(|message| message.sender.as_deref() == Some(client::TUI))
        .then(|| Principal::Client(client::TUI.into()))
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
        Principal::Client(id) => client::known(id) || config.clients.contains_key(id),
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
        .chain(std::iter::once(Principal::Client(client::TUI.into())))
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
        Principal::Client(id) if id == client::TUI => vec!["tui:*".into(), "provider:tui:*".into()],

        Principal::Client(id) if client::known(id) => {
            vec![format!("client:{id}:*"), format!("provider:{id}:*")]
        }

        Principal::Client(id) => vec![format!("client:{id}:*")],
        Principal::Agent(id) => vec![format!("agent:{id}:*")],
    }
}

/// The scope a built-in client's conversation had before clients got their own scopes, when
/// built-in clients were recorded as channels.
pub fn legacy(scope: &str) -> Option<String> {
    if let Some(conversation) = scope.strip_prefix("tui:") {
        return Some(format!("provider:{}:{conversation}", client::TUI));
    }

    let (id, conversation) = scope.strip_prefix("client:")?.split_once(':')?;

    (client::known(id) && id != client::TUI).then(|| format!("provider:{id}:{conversation}"))
}

pub fn params(config: &Config, source: &Principal, own: &str, isolated: bool, data: Data) -> Value {
    let mut visibility = vec![own.to_owned()];

    visibility.extend(legacy(own));

    if !isolated {
        visibility.push("global".into());
        visibility.extend(sources(config, source, data).iter().flat_map(patterns));
    }

    serde_json::json!({"scope":own,"visibility":visibility,"excluded":[]})
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
    source: &Principal,
    session: &str,
    data: Data,
) -> Result<Value, PluginError> {
    use krabb_core::scope::{Access, Owner};
    let owner = source.clone();

    let access = Access {
        sources: sources(config, &owner, data),
        owner: Owner { source: owner, conversation: session.into() },
        excluded: Vec::new(),
    };

    access.validate()?;
    Ok(serde_json::to_value(access)?)
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

        let access: Access = serde_json::from_value(
            super::access(&config, &principal("discord"), "d", Data::Schedules).unwrap(),
        )
        .unwrap();

        assert!(access.allows(&Owner { source: telegram.clone(), conversation: "t".into() }));

        let access: Access = serde_json::from_value(
            super::access(&config, &principal("telegram"), "t", Data::Schedules).unwrap(),
        )
        .unwrap();

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
        assert!(!sources(&config, &tui, Data::Context).contains(&principal("cli")));

        let mut config = config;

        config.channels.get_mut("telegram").unwrap().worktree = true;

        assert!(!sources(&config, &discord, Data::Files).contains(&telegram));
        assert!(sources(&config, &telegram, Data::Files).is_empty());

        config.clients.entry("tui".into()).or_default().visibility = Some(Vec::new());

        assert_eq!(rules(&config, &tui), &[]);
    }

    #[test]
    fn built_in_clients_keep_reading_the_scopes_they_had_as_channels() {
        assert_eq!(legacy("tui:main").as_deref(), Some("provider:tui:main"));
        assert_eq!(legacy("client:cli:ask-1").as_deref(), Some("provider:cli:ask-1"));
        assert_eq!(legacy("client:web:one"), None);
        assert_eq!(legacy("provider:telegram:7"), None);
        assert!(patterns(&principal("cli")).contains(&"provider:cli:*".into()));

        let config = Config::default();
        let own = "client:cli:ask-1";
        let params = params(&config, &principal("cli"), own, true, Data::Memory);

        assert!(visible(&params, "provider:cli:ask-1"));
        assert!(!visible(&params, "provider:cli:other"));
    }
}
