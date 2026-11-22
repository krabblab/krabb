//! Runs shared slash commands for a conversation without an intelligence turn.
//!
//! Every command acts only on the caller's own conversation, and channel callers never gain the
//! local operator's authority. The model reaches the same commands through the `conversation`
//! tool, which can never confirm destructive actions on the user's behalf.

use super::*;
use krabb_core::slash::{Access, Invocation, Need, SPECS, Spec};
use std::time::Instant;

const CONFIRM: Duration = Duration::from_secs(120);
const PENDING: usize = 256;
const LIST: usize = 20;
const WORKING: &str = "A reply is in progress; send /session cancel first.";
const SHORT: &str = "There is not enough history to compact yet.";
const NO_REPLY: &str = "There is no reply to answer again yet.";
const AGAIN_WORKING: &str = "Send /again when the current reply finishes.";

const HATCH_HELP: &str = "Tell me how you'd like me to be, in your own words, after /hatch. For \
                          example: /hatch call me Sam, be playful but brief, and reply in \
                          Italian. You can change my name, how I address you, my personality, \
                          tone of voice, writing style, way of working, and language. Send \
                          /hatch undo to go back to the previous version.";

/// Serializes personality changes, so each one edits the result of the one before.
static HATCHING: std::sync::LazyLock<AsyncMutex<()>> =
    std::sync::LazyLock::new(|| AsyncMutex::new(()));

/// Pending `/session clear` codes by conversation and sender, with their expiry.
type Confirmations = BTreeMap<(String, String), (String, Instant)>;

static CONFIRMATIONS: std::sync::LazyLock<Mutex<Confirmations>> =
    std::sync::LazyLock::new(|| Mutex::new(BTreeMap::new()));

/// Conversations that start fresh once the reply that asked for it has finished.
static FRESH: std::sync::LazyLock<Mutex<BTreeSet<String>>> =
    std::sync::LazyLock::new(|| Mutex::new(BTreeSet::new()));

pub(crate) struct Host<'a> {
    pub plugins: &'a Plugins,
    pub sessions: &'a Arc<Mutex<state::Store>>,
    pub cancels: &'a Arc<Mutex<BTreeMap<String, Arc<Cancellation>>>>,
    pub approvals: &'a Arc<AsyncMutex<approval::Gate>>,
    /// Daemon shutdown, which interrupts long commands such as `/compact`.
    pub stop: &'a Stop,
    pub provider: &'a str,
    pub model: &'a str,
    pub call: u64,
    /// A reply is streaming on this connection, so long commands would hold up its delivery.
    pub answering: bool,
    /// Krabb's home, for commands that change its instructions; absent where only the model
    /// can call commands.
    pub home: Option<&'a Path>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Origin {
    /// The user typed the exact command or picked it from a native menu.
    Text,
    /// The model chose the command from natural language during a turn.
    Tool,
}

pub(crate) struct Caller<'a> {
    pub integration: &'a str,
    pub chat: &'a str,
    pub thread: Option<&'a str>,
    pub session: &'a str,
    pub sender: Option<&'a str>,
    /// Whether this caller may use tool-backed commands such as memory and data.
    pub tools: bool,
    /// Whether this caller is the channel's configured owner.
    pub owner: bool,
    pub origin: Origin,
}

impl Caller<'_> {
    fn channel(&self) -> bool {
        !krabb_core::client::known(self.integration)
    }
}

/// A channel command's routing, shared by every command a channel user can send.
struct Intake {
    id: String,
    chat: String,
    thread: Option<String>,
    session: String,
    sender: Option<String>,
    tools: bool,
    owner: bool,
}

impl Intake {
    fn read(
        host: &Host<'_>,
        integration: &str,
        event: &Value,
        policy: &ChannelConfig,
        mode: ApprovalMode,
    ) -> Result<Self, PluginError> {
        let id = event_id(&event["id"]).ok_or("Command message has no identity.")?;
        let chat = event_id(&event["chat"]).ok_or("Command message has no destination.")?;
        let thread = event_id(&event["thread"]);
        let approval =
            sharing::approval(&host.plugins.config, &sharing::principal(integration), mode);

        let sender = event_id(&event["sender"]);

        Ok(Self {
            session: session_id(integration, &chat, thread.as_deref()),
            owner: sender.is_some() && policy.owner == sender,
            sender,
            tools: tools_allowed(policy, event, &chat, approval.enabled()),
            id,
            chat,
            thread,
        })
    }

    fn key(&self, prefix: &str) -> String {
        format!("{prefix}-{}", jobs::dialog::key(&self.session, &self.id))
    }

