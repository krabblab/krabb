use super::*;

#[derive(Clone)]
pub(super) struct Context {
    pub plugins: Plugins,
    pub sessions: Arc<Mutex<state::Store>>,
    pub stop: Arc<Stop>,
    pub cancels: Arc<Mutex<BTreeMap<String, Arc<Cancellation>>>>,
    pub policies: BTreeMap<String, ChannelConfig>,
    pub provider: String,
    pub model: String,
    pub approval_mode: ApprovalMode,
    pub home: PathBuf,
    pub media: PathBuf,
    pub approvals: Arc<AsyncMutex<approval::Gate>>,
}

fn selection<'a>(
    policy: &'a ChannelConfig,
    provider: &'a str,
    model: &'a str,
) -> (&'a str, &'a str) {
    let selected = policy.provider.as_deref().unwrap_or(provider);
    let fallback = if selected == provider { model } else { "default" };

    (selected, policy.model.as_deref().unwrap_or(fallback))
}

pub(super) async fn run(context: Context) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    type Outcome = Result<(), Box<dyn std::error::Error + Send + Sync>>;

    let mut workers = tokio::task::JoinSet::<(String, Outcome)>::new();
    let mut active = BTreeMap::<String, Arc<Stop>>::new();
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    let mut next_call = 1_u64 << 48;
    let mut failure = None;

    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            biased;
            _ = context.stop.notified() => break,
            _ = tokio::signal::ctrl_c() => break,

            completed = workers.join_next(), if !workers.is_empty() => {
                match completed {
                    Some(Ok((id, result))) => {
                        active.remove(&id);

                        if let Err(error) = result {
                            warn!(channel = id, error = %diagnostic(sentence(error.to_string())), "Channel worker stopped.");
                        }
                    }

                    Some(Err(error)) => {
                        failure = Some(error.to_string());
                        break;
                    }

                    None => {}
                }
            }

            _ = tick.tick() => {
                let loaded = context.plugins.all().await.into_iter()
                    .filter(|(_, plugin)| plugin.supports(Capability::Channel))
                    .map(|(id, _)| id)
                    .collect::<BTreeSet<_>>();

                for (id, stop) in &active {
                    if !loaded.contains(id) {
                        stop.signal();
                    }
                }

                for id in loaded {
                    if active.contains_key(&id) || active.len() >= 64 {
                        continue;
                    }

                    // Keep workers' cancellation IDs apart, including streaming subrequests.
                    let call = next_call;
                    next_call += 1_u64 << 40;

                    if next_call >= 1_u64 << 60 {
                        failure = Some("Channel request ID space is exhausted.".into());
                        break;
                    }

                    let stop = Arc::new(Stop::new());
                    active.insert(id.clone(), Arc::clone(&stop));
                    let context = context.clone();

                    workers.spawn(async move {
                        let policy = context.policies.get(&id).cloned().unwrap_or_default();
                        let (provider, model) = selection(&policy, &context.provider, &context.model);
                        let result = bridge_with_media(
                            &context.plugins,
                            &id,
                            provider,
                            model,
                            context.sessions,
                            stop,
                            context.cancels,
                            context.policies,
                            context.approval_mode,
                            false,
                            context.home.clone(),
                            workspace_root_at(&context.home),
                            context.media,
                            context.approvals,
                            call,
                        ).await;

                        (id, result)
                    });
                }

                if failure.is_some() {
                    break;
                }
            }
        }
    }

    for stop in active.values() {
        stop.signal();
    }

    while workers.join_next().await.is_some() {}

    match failure {
        Some(error) => Err(error.into()),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_models_without_leaking_provider_defaults() {
        let mut policy = ChannelConfig::default();

        assert_eq!(selection(&policy, "codex", "original"), ("codex", "original"));

        policy.provider = Some("other".into());

        assert_eq!(selection(&policy, "codex", "original"), ("other", "default"));

        policy.model = Some("chosen".into());

        assert_eq!(selection(&policy, "codex", "original"), ("other", "chosen"));
    }

    #[test]
    fn channel_configuration_validates_provider_and_model() {
        let config: Config = toml::from_str(
            "[channels.telegram]\nprovider = 'codex'\n[channels.whatsapp]\nprovider = 'openai'\nmodel = 'test-model'",
        ).unwrap();

        config.validate().unwrap();

        let roundtrip: Config = toml::from_str(&toml::to_string(&config).unwrap()).unwrap();

        assert_eq!(roundtrip.channels["whatsapp"].model.as_deref(), Some("test-model"));
        assert_eq!(roundtrip.channels["telegram"].provider.as_deref(), Some("codex"));

        for value in [
            "[channels.bad_name]",
            "[channels.telegram]\nprovider = '../bad'",
            "[channels.telegram]\nmodel = ''",
            "[channels.telegram]\nmodel = '   '",
            "[channels.telegram]\nmodel = \"line\\nline\"",
        ] {
            let config: Config = toml::from_str(value).unwrap();

            assert!(config.validate().is_err(), "{value}");
        }
    }
}
