use super::{ChannelConfig, Config};
use std::collections::BTreeMap;

type Layer<'a> = (Option<&'a str>, Option<&'a str>);

#[derive(Clone, Debug)]
pub(super) struct Policy {
    default: (String, String),
    channels: BTreeMap<String, (String, String)>,
}

impl Policy {
    pub fn new(config: &Config) -> Self {
        Self {
            default: current(config, (None, None), (None, None)),
            channels: config
                .channels
                .iter()
                .map(|(id, policy)| {
                    (
                        id.clone(),
                        current(
                            config,
                            (policy.provider.as_deref(), policy.model.as_deref()),
                            (None, None),
                        ),
                    )
                })
                .collect(),
        }
    }

    pub fn channel(&self, id: &str) -> (String, String) {
        self.channels.get(id).unwrap_or(&self.default).clone()
    }
}

pub(super) fn reloadable(key: &str) -> bool {
    matches!(
        key.split('.').collect::<Vec<_>>().as_slice(),
        ["provider" | "model"] | ["channels", _, "provider" | "model"]
    )
}

pub(super) fn resolve(
    config: &Config,
    scope: Layer<'_>,
    env: Layer<'_>,
    args: Layer<'_>,
) -> (String, String) {
    let mut provider = "codex";
    let mut model = "default";

    for (next_provider, next_model) in
        [(config.provider.as_deref(), config.model.as_deref()), scope, env, args]
    {
        if let Some(next) = next_provider {
            if next != provider {
                model = "default";
            }

            provider = next;
        }

        if let Some(next) = next_model {
            model = next;
        }
    }

    (provider.into(), model.into())
}

pub(super) fn current(config: &Config, scope: Layer<'_>, args: Layer<'_>) -> (String, String) {
    let provider = std::env::var("KRABB_MODEL_PLUGIN").ok();
    let model = std::env::var("KRABB_MODEL").ok();

    resolve(config, scope, (provider.as_deref(), model.as_deref()), args)
}

pub(super) fn client(config: &Config, id: &str, args: Layer<'_>) -> (String, String) {
    let scope = config
        .clients
        .get(id)
        .map(|client| (client.provider.as_deref(), client.model.as_deref()))
        .unwrap_or_default();

    current(config, scope, args)
}

pub(super) fn tui(config: &Config, args: &[String]) -> (String, String) {
    let mut provider = None;
    let mut model = None;

    for pair in args.chunks_exact(2) {
        match pair[0].as_str() {
            "--plugin" => provider = Some(pair[1].as_str()),
            "--model" => model = Some(pair[1].as_str()),

            _ => {}
        }
    }

    client(config, "tui", (provider, model))
}

pub(super) fn channels(config: &Config) -> BTreeMap<String, ChannelConfig> {
    config
        .channels
        .iter()
        .map(|(id, policy)| {
            let (provider, model) = current(
                config,
                (policy.provider.as_deref(), policy.model.as_deref()),
                (None, None),
            );
            let mut policy = policy.clone();
            policy.provider = Some(provider);
            policy.model = Some(model);

            (id.clone(), policy)
        })
        .collect()
}