    fn author(&self) -> &str {
        self.sender.as_deref().unwrap_or("user")
    }

    fn caller<'a>(&'a self, integration: &'a str) -> Caller<'a> {
        Caller {
            integration,
            chat: &self.chat,
            thread: self.thread.as_deref(),
            session: &self.session,
            sender: self.sender.as_deref(),
            tools: self.tools,
            owner: self.owner,
            origin: Origin::Text,
        }
    }

    fn reply(
        self,
        host: &Host<'_>,
        integration: &str,
        event: &Value,
        key: String,
        reply: String,
    ) -> Result<(), PluginError> {
        host.sessions.lock().map_err(|_| state::POISONED)?.queue_delivery(
            key,
            integration,
            self.chat,
            event["private"] == true,
            self.thread,
            reply,
        )?;

        Ok(())
    }
}

/// What a channel event's slash command left for the bridge to do.
pub(crate) enum Command {
    /// Not a command; the event is a message for the model.
    Pass,
    /// Answered, so the event is done.
    Handled,
    /// `/again` removed the latest exchange; answer this question as the event.
    Ask(String),
}

fn receipt(host: &Host<'_>, session: &str, key: &str) -> Result<Option<String>, PluginError> {
    let store = host.sessions.lock().map_err(|_| state::POISONED)?;

    Ok(store
        .sessions
        .get(session)
        .and_then(|value| value.commands.iter().find(|control| control.id == key))
        .map(|control| control.reply.clone()))
}

/// Turns an `/again` event into the question it re-asks; native content would otherwise win.
pub(crate) fn reask(event: &mut Value, question: String) {
    if let Some(event) = event.as_object_mut() {
        event.remove("content");
        event.insert("text".into(), question.into());
    }
}

/// Removes the latest exchange once, recording the question so a replayed command reuses it.
fn rewind(
    host: &Host<'_>,
    integration: &str,
    intake: &Intake,
    key: &str,
    text: &str,
) -> Result<Result<String, String>, PluginError> {
    let mut store = host.sessions.lock().map_err(|_| state::POISONED)?;

    let Some(session) = store.sessions.get(&intake.session) else {
        return Ok(Err(NO_REPLY.into()));
    };

    if let Some(control) = session.commands.iter().find(|control| control.id == key) {
        return Ok(Ok(control.reply.clone()));
    }

    // A reply still being sent would arrive after the new answer; uncertain ones wait for
    // an operator, so they never block this.
    let sending = store.outbox.iter().any(|delivery| {
        delivery.status != state::DeliveryStatus::Uncertain
            && delivery.channel == integration
            && delivery.chat == intake.chat
            && delivery.thread == intake.thread
    });

    if session.replying() || sending {
        return Ok(Err(AGAIN_WORKING.into()));
    }

    let (from, question) = match last_exchange(&session.messages) {
        Ok(found) => found,
        Err(reply) => return Ok(Err(reply.into())),
    };

    if session.messages[from].sender.as_deref() != intake.sender.as_deref() {
        return Ok(Err("Only the person who asked can use /again on that question.".into()));
    }

    let control = state::dialog::Control {
        id: key.to_owned(),
        sender: intake.author().to_owned(),
        input: text.to_owned(),
        reply: question.clone(),
    };

    store.rewind(&intake.session, from, control)?;
    info!(session = %intake.session, "Latest reply removed to answer again.");

    Ok(Ok(question))
}

/// Finds the latest text question and checks that its reply ran no tools.
fn last_exchange(messages: &[Message]) -> Result<(usize, String), &'static str> {
    let last = messages.iter().rposition(|message| message.role == Role::User).ok_or(NO_REPLY)?;
    let reply = &messages[last + 1..];
    let mut from = last;

    // Messages sent before the reply started were answered together, so they are asked again.
    while from > 0 && messages[from - 1].role == Role::User {
        if messages[from - 1].sender != messages[last].sender {
            return Err("That reply answered several people, so it cannot be answered again.");
        }

        from -= 1;
    }

    if reply.is_empty() {
        return Err(NO_REPLY);
    }

    // Task and schedule notices carry a sender; removing them would hide what they reported.
    // A failed reply is the host's own answer, so it can be answered again.
    if reply.iter().any(|message| {
        message.role == Role::Assistant
            && message.sender.as_deref().is_some_and(|sender| sender != krabb_core::types::SYSTEM)
    }) {
        return Err("Something else was posted after that reply, so it can no longer be \
                    answered again. Ask again in your own words instead.");
    }

    if reply.iter().any(|message| {
        message.role != Role::Assistant
            || message.content.iter().any(|content| !matches!(content, Content::Text { .. }))
    }) {
        return Err("The latest reply used tools, so answering again could repeat what they did. \
                    Ask again in your own words instead.");
    }

    let mut parts = Vec::new();

    for content in messages[from..=last].iter().flat_map(|message| &message.content) {
        match content {
            Content::Text { text } => parts.push(text.as_str()),

            Content::Context { .. } => {}

            _ => {
                return Err("/again only works for text messages; send the question again with \
                            its attachments.");
            }
        }
    }

    let question = parts.join("\n");

    if question.trim().is_empty() {
        return Err(NO_REPLY);
    }

    Ok((from, question))
}

