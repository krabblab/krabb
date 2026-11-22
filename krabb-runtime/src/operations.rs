use super::*;
use krabb_core::operation::{self, Query, Receipt, Spec, Start};
use state::JobStatus;

pub(crate) const RUNNER: &str = "runner";
const DECLARATIONS: usize = 16;
const DECLARATION_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Config {
    pub max_seconds: Option<u64>,
}

impl Config {
    pub fn validate(&self) -> Result<(), String> {
        if self.max_seconds == Some(0) {
            return Err("Operation allowance must be a positive number of seconds.".into());
        }

        Ok(())
    }
}

#[derive(Clone)]
pub(crate) struct Offer {
    plugin: String,
    spec: Spec,
    instance: std::sync::Weak<Live>,
    generation: u64,
}

impl std::fmt::Debug for Offer {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output
            .debug_struct("Offer")
            .field("plugin", &self.plugin)
            .field("spec", &self.spec)
            .finish()
    }
}

impl Offer {
    fn tool(&self) -> ToolSpec {
        ToolSpec {
            name: format!("operation_{}_{}", self.plugin, self.spec.name),
            description: Some(format!(
                concat!(
                    "{} The host saves the operation handle, monitors it without inference, ",
                    "and resumes this task when it settles. Supply one contextual ",
                    "acknowledgement in message. Do not restart an accepted operation."
                ),
                self.spec.description
            )),
            schema: serde_json::json!({"type":"object", "properties":{
                "arguments":self.spec.input, "message":{"type":"string"}
            }, "required":["arguments","message"], "additionalProperties":false}),
        }
    }
}

/// Declarations listed by one runner process; a restart or replacement lists them again.
pub(crate) struct Catalog {
    instance: std::sync::Weak<Live>,
    generation: u64,
    offers: BTreeMap<String, Offer>,
}

pub(crate) async fn discover(plugins: &Plugins) -> Result<BTreeMap<String, Offer>, PluginError> {
    let Some(plugin) = plugins.get(RUNNER).await else { return Ok(BTreeMap::new()) };

    if !plugin.supports(Capability::Runner) {
        return Ok(BTreeMap::new());
    }

    let generation = plugin.restart_generation();

    if let Some(catalog) =
        plugins.operations.lock().map_err(|_| "Operation lock is poisoned.")?.as_ref()
        && catalog.generation == generation
        && catalog.instance.upgrade().is_some_and(|instance| Arc::ptr_eq(&instance, &plugin))
    {
        return Ok(catalog.offers.clone());
    }

    let response = tokio::time::timeout(
        Duration::from_secs(2),
        plugin.call(Request::call(1, operation::LIST, serde_json::json!({}))),
    )
    .await;

    let Ok(Ok(response)) = response else { return Ok(BTreeMap::new()) };

    let offers = offers(response, &plugin, generation);

    *plugins.operations.lock().map_err(|_| "Operation lock is poisoned.")? =
        Some(Catalog { instance: Arc::downgrade(&plugin), generation, offers: offers.clone() });

    Ok(offers)
}

fn offers(response: Response, plugin: &Arc<Live>, generation: u64) -> BTreeMap<String, Offer> {
    let mut offers = BTreeMap::new();

    if response.error.is_some() {
        return offers;
    }

    let declarations = response.result.and_then(|value| value.get("operations").cloned());
    let specs = declarations.and_then(|value| {
        if value.as_array().is_none_or(|items| items.len() > DECLARATIONS)
            || serde_json::to_vec(&value).ok()?.len() > DECLARATION_BYTES
        {
            return None;
        }

        serde_json::from_value::<Vec<Spec>>(value).ok()
    });

    let Some(specs) = specs else {
        warn!(
            "Runner returned invalid operation declarations, and extended tools are unavailable."
        );
        return offers;
    };

    for spec in specs {
        let offer =
            Offer { plugin: RUNNER.into(), spec, instance: Arc::downgrade(plugin), generation };
        let tool = offer.tool();

        if offer.spec.validate().is_err()
            || validate_tool_spec(&tool).is_err()
            || offers.insert(tool.name, offer).is_some()
        {
            warn!(
                "Runner returned invalid or duplicate operations, and extended tools are unavailable."
            );
            return BTreeMap::new();
        }
    }

    offers
}

/// Reads a validated receipt from an operation response; `call` names the request.
fn read_receipt(response: Response, call: &str) -> Result<Receipt, PluginError> {
    if let Some(error) = response.error {
        return Err(error.message.into());
    }

    let receipt: Receipt = serde_json::from_value(
        response.result.ok_or_else(|| format!("{call} returned no receipt."))?,
    )?;

    receipt.validate()?;
    Ok(receipt)
}

pub(crate) fn definitions(offers: &BTreeMap<String, Offer>) -> Vec<ToolSpec> {
    offers.values().map(Offer::tool).collect()
}