pub(super) fn validate(provider: Option<&str>, model: Option<&str>) -> Result<(), String> {
    if provider.is_some_and(|provider| !super::valid(provider)) {
        return Err("Provider IDs must contain lowercase letters, digits, or hyphens.".into());
    }

    if model.is_some_and(|model| {
        model.trim().is_empty() || model.len() > 256 || model.chars().any(char::is_control)
    }) {
        return Err("Models must contain 1–256 visible characters.".into());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Layer<'_> = (None, None);

    #[test]
    fn only_channel_and_global_selections_are_reloadable() {
        for key in ["model", "provider", "channels.telegram.model", "channels.discord.provider"] {
            assert!(reloadable(key));
        }

        for key in
            ["channels.telegram.tools", "clients.tui.model", "channels", "tools", "model.extra"]
        {
            assert!(!reloadable(key));
        }
    }

    fn configured() -> Config {
        toml::from_str("provider = 'openai'\nmodel = 'global-model'").unwrap()
    }

    #[test]
    fn arguments_override_environment_scope_and_global_configuration() {
        let config = configured();
        let scope = (Some("openrouter"), Some("channel-model"));
        let env = (Some("ollama"), Some("environment-model"));
        let args = (Some("codex"), Some("argument-model"));

        for (scope, env, args, expected) in [
            (NONE, NONE, NONE, ("openai", "global-model")),
            (scope, NONE, NONE, ("openrouter", "channel-model")),
            (scope, env, NONE, ("ollama", "environment-model")),
            (scope, env, args, ("codex", "argument-model")),
            (scope, env, (None, Some("argument-model")), ("ollama", "argument-model")),
        ] {
            assert_eq!(resolve(&config, scope, env, args), (expected.0.into(), expected.1.into()));
        }

        assert_eq!(
            resolve(&Config::default(), NONE, NONE, NONE),
            ("codex".into(), "default".into())
        );
    }

    #[test]
    fn provider_changes_discard_lower_priority_models() {
        let config = configured();

        for (scope, env, args, expected) in [
            ((Some("ollama"), None), NONE, NONE, ("ollama", "default")),
            (NONE, (Some("ollama"), None), NONE, ("ollama", "default")),
            (NONE, NONE, (Some("ollama"), None), ("ollama", "default")),
            (NONE, (Some("openai"), None), NONE, ("openai", "global-model")),
            (NONE, (None, Some("env-model")), (Some("ollama"), None), ("ollama", "default")),
            ((Some("ollama"), None), (None, Some("env-model")), NONE, ("ollama", "env-model")),
        ] {
            assert_eq!(resolve(&config, scope, env, args), (expected.0.into(), expected.1.into()));
        }
    }

    #[test]
    fn configuration_roundtrips_and_validates_each_scope() {
        let text = "provider = 'openai'\nmodel = 'global-model'\n[clients.tui]\nprovider = 'codex'\nmodel = 'tui-model'\n[clients.cli]\nprovider = 'ollama'\n[channels.whatsapp]\nmodel = 'channel-model'";
        let config: Config = toml::from_str(text).unwrap();
        config.validate().unwrap();
        let roundtrip: Config = toml::from_str(&toml::to_string(&config).unwrap()).unwrap();

        assert_eq!(roundtrip.provider.as_deref(), Some("openai"));
        assert_eq!(roundtrip.model.as_deref(), Some("global-model"));
        assert_eq!(roundtrip.clients["tui"].model.as_deref(), Some("tui-model"));
        assert_eq!(roundtrip.clients["cli"].provider.as_deref(), Some("ollama"));

        for scope in ["", "[clients.tui]\n", "[clients.cli]\n", "[channels.telegram]\n"] {
            for invalid in [
                "provider = '../bad'",
                "provider = ''",
                "model = ''",
                "model = '   '",
                "model = \"bad\\nmodel\"",
            ] {
                let config: Config = toml::from_str(&format!("{scope}{invalid}")).unwrap();

                assert!(config.validate().is_err(), "{scope}{invalid}");
            }
        }

        assert!(validate(None, Some(&"x".repeat(257))).is_err());
    }

    #[test]
    fn channel_and_client_entrypoints_apply_the_shared_policy() {
        let config: Config = toml::from_str("provider = 'openai'\nmodel = 'global-model'\n[channels.telegram]\nprovider = 'ollama'\n[clients.tui]\nmodel = 'tui-model'\n[clients.cli]\nmodel = 'cli-model'").unwrap();
        let env_provider = std::env::var("KRABB_MODEL_PLUGIN").ok();
        let env_model = std::env::var("KRABB_MODEL").ok();
        let env = (env_provider.as_deref(), env_model.as_deref());
        let policies = channels(&config);
        let policy = &policies["telegram"];
        let expected = resolve(&config, (Some("ollama"), None), env, NONE);

        assert_eq!((policy.provider.clone().unwrap(), policy.model.clone().unwrap()), expected);
        assert_eq!(
            client(&config, "tui", NONE),
            resolve(&config, (None, Some("tui-model")), env, NONE)
        );

        assert_eq!(
            client(&config, "cli", NONE),
            resolve(&config, (None, Some("cli-model")), env, NONE)
        );

        assert_eq!(client(&config, "absent", NONE), resolve(&config, NONE, env, NONE));

        let args = [
            "--plugin", "openai", "--once", "--plugin", "--plugin", "codex", "--model", "explicit",
        ]
        .map(String::from);

        assert_eq!(tui(&config, &args), ("codex".into(), "explicit".into()));
    }
}