/// Answers a channel's slash command once per event, replaying the receipt on redelivery.
pub(crate) async fn channel(
    host: &Host<'_>,
    integration: &str,
    event: &Value,
    policy: &ChannelConfig,
    mode: ApprovalMode,
) -> Result<Command, PluginError> {
    let Some(text) = event["text"].as_str() else {
        return Ok(Command::Pass);
    };

    let Some(invocation) = krabb_core::slash::parse(text) else {
        return Ok(Command::Pass);
    };

    if invocation.spec.name == "task" {
        return Ok(Command::Pass);
    }

    let intake = Intake::read(host, integration, event, policy, mode)?;
    let caller = intake.caller(integration);
    let key = intake.key("slash");

    if let Some(reply) = receipt(host, &intake.session, &key)? {
        intake.reply(host, integration, event, key, reply)?;

        return Ok(Command::Handled);
    }

    // `/again` re-asks only a reply that ran no tools, so nothing it did is repeated.
    let reply = if invocation.spec.name == "again"
        && available(&loaded(host).await, &caller, invocation.spec)
    {
        match rewind(host, integration, &intake, &intake.key("again"), text)? {
            Ok(question) => return Ok(Command::Ask(question)),
            Err(reply) => reply,
        }
    } else {
        run(host, &caller, invocation).await?
    };

    let mut store = host.sessions.lock().map_err(|_| state::POISONED)?;

    if store.sessions.contains_key(&intake.session) {
        store.record_control(&intake.session, &key, intake.author(), text, &reply)?;
    }

    drop(store);
    intake.reply(host, integration, event, key, reply)?;

    Ok(Command::Handled)
}

fn unavailable(spec: &Spec) -> String {
    format!("/{} is not available here. Send /help to see what is.", spec.name)
}

pub(crate) async fn run(
    host: &Host<'_>,
    caller: &Caller<'_>,
    invocation: Invocation<'_>,
) -> Result<String, PluginError> {
    let spec = invocation.spec;
    let loaded = loaded(host).await;

    if !available(&loaded, caller, spec) {
        return Ok(unavailable(spec));
    }

    if host.answering && spec.lengthy() {
        return Ok(format!(
            "Krabb is busy with another message on this channel; send /{} once it finishes.",
            spec.name
        ));
    }

    let arguments = invocation.arguments;

    match spec.name {
        "help" => Ok(help(&loaded, caller, arguments, invocation.unknown)),
        "status" => status(host, caller),
        "model" => model(host, caller, arguments).await,
        "session" => session(host, caller, spec, arguments),
        "compact" => compact(host, caller).await,
        "hatch" => hatch(host, caller, spec, arguments).await,

        "again" => Ok("Only the user can send /again, and it cannot redo a reply that used \
                       tools, like this one. Answer the question again yourself instead."
            .into()),
        "workspace" => workspace(host, caller),
        "approvals" => approvals(host, caller).await,
        "plugin" => Ok(plugin(host).await),
        "memory" => memory(host, caller, spec, arguments).await,
        "data" => data(host, caller, arguments).await,
        "task" => task(host, caller, arguments),
        "schedule" => schedule(host).await,
        _ => Ok(usage(spec)),
    }
}

/// Host tool the model uses when the user asks for a command in their own words.
pub(crate) const TOOL: &str = "conversation";

pub(crate) fn spec() -> ToolSpec {
    ToolSpec {
        name: TOOL.into(),
        description: Some(
            "Run a Krabb conversation command when the user asks for one in their own words, in \
             any language: help, status, this conversation's model (show, list, or switch with \
             the model name as arguments), session (new starts a fresh conversation after this \
             reply), task, workspace, approvals, plugin, memory (search terms as arguments), or \
             data (a database name as arguments). Cancelling a reply, clearing history, \
             compacting, regenerating a reply, and changing your personality (hatch) need the \
             user's own message; relay the instructions this tool returns."
                .into(),
        ),
        schema: serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "enum": SPECS.iter().map(|spec| spec.name).collect::<Vec<_>>(),
                },
                "arguments": {"type": "string", "maxLength": 512},
            },
            "required": ["name"],
            "additionalProperties": false,
        }),
    }
}