pub(crate) async fn start(
    offer: &Offer,
    args: &Value,
    id: u64,
    registry: &ToolRegistry,
    plugins: &Plugins,
    sessions: &Arc<Mutex<state::Store>>,
) -> Result<ToolResult, PluginError> {
    let run = registry.run.as_ref().ok_or("A durable task is required for an operation.")?;
    let message = args["message"]
        .as_str()
        .filter(|text| !text.trim().is_empty() && text.len() <= tasks::MESSAGE_LIMIT)
        .ok_or("A contextual acknowledgement of at most 2,000 bytes is required.")?;

    let start = Start {
        id: format!("{}-op-{}-{id}", run.id, run.generation),
        name: offer.spec.name.clone(),
        arguments: args["arguments"].clone(),
    };

    start.validate()?;

    let plugin = plugins
        .get(&offer.plugin)
        .await
        .filter(|plugin| plugin.supports(Capability::Runner))
        .ok_or("Runner is unavailable, and the extended operation was not started.")?;

    if offer.instance.upgrade().is_none_or(|previous| !Arc::ptr_eq(&previous, &plugin))
        || offer.generation != plugin.restart_generation()
    {
        return Err(
            "Runner changed after discovery, so refresh its capabilities before starting.".into()
        );
    }

    let previous = {
        let mut store = sessions.lock().map_err(|_| crate::state::POISONED)?;
        let job = store.job(&run.id)?;
        let previous = (job.unsafe_step, job.next);

        store.prepare_operation(run, &offer.plugin, offer.spec.clone(), start.clone())?;
        previous
    };

    let result = registry
        .cleanup
        .call(
            Arc::clone(&plugin.process),
            Request::call(id, operation::START, serde_json::to_value(&start)?),
        )
        .await;

    if let Ok(Response { error: Some(error), .. }) = &result {
        sessions
            .lock()
            .map_err(|_| crate::state::POISONED)?
            .reject_operation(&run.id, &start.id, previous)?;

        return Ok(ToolResult::text(format!(
            "The runner refused the operation, and nothing was started. {}",
            krabb_core::failure::decode(error.clone())
        )));
    }

    let receipt = result
        .map_err(PluginError::from)
        .and_then(|response| read_receipt(response, "Operation start"));

    match receipt {
        Ok(receipt) => sessions.lock().map_err(|_| crate::state::POISONED)?.accept_operation(
            &run.id,
            &start.id,
            receipt,
            (registry.execution != state::SessionKind::Conversation).then_some(message),
            unix_seconds(),
        )?,

        Err(error) => {
            sessions
                .lock()
                .map_err(|_| crate::state::POISONED)?
                .abandon_operation(&run.id, &start.id)?;
            return Err(error);
        }
    }

    registry.task.handoff.send_replace(Some(message.into()));
    plugins.jobs.wake.notify_one();
    Ok(ToolResult::text("The operation receipt is saved. The host will monitor its handle."))
}

pub(crate) async fn monitor(
    context: &jobs::Context,
    job: &state::Job,
    cancel: &Cancellation,
) -> Result<(), PluginError> {
    let operation = job.operation.as_ref().ok_or("Operation not found.")?;
    let key = &operation.start.id;

    if operation.phase == state::OperationPhase::Dispatching {
        context
            .sessions
            .lock()
            .map_err(|_| crate::state::POISONED)?
            .abandon_operation(&job.id, key)?;
        return Ok(());
    }

    let receipt = operation.receipt.as_ref().ok_or("Operation receipt not found.")?;

    if receipt.state.terminal() {
        context.sessions.lock().map_err(|_| crate::state::POISONED)?.collect_operation(
            &job.id,
            key,
            unix_seconds(),
        )?;
        return Ok(());
    }

    let Some(plugin) = context.plugins.get(&operation.plugin).await else {
        context.sessions.lock().map_err(|_| crate::state::POISONED)?.fail_operation_poll(
            &job.id,
            key,
            unix_seconds(),
        )?;
        return Ok(());
    };

    let now = unix_seconds();
    let exceeded = operation.exceeded
        || context.config.operations.max_seconds.is_some_and(|limit| {
            operation.accepted.is_some_and(|accepted| now.saturating_sub(accepted) > limit)
        });

    if exceeded && !operation.exceeded {
        context
            .sessions
            .lock()
            .map_err(|_| crate::state::POISONED)?
            .exceed_operation(&job.id, key)?;
    }

    let cancelling = (exceeded
        || matches!(job.status, JobStatus::Cancelling | JobStatus::Cancelled))
        && operation.spec.cancel
        && !operation.cancel_sent;

    if cancelling {
        context
            .sessions
            .lock()
            .map_err(|_| crate::state::POISONED)?
            .record_operation_cancel(&job.id, key)?;
    }

    let query = Query {
        name: operation.spec.name.clone(),
        handle: receipt.handle.clone().ok_or("Operation handle not found.")?,
    };

    let request = Request::call(
        job.step.saturating_mul(1_000).saturating_add(900),
        if cancelling { operation::CANCEL } else { operation::INSPECT },
        serde_json::to_value(query)?,
    );

    let result = tokio::select! {
        biased;
        _ = context.stop.notified() => return Ok(()),
        _ = cancel.cancelled() => return Ok(()),
        result = tokio::time::timeout(Duration::from_secs(120), plugin.call(request)) => result,
    };

    let result = match result {
        Ok(response) => response.map_err(PluginError::from),
        Err(elapsed) => Err(elapsed.into()),
    };

    let mut store = context.sessions.lock().map_err(|_| crate::state::POISONED)?;

    if cancelling && !result.as_ref().is_ok_and(|response| response.error.is_none()) {
        store.retry_operation_cancel(&job.id, key)?;
    }

    match result.and_then(|response| read_receipt(response, "Operation inspection")) {
        Ok(receipt) => {
            let terminal = receipt.state.terminal();

            if let Err(error) = store.accept_operation(&job.id, key, receipt, None, unix_seconds())
            {
                debug!(operation=key, error=%diagnostic(error.to_string()), "Operation receipt was \
                    rejected.");
                store.fail_operation_poll(&job.id, key, unix_seconds())?;
                return Ok(());
            }

            if terminal {
                store.collect_operation(&job.id, key, unix_seconds())?;
            }
        }

        Err(error) => {
            debug!(operation=key, error=%diagnostic(error.to_string()), "Operation inspection will \
                retry.");
            store.fail_operation_poll(&job.id, key, unix_seconds())?;
        }
    }

    Ok(())
}
