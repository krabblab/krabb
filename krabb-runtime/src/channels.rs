pub(super) mod intake;

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

const WORKERS: usize = 64;

const CALL_BASE: u64 = 1 << 48;
const CALL_SPAN: u64 = 1 << 40;
const EPOCHS: u64 = 256;

/// Offsets of a turn's streaming and intake subrequests inside a worker's range.
pub(super) const STREAM_CALLS: u64 = 1 << 28;
pub(super) const INTAKE_CALLS: u64 = 1 << 30;

const _: () = assert!(STREAM_CALLS < INTAKE_CALLS && 2 * INTAKE_CALLS <= CALL_SPAN / EPOCHS);

/// Keeps each live worker's request IDs apart, including streaming and intake subrequests, which
/// stay inside the worker's range. A slot is reused only after its worker has finished, and each
/// reuse starts a new range, so a late answer to a stopped worker's call cannot match a call of
/// its successor.
fn first_call(slot: usize, epoch: u64) -> u64 {
    CALL_BASE + slot as u64 * CALL_SPAN + epoch % EPOCHS * (CALL_SPAN / EPOCHS)
}

pub(super) async fn run(context: Context) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    type Outcome = Result<(), Box<dyn std::error::Error + Send + Sync>>;

    let mut workers = tokio::task::JoinSet::<(String, usize, Outcome)>::new();
    let mut active = BTreeMap::<String, Arc<Stop>>::new();
    let mut slots = (0..WORKERS).rev().collect::<Vec<_>>();
    let mut epochs = [0_u64; WORKERS];
    let mut backoff = BTreeMap::<String, (u32, tokio::time::Instant)>::new();
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    let mut failure = None;

    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            biased;
            _ = context.stop.notified() => break,
            _ = tokio::signal::ctrl_c() => break,

            completed = workers.join_next(), if !workers.is_empty() => {
                match completed {
                    Some(Ok((id, slot, result))) => {
                        active.remove(&id);
                        slots.push(slot);

                        if let Err(error) = result {
                            warn!(channel = id, error = %diagnostic(sentence(error.to_string())), "Channel worker stopped.");

                            let attempts = backoff.get(&id).map_or(0, |(attempts, _)| *attempts);
                            let resume = tokio::time::Instant::now() + super::retry_delay(attempts);

                            backoff.insert(id, (attempts.saturating_add(1), resume));
                        } else {
                            backoff.remove(&id);
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
                    .filter(|(id, plugin)| {
                        plugin.supports(Capability::Channel) && !krabb_core::client::known(id)
                    })
                    .map(|(id, _)| id)
                    .collect::<BTreeSet<_>>();

                for (id, stop) in &active {
                    if !loaded.contains(id) {
                        stop.signal();
                    }
                }

                backoff.retain(|id, _| loaded.contains(id));

                for id in loaded {
                    let waiting = backoff
                        .get(&id)
                        .is_some_and(|(_, resume)| tokio::time::Instant::now() < *resume);

                    if active.contains_key(&id) || waiting {
                        continue;
                    }

                    let Some(slot) = slots.pop() else { break };

                    let call = first_call(slot, epochs[slot]);

                    epochs[slot] = epochs[slot].wrapping_add(1);
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

                        (id, slot, result)
                    });
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
    fn a_reused_worker_slot_starts_a_new_request_range() {
        assert_ne!(first_call(3, 0), first_call(3, 1));
        assert!(first_call(3, EPOCHS - 1) < first_call(4, 0));
        assert!(first_call(3, 0) + 2 * INTAKE_CALLS <= first_call(3, 1));
        assert_eq!(first_call(3, EPOCHS), first_call(3, 0));
        assert!(first_call(WORKERS - 1, EPOCHS - 1) + CALL_SPAN / EPOCHS <= i64::MAX as u64);
    }

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