/// Runs the `conversation` tool for a turn, with the same scope a typed command would have.
pub(crate) async fn call(
    host: &Host<'_>,
    caller: &Caller<'_>,
    args: &Value,
) -> Result<String, PluginError> {
    let name = args["name"].as_str().unwrap_or_default();
    let arguments = args["arguments"].as_str().unwrap_or_default().trim();

    let spec = krabb_core::slash::find(name).ok_or("Unknown conversation command.")?;

    if arguments.len() > 512 || arguments.chars().any(char::is_control) {
        return Err("Command arguments must be a single line of at most 512 bytes.".into());
    }

    run(host, caller, Invocation { spec, arguments, unknown: None }).await
}

/// Starts a fresh conversation for sessions whose reply asked for one through the model.
///
/// Waits while messages are queued or a reply is running, so none move to the archive.
pub(crate) fn settle(store: &mut state::Store, session: &str) -> Result<(), PluginError> {
    let busy = store.sessions.get(session).is_some_and(|value| {
        value.status == "working" || value.inflight.is_some() || !value.queued.is_empty()
    });

    if busy {
        return Ok(());
    }

    let fresh = FRESH.lock().map_err(|_| state::POISONED)?.remove(session);

    if fresh {
        restart(store, session)?;
    }

    Ok(())
}

/// Capabilities of every loaded plugin, read once per command.
async fn loaded(host: &Host<'_>) -> Vec<Capability> {
    host.plugins
        .all()
        .await
        .iter()
        .flat_map(|(_, plugin)| plugin.hello.capabilities.clone())
        .collect()
}

fn available(loaded: &[Capability], caller: &Caller<'_>, spec: &Spec) -> bool {
    let present = match &spec.need {
        Need::Nothing => true,
        Need::Capability(capability) => loaded.contains(capability),
    };

    let granted = match spec.access {
        Access::Anyone => true,
        Access::Local => !caller.channel(),
        Access::Channel => caller.channel(),
        Access::Tools => caller.tools,
        Access::Owner => !caller.channel() || caller.owner,
    };

    present && granted
}

fn usage(spec: &Spec) -> String {
    format!("Usage: {}\n{}.", spec.usage, spec.summary)
}

fn help(
    loaded: &[Capability],
    caller: &Caller<'_>,
    arguments: &str,
    unknown: Option<&str>,
) -> String {
    let topic = arguments.trim_start_matches('/');

    if let Some(spec) = krabb_core::slash::find(topic)
        && available(loaded, caller, spec)
    {
        return usage(spec);
    }

    let mut rows = Vec::new();

    for spec in SPECS {
        if available(loaded, caller, spec) {
            rows.push(format!("{} — {}", spec.usage, spec.summary));
        }
    }

    let mut text = match unknown {
        Some(name) => format!("Unknown command {}.\n", clip(name.to_owned(), 32)),
        None => String::new(),
    };

    text.push_str("Commands:\n");
    text.push_str(&rows.join("\n"));
    text.push_str("\nYou can also ask in plain words, for example to start a new conversation.");

    text
}

fn status(host: &Host<'_>, caller: &Caller<'_>) -> Result<String, PluginError> {
    let store = host.sessions.lock().map_err(|_| state::POISONED)?;
    let session = store.sessions.get(caller.session);
    let model = session.map_or(host.model, |session| session.model.as_str());

    let activity = match session {
        None => "new conversation".to_owned(),

        Some(session) => {
            let state = if session.status == "working" { "replying" } else { "idle" };

            let count = session.conversation().count();
            let noun = messages(count);

            format!("{state}, {count} {noun}")
        }
    };

    let tasks = store
        .jobs
        .values()
        .filter(|job| job.origin.session == caller.session && !job.status.terminal())
        .count();

    let mut text =
        format!("Krabb {VERSION}\nModel: {} / {model}\nConversation: {activity}", host.provider);

    if let Some(usage) = session.and_then(|session| session.context_usage.as_ref())
        && usage.limit > 0
    {
        text.push_str(&format!(
            "\nContext: {} of {} tokens ({}%)",
            tokens(usage.used),
            tokens(usage.limit),
            usage.used.saturating_mul(100) / usage.limit
        ));
    }

    if tasks > 0 {
        text.push_str(&format!("\nUnfinished tasks: {tasks} (send /task to list them)"));
    }

    Ok(text)
}

/// Summarizes all but the latest exchanges into the model context; stored history stays intact.
async fn compact(host: &Host<'_>, caller: &Caller<'_>) -> Result<String, PluginError> {
    if caller.origin == Origin::Tool {
        return Ok("Tell the user to send /compact after this reply.".into());
    }

    let (model, workspace, context, through) = {
        let store = host.sessions.lock().map_err(|_| state::POISONED)?;

        let Some(session) = store.sessions.get(caller.session) else {
            return Ok(SHORT.into());
        };

        if session.replying() {
            return Ok(WORKING.into());
        }

        let Some(through) = session.messages.last().map(|message| message.id.clone()) else {
            return Ok(SHORT.into());
        };

        if session.compacted_through.as_deref() == Some(through.as_str()) {
            return Ok("This conversation is already compacted.".into());
        }

        (session.model.clone(), session.workspace.clone(), session.context(), through)
    };

    let Some(boundary) = ipc::compaction_boundary(&context) else {
        return Ok(SHORT.into());
    };

    let Some(provider) =
        host.plugins.get(host.provider).await.filter(|plugin| plugin.supports(Capability::Model))
    else {
        return Ok("The model is not running; try again in a moment.".into());
    };

    let summary =
        complete(host, &provider, &model, workspace.as_deref(), &context[..boundary]).await;

    // Provider failures answer the user; they must not stop the channel's message loop.
    let summary = match summary {
        Ok(summary) => summary,

        Err(error) => {
            warn!(
                session = caller.session,
                error = %diagnostic(sentence(error)),
                "Conversation compaction failed."
            );

            return Ok(
                "Summarizing failed, so the conversation is unchanged. Try again later.".into()
            );
        }
    };

    let compacted = ipc::summarized(
        format!("compact-{through}"),
        caller.session,
        &summary,
        &context[boundary..],
    );

    let saved = host.sessions.lock().map_err(|_| state::POISONED)?.set_compacted_context(
        caller.session,
        &through,
        compacted,
    )?;

    if !saved {
        return Ok("The conversation changed while summarizing; send /compact again.".into());
    }

    info!(session = caller.session, summarized = boundary, "Conversation compacted by command.");

    Ok(format!(
        "Summarized {boundary} earlier {} for the model; the latest exchanges stay as they are \
         and the full history is kept.",
        messages(boundary)
    ))
}

/// Rewrites KRAB.md and KLAW.md from the user's wishes, or restores the previous version.
async fn hatch(
    host: &Host<'_>,
    caller: &Caller<'_>,
    spec: &Spec,
    arguments: &str,
) -> Result<String, PluginError> {
    if caller.origin == Origin::Tool {
        return Ok("Changing my personality needs the user's own message. Tell them to send \
                   /hatch followed by what to change."
            .into());
    }

    let Some(home) = host.home else {
        return Ok(unavailable(spec));
    };

    let directory = home.join("data/system");
    let wishes = arguments.trim();

    if wishes.is_empty() {
        return Ok(HATCH_HELP.into());
    }

    if wishes.chars().count() > hatch::WISH_LIMIT {
        return Ok(format!("Describe the change in at most {} characters.", hatch::WISH_LIMIT));
    }

    let _hatching = HATCHING.lock().await;

    if wishes.eq_ignore_ascii_case("undo") {
        let restored = hatch::undo(&directory)?;

        if restored.is_empty() {
            return Ok("There is no previous personality to go back to.".into());
        }

        info!(session = caller.session, "Personality restored by command.");

        return Ok(
            "I'm back to my previous personality. Send /hatch undo again to switch back.".into()
        );
    }

    let Some(provider) =
        host.plugins.get(host.provider).await.filter(|plugin| plugin.supports(Capability::Model))
    else {
        // Channel users never see the host's paths.
        return Ok(if caller.channel() {
            "Hatching needs an intelligence provider, and none is running. Set one up on the \
             Krabb host, or edit KRAB.md and KLAW.md there by hand."
                .into()
        } else {
            hatch::no_provider(&directory)
        });
    };

    let current = hatch::read(&directory)?;
    let request = hatch::request(caller.session, &current, wishes);

    let text = match complete(host, &provider, host.model, None, &request).await {
        Ok(text) => text,

        Err(error) => {
            warn!(
                session = caller.session,
                error = %diagnostic(sentence(error)),
                "Hatching failed."
            );

            return Ok("I couldn't reach the model, so nothing changed. Try again later.".into());
        }
    };

    let draft = match hatch::parse(&text) {
        Ok(draft) => draft,

        Err(problem) => {
            warn!(session = caller.session, %problem, "Hatching draft was rejected.");

            return Ok(format!("{problem} Nothing changed; send /hatch again to retry."));
        }
    };

    if draft.files == current {
        return Ok(format!("{} Nothing needed to change.", draft.summary));
    }

    hatch::save(&directory, &current, &draft.files)?;
    info!(session = caller.session, "Personality changed by command.");

    Ok(format!("{}\nSend /hatch undo to go back.", draft.summary))
}

/// Asks the model for one reply, stopping early when the daemon shuts down. Returns the
/// reply, or the provider or cleanup failure as text for the log.
async fn complete(
    host: &Host<'_>,
    provider: &Live,
    model: &str,
    workspace: Option<&str>,
    messages: &[Message],
) -> Result<String, String> {
    let cleanup = process::Cleanup::default();

    let reply = tokio::select! {
        biased;
        _ = host.stop.notified() => Err("The request was interrupted.".into()),

        reply = ipc::complete(
            provider,
            model,
            workspace,
            messages,
            host.call,
            &cleanup,
            host.plugins.config.inference.task_millis(),
        ) => reply.map_err(|error| error.to_string()),
    };

    let finished = cleanup.finish().await.map_err(|error| error.to_string());

    match (finished, reply) {
        (Ok(()), Ok(reply)) => Ok(reply),
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}

fn messages(count: usize) -> &'static str {
    if count == 1 { "message" } else { "messages" }
}

fn tokens(count: u64) -> String {
    if count < 1_000 { count.to_string() } else { format!("{}k", count / 1_000) }
}

async fn model(
    host: &Host<'_>,
    caller: &Caller<'_>,
    arguments: &str,
) -> Result<String, PluginError> {
    let current = host
        .sessions
        .lock()
        .map_err(|_| state::POISONED)?
        .sessions
        .get(caller.session)
        .map_or_else(|| host.model.to_owned(), |session| session.model.clone());

    match arguments {
        "" => Ok(format!(
            "This conversation uses {} / {current}. Send /model list to see the options or \
             /model <name> to switch.",
            host.provider
        )),

        "list" => models(host).await,

        name => {
            if name.len() > META_LIMIT
                || name.chars().any(|character| character.is_control() || character.is_whitespace())
            {
                return Ok("Send /model <name> with a single model name.".into());
            }

            let mut store = host.sessions.lock().map_err(|_| state::POISONED)?;

            if !store.sessions.contains_key(caller.session) {
                store.ensure_channel(caller.session, name)?;
            }

            store.set_model(caller.session, name)?;
            info!(session = caller.session, "Conversation model changed by command.");

            Ok(format!(
                "This conversation now uses {} / {name}, starting with the next reply.",
                host.provider
            ))
        }
    }
}

/// Lists the scheduler's work; only local clients reach it, so the whole list is theirs.
async fn schedule(host: &Host<'_>) -> Result<String, PluginError> {
    let Some((_, plugin)) = host.plugins.find(Capability::Scheduler).await else {
        return Ok("Schedules are unavailable right now.".into());
    };

    let response =
        plugin.call(Request::call(host.call, "list", serde_json::json!({"page": 1}))).await?;

    Ok(match (response.result, response.error) {
        (Some(value), None) => clip(format_schedule_list(&value), 3500),
        _ => "Schedules are unavailable right now.".into(),
    })
}

async fn models(host: &Host<'_>) -> Result<String, PluginError> {
    let Some(plugin) =
        host.plugins.get(host.provider).await.filter(|plugin| plugin.supports(Capability::Model))
    else {
        return Ok("The intelligence plugin for this conversation is not running.".into());
    };

    if !plugin.hello.commands.iter().any(|command| command.name == host.provider) {
        return Ok(format!(
            "{} cannot list its models; send /model <name> to switch.",
            host.provider
        ));
    }

    let response = plugin
        .call(Request::call(
            host.call,
            "command",
            serde_json::json!({"name": host.provider, "args": ["models"]}),
        ))
        .await?;

    let text = match (response.result, response.error) {
        (Some(value), None) => value["text"].as_str().map(str::to_owned),
        _ => None,
    };

    Ok(text
        .map_or_else(|| "The model list is unavailable right now.".into(), |text| clip(text, 3500)))
}

fn session(
    host: &Host<'_>,
    caller: &Caller<'_>,
    spec: &Spec,
    arguments: &str,
) -> Result<String, PluginError> {
    let (action, code) = arguments.split_once(char::is_whitespace).unwrap_or((arguments, ""));
    let code = code.trim();
    let mut store = host.sessions.lock().map_err(|_| state::POISONED)?;
    let current = store.sessions.get(caller.session);
    let working = current.is_some_and(|session| session.status == "working");
    let count = current.map_or(0, |session| session.conversation().count());

    match action {
        "" => Ok(format!(
            "This conversation has {count} {}. Send /session new to start fresh, \
             /session cancel to stop the current reply, or /session clear to delete its history.",
            messages(count)
        )),

        "new" if count == 0 => Ok("This is already a new conversation.".into()),

        "new" if caller.origin == Origin::Tool && !caller.channel() => {
            Ok("Tell the user to start a new conversation with /new in this client.".into())
        }

        "new" if caller.origin == Origin::Tool => {
            FRESH.lock().map_err(|_| state::POISONED)?.insert(caller.session.to_owned());

            Ok("A new conversation starts after this reply; earlier messages are archived.".into())
        }

        "new" if working => Ok(WORKING.into()),

        "new" => {
            restart(&mut store, caller.session)?;

            Ok("Started a new conversation. Earlier messages are archived.".into())
        }

        "cancel" if caller.origin == Origin::Tool => {
            Ok("Tell the user to send /session cancel to stop a reply.".into())
        }

        "cancel" if !working => Ok("Nothing is running in this conversation.".into()),

        "cancel" => {
            store.interrupt_foreground(caller.session)?;
            drop(store);
            cancellation(host.cancels, caller.session).request();

            Ok("Stopping the current reply.".into())
        }

        "clear" if count == 0 => Ok("This conversation has no history to clear.".into()),
        "clear" if working => Ok(WORKING.into()),
        "clear" => clear(&mut store, caller, code),

        _ => Ok(usage(spec)),
    }
}

/// Deletes history only after the same sender repeats the command with a fresh code.
fn clear(store: &mut state::Store, caller: &Caller<'_>, code: &str) -> Result<String, PluginError> {
    let sender = caller.sender.unwrap_or("user").to_owned();
    let key = (caller.session.to_owned(), sender);
    let mut pending = CONFIRMATIONS.lock().map_err(|_| state::POISONED)?;
    let now = Instant::now();

    pending.retain(|_, (_, expires)| *expires > now);

    let confirmed = caller.origin == Origin::Text
        && !code.is_empty()
        && pending.get(&key).is_some_and(|(expected, _)| expected == code);

    if confirmed {
        pending.remove(&key);
        drop(pending);
        store.clear_history(caller.session)?;
        info!(session = caller.session, "Conversation history cleared by command.");

        return Ok("Deleted this conversation's history.".into());
    }

    if pending.len() >= PENDING {
        return Ok("Too many confirmations are pending; try again in a few minutes.".into());
    }

    let mut bytes = [0_u8; 4];

    getrandom::fill(&mut bytes)?;

    let code = format!("{:04}", u32::from_le_bytes(bytes) % 10_000);

    pending.insert(key, (code.clone(), now + CONFIRM));

    Ok(format!(
        "This deletes this conversation's history and cannot be undone. To confirm, send \
         /session clear {code} within two minutes."
    ))
}

fn restart(store: &mut state::Store, session: &str) -> Result<(), PluginError> {
    if !store.sessions.contains_key(session) {
        return Ok(());
    }

    let archive = format!("{session}-{}", unix_seconds());

    store.rename_session(session, &archive)?;
    store.archive_session(&archive, true)?;
    info!(session, "Conversation restarted by command.");

    Ok(())
}

fn workspace(host: &Host<'_>, caller: &Caller<'_>) -> Result<String, PluginError> {
    let store = host.sessions.lock().map_err(|_| state::POISONED)?;
    let isolated = store
        .sessions
        .get(caller.session)
        .is_some_and(|session| session.isolated.is_some() || session.workspace.is_some());

    Ok(if isolated {
        "This conversation has its own workspace.".into()
    } else {
        "This conversation uses the shared workspace.".into()
    })
}

async fn approvals(host: &Host<'_>, caller: &Caller<'_>) -> Result<String, PluginError> {
    let now = unix_seconds();

    let rows = host
        .approvals
        .lock()
        .await
        .list()?
        .into_iter()
        .filter(|view| view.target.session == caller.session)
        .take(LIST)
        .map(|view| {
            let minutes = view.expires.saturating_sub(now).div_ceil(60);

            format!("{} (expires in {minutes} min)", view.target.tool)
        })
        .collect::<Vec<_>>();

    Ok(if rows.is_empty() {
        "Nothing is waiting for approval here.".into()
    } else {
        format!(
            "Waiting for approval:\n{}\nAnswer with the buttons on each request.",
            rows.join("\n")
        )
    })
}

async fn plugin(host: &Host<'_>) -> String {
    let rows = host
        .plugins
        .all()
        .await
        .into_iter()
        .take(LIST)
        .map(|(id, plugin)| {
            let capabilities = plugin
                .hello
                .capabilities
                .iter()
                .map(capability_name)
                .collect::<Vec<_>>()
                .join(", ");

            format!("{id} ({capabilities})")
        })
        .collect::<Vec<_>>();

    format!("Active plugins:\n{}", rows.join("\n"))
}

async fn memory(
    host: &Host<'_>,
    caller: &Caller<'_>,
    spec: &Spec,
    arguments: &str,
) -> Result<String, PluginError> {
    if arguments.is_empty() {
        return Ok(usage(spec));
    }

    let value =
        read(host, caller, "memory_search", serde_json::json!({"query": arguments})).await?;

    let rows = value["items"]
        .as_array()
        .into_iter()
        .flatten()
        .take(LIST)
        .map(|item| {
            let key = item["key"].as_str().unwrap_or("memory");
            let text = item["value"].as_str().unwrap_or_default();

            format!("{key}: {}", clip(text.to_owned(), 200))
        })
        .collect::<Vec<_>>();

    Ok(if rows.is_empty() { "No matching memories.".into() } else { rows.join("\n") })
}

async fn data(
    host: &Host<'_>,
    caller: &Caller<'_>,
    arguments: &str,
) -> Result<String, PluginError> {
    let args = if arguments.is_empty() {
        serde_json::json!({"operation": "list_databases"})
    } else {
        serde_json::json!({"operation": "list_tables", "database": arguments})
    };

    let value = read(host, caller, "data", args).await?;
    let field = if arguments.is_empty() { "databases" } else { "tables" };

    let names = value[field]
        .as_array()
        .into_iter()
        .flatten()
        .take(LIST)
        .filter_map(|item| item.as_str().or_else(|| item["name"].as_str()))
        .collect::<Vec<_>>();

    Ok(match (names.is_empty(), arguments.is_empty()) {
        (true, true) => "No databases are saved here yet.".into(),
        (true, false) => format!("{} has no tables.", clip(arguments.to_owned(), 64)),
        (false, true) => format!("Databases:\n{}", names.join("\n")),
        (false, false) => format!("Tables:\n{}", names.join("\n")),
    })
}

/// Runs a read-only host tool under the same scope and sharing rules as a turn.
async fn read(
    host: &Host<'_>,
    caller: &Caller<'_>,
    name: &str,
    args: Value,
) -> Result<Value, PluginError> {
    let workspace = host
        .sessions
        .lock()
        .map_err(|_| state::POISONED)?
        .sessions
        .get(caller.session)
        .and_then(|session| session.isolated.clone());

    let source = sharing::principal(caller.integration);
    let scope = origin::scope(&source, caller.session, caller.chat, caller.thread);
    let registry = ToolRegistry { source: Some(source.clone()), ..ToolRegistry::default() };

    let turn = format!("{}:command", caller.session);

    let result = tool_with_registry(
        host.plugins,
        &registry,
        host.call,
        name,
        args,
        false,
        workspace.as_deref(),
        &scope,
        caller.session,
        &turn,
        &mut None,
    )
    .await;

    registry.cleanup.finish().await?;

    let text = tool_result_text(&result?);

    Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
}

fn task(host: &Host<'_>, caller: &Caller<'_>, arguments: &str) -> Result<String, PluginError> {
    let action = arguments.split_whitespace().next().unwrap_or_default();

    if caller.origin == Origin::Tool && matches!(action, "approve" | "deny") {
        return Ok("Only the user can approve or deny a saved action. Tell them to send \
                   /task approve <id> or /task deny <id>."
            .into());
    }

    let input = Message {
        id: format!("command-{}", now()),
        session: caller.session.to_owned(),
        role: Role::User,
        sender: caller.sender.map(str::to_owned),
        content: vec![Content::Text { text: format!("/task {arguments}") }],
    };

    let mut store = host.sessions.lock().map_err(|_| state::POISONED)?;

    if !store.sessions.contains_key(caller.session) {
        return Ok("There are no unfinished tasks in this conversation.".into());
    }

    Ok(jobs::dialog::message(&mut store, &input, &host.plugins.jobs)?
        .unwrap_or_else(|| "Send /task to list tasks.".into()))
}

#[cfg(test)]
mod tests;
