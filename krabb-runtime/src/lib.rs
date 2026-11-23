#![forbid(unsafe_code)]

#[path = "templates/mod.rs"]
mod templates;

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    io::{BufRead, BufReader, IsTerminal, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{ExitCode, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use base64::{Engine, engine::general_purpose::STANDARD};
use clap::{ArgAction, Args, CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};
use clap_complete::{generate, shells};

use krabb_core::client;

use krabb_core::types::{
    Capability, CommandSpec, Content, Event, Hello, Message, ModelReply, ModelRequest, Protocol,
    Request, Response, Role, ToolContent, ToolResult, ToolSpec,
};

use krabb_file::{append_jsonl as append_jsonl_file, save as save_file};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use tokio::{
    net::TcpListener,
    sync::{Mutex as AsyncMutex, Notify, RwLock, watch},
};

use tracing::{debug, error, info, warn};

mod accounting;
mod agent;
mod answer;
pub(crate) mod approval;
mod archive;
mod bridge;
mod channels;
mod choice;
mod cli;
mod clients;
mod compaction;
mod context;
#[cfg(test)]
mod context_tests;
mod credentials;
mod daemon;
mod delivery;
mod digest;
mod dispatch;
mod doctor;
mod external;
mod failure;
mod hatch;
mod home;
mod inbound;
mod index;
mod inference;
pub(crate) mod ipc;
mod jobs;
mod krabfile;
mod launch;
mod lifecycle;
mod listing;
mod locks;
pub(crate) mod logging;
mod media;
mod messaging;
mod onboard;
mod operations;
mod origin;
mod outbound;
mod plugins;
mod process;
mod recovery;
mod releases;
mod revoke;
mod schedules;
mod sessions;
mod style;
mod tools;
use agent::*;
use answer::*;
use archive::*;
use bridge::*;
use cli::*;
use daemon::*;
use dispatch::*;
use doctor::*;
use external::*;
use home::*;
use inbound::*;
use krabfile::*;
use launch::*;
use listing::*;
use locks::*;
use outbound::*;
use plugins::*;
use schedules::{command::*, execution::*};
use service::*;
use sessions::*;
use sources::*;
use status::*;
use tools::*;
use updates::command::*;
mod schema;
mod selection;
mod service;
mod settings;
mod setup;
mod sharing;
mod slash;
mod sources;
pub(crate) mod state;
mod status;
mod storage;
mod tasks;
pub(crate) mod terminal;
mod turns;
mod updates;
mod upgrade;
mod worktree;
use process::Process;

const NAME: &str = "krabb";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const RUNTIME_COMMAND_OWNER: &str = "__runtime.";

const BANNER: &str = concat!(
    "\n",
    "██╗  ██╗██████╗  █████╗ ██████╗ ██████╗ \n",
    "██║ ██╔╝██╔══██╗██╔══██╗██╔══██╗██╔══██╗\n",
    "█████╔╝ ██████╔╝███████║██████╔╝██████╔╝\n",
    "██╔═██╗ ██╔══██╗██╔══██║██╔══██╗██╔══██╗\n",
    "██║  ██╗██║  ██║██║  ██║██████╔╝██████╔╝\n",
    "╚═╝  ╚═╝╚═╝  ╚═╝╚═╝  ╚═╝╚═════╝ ╚═════╝ ",
);

const BANNER_COLUMNS: u16 = 41;

pub(crate) fn banner(columns: u16) -> &'static str {
    if columns < BANNER_COLUMNS { "\nKrabb" } else { BANNER }
}

pub(crate) fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let bytes = bytes.as_ref();
    let mut encoded = String::with_capacity(bytes.len() * 2);

    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }

    encoded
}

const TOOL_STEPS: usize = 8;

const CLI_LIST_PAGE_SIZE: usize = 5;
const SCHEDULE_HISTORY_PAGE_SIZE: usize = 5;
const SCHEDULE_HISTORY_LINE_LIMIT: usize = 64 * 1024;

const TOOL_CALLS: usize = 16;
// Bound complete approval displays, including captured schedule context and channel text.
const APPROVAL_ARGUMENT_LIMIT: usize = 512;
const APPROVAL_SCHEDULE_LIMIT: usize = 1_024;
const APPROVAL_TEXT_LIMIT: usize = 2_000;
const TURN_LIMIT: std::time::Duration = std::time::Duration::from_secs(300);
const TOKEN_LIMIT: u64 = 128_000;
const ARCHIVE_LIMIT: u64 = 64 * 1024 * 1024;
const ARCHIVE_EXPANDED: u64 = 256 * 1024 * 1024;
const ARCHIVE_FILES: usize = 10_000;
const DOWNLOAD_CONNECT: u64 = 10;
const DOWNLOAD_TIME: u64 = 120;
const MEDIA_LIMIT: u64 = 4 * 1024 * 1024;
const IMAGE_LIMIT: usize = 4 * 1024 * 1024;
const IMAGE_TOTAL_LIMIT: usize = 4 * 1024 * 1024;
const FILE_LIMIT: u64 = 64 * 1024;
const TEXT_LIMIT: usize = krabb_core::types::TEXT_LIMIT;
const META_LIMIT: usize = 4 * 1024;
const CONTENT_LIMIT: usize = krabb_core::types::CONTENT_LIMIT;
const CONTEXT_LIMIT: usize = 8 * 1024;
const TOOL_ARGS_LIMIT: usize = 64 * 1024;
const TOOL_CONTENT_LIMIT: usize = 2 * 1024 * 1024;
const MANIFEST_LIMIT: u64 = 1024 * 1024;
const LOCK_LIMIT: u64 = 8 * 1024 * 1024;
const COMMAND_TIME: Duration = Duration::from_secs(120);
const SCHEDULER_CALL_TIME: Duration = Duration::from_secs(10);
const SCHEDULE_CONCURRENCY: usize = 4;
const SCHEDULE_CALL_RANGE: u64 = 1 << 20;
type WorkspaceLocks = Arc<AsyncMutex<HashMap<PathBuf, Arc<AsyncMutex<()>>>>>;
const COMMAND_OUTPUT: usize = 2 * 1024 * 1024;
/// Task tools the host owns; plugins may not register these names.
const HOST_TOOLS: [&str; 6] =
    ["task", "job", "checkpoint", jobs::input::NAME, slash::TOOL, settings::TOOL];

const ACK_ATTEMPTS: u32 = 3;
const TOOL_CALL_PROTOCOL_MINOR: u16 = 1;
type PluginError = Box<dyn std::error::Error + Send + Sync>;

pub(crate) struct Stop {
    signal: watch::Sender<bool>,
}

#[derive(Clone)]
pub(crate) struct Cancellation {
    signal: watch::Sender<bool>,
    acknowledged: watch::Sender<bool>,
}

impl Cancellation {
    fn new() -> Self {
        let (signal, _) = watch::channel(false);
        let (acknowledged, _) = watch::channel(false);
        Self { signal, acknowledged }
    }

    fn request(&self) {
        self.signal.send_replace(true);
    }

    fn requested(&self) -> bool {
        *self.signal.borrow()
    }

    fn acknowledge(&self) {
        self.acknowledged.send_replace(true);
    }

    async fn cancelled(&self) {
        let mut signal = self.signal.subscribe();

        if !*signal.borrow() {
            let _ = signal.changed().await;
        }
    }

    async fn wait(&self) {
        let mut acknowledged = self.acknowledged.subscribe();

        if !*acknowledged.borrow() {
            let _ = acknowledged.changed().await;
        }
    }
}

#[derive(Clone)]
pub(crate) struct Plugins {
    config: Arc<Config>,
    selection: Arc<Mutex<Option<selection::Policy>>>,
    items: Arc<RwLock<BTreeMap<String, Arc<Live>>>>,
    detached: Arc<RwLock<BTreeSet<String>>>,
    loading: Arc<AsyncMutex<()>>,
    changed: Arc<Notify>,
    lifecycle: Arc<RwLock<()>>,
    jobs: Arc<jobs::Signals>,
    workspaces: WorkspaceLocks,
    clients: Arc<clients::Registry>,
    inference: Arc<inference::History>,
    operations: Arc<Mutex<Option<operations::Catalog>>>,
}

#[derive(Clone, Debug, Default)]
struct ToolRegistry {
    specs: Vec<ToolSpec>,
    owners: BTreeMap<String, String>,
    operations: BTreeMap<String, operations::Offer>,
    cleanup: process::Cleanup,
    task: Arc<tasks::State>,
    source: Option<sharing::Principal>,
    execution: state::SessionKind,
    run: Option<krabb_core::task::Run>,
    restart: Arc<std::sync::atomic::AtomicBool>,
}

impl Default for Plugins {
    fn default() -> Self {
        Self {
            config: Arc::new(Config::default()),
            selection: Arc::new(Mutex::new(None)),
            items: Arc::new(RwLock::new(BTreeMap::new())),
            detached: Arc::new(RwLock::new(BTreeSet::new())),
            loading: Arc::new(AsyncMutex::new(())),
            changed: Arc::new(Notify::new()),
            lifecycle: Arc::new(RwLock::new(())),
            jobs: Arc::new(jobs::Signals::default()),
            workspaces: Arc::new(AsyncMutex::new(HashMap::new())),
            clients: Arc::new(clients::Registry::default()),
            inference: Arc::new(inference::History::default()),
            operations: Arc::default(),
        }
    }
}

impl Plugins {
    fn channel_selection(
        &self,
        channel: &str,
        provider: &str,
        model: &str,
    ) -> Result<(String, String), PluginError> {
        Ok(self.selection.lock().map_err(|_| "Selection lock is poisoned.")?.as_ref().map_or_else(
            || (provider.to_owned(), model.to_owned()),
            |policy| policy.channel(channel),
        ))
    }
}

/// A plugin's advertised identity over a process slot that survives restarts and updates.
pub(crate) struct Live {
    hello: Hello,
    process: Arc<AsyncMutex<Option<Process>>>,
    turns: Arc<turns::Turns>,
    restart_generation: Arc<std::sync::atomic::AtomicU64>,
}

struct CancelPluginCall {
    process: Arc<AsyncMutex<Option<Process>>>,
    id: u64,
    armed: bool,
}

impl CancelPluginCall {
    fn new(process: Arc<AsyncMutex<Option<Process>>>, id: u64) -> Self {
        Self { process, id, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CancelPluginCall {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }

        let process = Arc::clone(&self.process);
        let id = self.id;

        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                if let Some(plugin) = process.lock().await.as_mut() {
                    let request = Request::Note {
                        jsonrpc: "2.0".into(),
                        method: "cancel".into(),
                        params: serde_json::json!({"id": id}),
                    };

                    let _ = plugin.notify(request).await;
                }
            });
        }
    }
}

impl Live {
    fn new(process: impl Into<Process>) -> Self {
        let process = process.into();

        Self {
            hello: process.hello.clone(),
            process: Arc::new(AsyncMutex::new(Some(process))),
            turns: Arc::default(),
            restart_generation: Arc::default(),
        }
    }

    /// The same slot under a new version, so callers holding this one reach the new process.
    fn successor(&self, hello: Hello) -> Self {
        Self {
            hello,
            process: Arc::clone(&self.process),
            turns: Arc::clone(&self.turns),
            restart_generation: Arc::clone(&self.restart_generation),
        }
    }

    fn supports(&self, capability: Capability) -> bool {
        self.hello.capabilities.contains(&capability)
    }

    fn restart_generation(&self) -> u64 {
        self.restart_generation.load(std::sync::atomic::Ordering::Relaxed)
    }

    async fn call(&self, request: Request) -> krabb_core::Result<Response> {
        let cancel = request.id().map(|id| CancelPluginCall::new(Arc::clone(&self.process), id));

        let result = {
            let mut slot = self.process.lock().await;
            let process = slot
                .as_mut()
                .ok_or_else(|| krabb_core::Error::Protocol("Plugin is stopped.".into()))?;

            // The running version's formats apply, since an update may have replaced it.
            let request = messaging::prepare(request, &process.hello.formats);

            process.call(request).await
        };

        if let Some(mut cancel) = cancel {
            cancel.disarm();
        }

        result
    }

    async fn restart(&self) -> krabb_core::Result<()> {
        self.process
            .lock()
            .await
            .as_mut()
            .ok_or_else(|| krabb_core::Error::Protocol("Plugin is stopped.".into()))?
            .restart()
            .await?;

        self.restart_generation.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    async fn stop(&self) -> krabb_core::Result<()> {
        if let Some(process) = self.process.lock().await.take() {
            process.stop().await
        } else {
            Ok(())
        }
    }
}

impl Plugins {
    async fn get(&self, id: &str) -> Option<Arc<Live>> {
        self.items.read().await.get(id).cloned()
    }

    async fn insert(&self, plugin: Live) -> Option<Arc<Live>> {
        let old = self.items.write().await.insert(plugin.hello.id.clone(), Arc::new(plugin));
        self.changed.notify_waiters();
        self.changed.notify_one();
        old
    }

    async fn remove(&self, id: &str) -> Option<Arc<Live>> {
        let plugin = self.items.write().await.remove(id);

        if plugin.is_some() {
            self.changed.notify_waiters();
            self.changed.notify_one();
        }

        plugin
    }

    async fn find(&self, capability: Capability) -> Option<(String, Arc<Live>)> {
        self.items
            .read()
            .await
            .iter()
            .find(|(_, plugin)| plugin.hello.capabilities.contains(&capability))
            .map(|(id, plugin)| (id.clone(), Arc::clone(plugin)))
    }

    async fn all(&self) -> Vec<(String, Arc<Live>)> {
        self.items
            .read()
            .await
            .iter()
            .map(|(id, plugin)| (id.clone(), Arc::clone(plugin)))
            .collect()
    }
}

async fn tool_registry(
    plugins: &Plugins,
    enabled: bool,
    shell_enabled: bool,
) -> Result<ToolRegistry, Box<dyn std::error::Error + Send + Sync>> {
    let mut registry = ToolRegistry::default();

    if !enabled {
        return Ok(registry);
    }

    for (plugin_id, plugin) in plugins.all().await {
        if !plugin.supports(Capability::Tool) {
            continue;
        }

        let process = Arc::clone(&plugin.process).lock_owned().await;
        let response = tokio::time::timeout(
            Duration::from_secs(2),
            registry
                .cleanup
                .call_locked(process, Request::call(1, "tools/list", serde_json::json!({}))),
        )
        .await;
        registry.cleanup.finish().await?;
        let definitions = match response {
            Ok(Ok(response)) if response.error.is_none() => {
                let result = response.result.ok_or("Tool listing returned no result.")?;
                serde_json::from_value::<Vec<ToolSpec>>(result["tools"].clone())?
            }

            Ok(Ok(response))
                if plugin_id == "tools"
                    && response.error.as_ref().is_some_and(|error| error.code == -32601) =>
            {
                tools(shell_enabled)
            }

            Ok(Ok(_)) if plugin_id == "tools" => tools(shell_enabled),

            Ok(Ok(response)) => {
                if let Some(error) = response.error {
                    return Err(format!(
                        "Plugin {plugin_id} could not list tools: {}",
                        error.message
                    )
                    .into());
                }

                return Err(format!("Plugin {plugin_id} returned an invalid tool listing.").into());
            }

            Ok(Err(error)) if plugin_id == "tools" => {
                let _ = error;
                tools(shell_enabled)
            }

            Ok(Err(error)) => {
                return Err(format!("Plugin {plugin_id} tool listing failed: {error}").into());
            }

            Err(_) if plugin_id == "tools" => tools(shell_enabled),

            Err(_) => return Err(format!("Plugin {plugin_id} tool listing timed out.").into()),
        };

        if definitions.len() > 128 {
            return Err(format!("Plugin {plugin_id} listed too many tools.").into());
        }

        for definition in definitions {
            validate_tool_spec(&definition)?;

            if registry.owners.insert(definition.name.clone(), plugin_id.clone()).is_some() {
                return Err(
                    format!("Multiple plugins registered tool `{}`.", definition.name).into()
                );
            }

            registry.specs.push(definition);
        }
    }

    Ok(registry)
}

fn validate_tool_spec(spec: &ToolSpec) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if spec.name.is_empty()
        || spec.name.len() > 128
        || !spec
            .name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        || !spec.schema.is_object()
        || spec.schema["type"] != "object"
        || !spec.schema["properties"].is_object()
        || serde_json::to_vec(&spec.schema).is_ok_and(|schema| schema.len() > 64 * 1024)
        || spec.description.as_ref().is_some_and(|description| description.len() > 4_096)
        || validate_schema_definition(&spec.schema, 0).is_err()
    {
        return Err(format!("Tool `{}` has an invalid definition.", spec.name).into());
    }

    Ok(())
}

fn validate_schema_definition(schema: &Value, depth: usize) -> Result<(), String> {
    if depth > 8 || !schema.is_object() {
        return Err("Tool schema is too deep or malformed.".into());
    }

    let kind = schema["type"].as_str().ok_or("Tool schema must declare a type.")?;

    if !matches!(kind, "object" | "array" | "string" | "integer" | "number" | "boolean") {
        return Err("Tool schema declares an unsupported type.".into());
    }

    if let Some(properties) = schema.get("properties") {
        let properties =
            properties.as_object().ok_or("Tool schema properties must be an object.")?;

        if properties.len() > 128 {
            return Err("Tool schema declares too many properties.".into());
        }

        for property in properties.values() {
            validate_schema_definition(property, depth + 1)?;
        }
    }

    if let Some(items) = schema.get("items") {
        validate_schema_definition(items, depth + 1)?;
    }

    if let Some(required) = schema.get("required") {
        let required = required.as_array().ok_or("Tool schema required must be an array.")?;

        if required.len() > 128 || required.iter().any(|item| !item.is_string()) {
            return Err("Tool schema has invalid required fields.".into());
        }
    }

    if schema.get("additionalProperties").is_some_and(|value| !value.is_boolean()) {
        return Err("Tool schema additionalProperties must be boolean.".into());
    }

    Ok(())
}

fn validate_registered_args(
    registry: &ToolRegistry,
    name: &str,
    args: &Value,
) -> Result<(), String> {
    let Some(schema) =
        registry.specs.iter().find(|spec| spec.name == name).map(|spec| &spec.schema)
    else {
        return Ok(());
    };

    let mut model_args = args.clone();

    if let Value::Object(values) = &mut model_args {
        values.remove("approve");
    }

    validate_schema_value(schema, &model_args, 0)
}

fn validate_schema_value(schema: &Value, value: &Value, depth: usize) -> Result<(), String> {
    if depth > 8 {
        return Err("Tool arguments exceed the schema depth limit.".into());
    }

    if let Some(expected) = schema.get("const")
        && value != expected
    {
        return Err("Tool arguments do not match the declared constant.".into());
    }

    if let Some(choices) = schema.get("enum")
        && !choices.as_array().is_some_and(|choices| choices.contains(value))
    {
        return Err("Tool arguments do not match an allowed value.".into());
    }

    let kind = schema["type"].as_str().ok_or("Tool schema must declare a type.")?;
    let valid = match kind {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        _ => false,
    };

    if !valid {
        return Err(format!("Tool argument must be {kind}."));
    }

    if let Some(object) = value.as_object() {
        let properties = schema["properties"].as_object();

        if let Some(required) = schema["required"].as_array() {
            for required in required.iter().filter_map(Value::as_str) {
                if !object.contains_key(required) {
                    return Err(format!("Tool argument `{required}` is required."));
                }
            }
        }

        if schema["additionalProperties"] == false
            && properties
                .is_some_and(|properties| object.keys().any(|key| !properties.contains_key(key)))
        {
            return Err("Tool arguments contain an undeclared property.".into());
        }

        if let Some(properties) = properties {
            for (key, value) in object {
                if let Some(property) = properties.get(key) {
                    validate_schema_value(property, value, depth + 1)?;
                }
            }
        }
    }

    if let Some(array) = value.as_array() {
        if let Some(items) = schema.get("items") {
            for item in array {
                validate_schema_value(items, item, depth + 1)?;
            }
        }

        if schema["minItems"].as_u64().is_some_and(|minimum| (array.len() as u64) < minimum)
            || schema["maxItems"].as_u64().is_some_and(|maximum| (array.len() as u64) > maximum)
        {
            return Err("Tool array argument is outside its allowed size.".into());
        }
    }

    if let Some(string) = value.as_str() {
        let length = string.chars().count() as u64;

        if schema["minLength"].as_u64().is_some_and(|minimum| length < minimum)
            || schema["maxLength"].as_u64().is_some_and(|maximum| length > maximum)
        {
            return Err("Tool text argument is outside its allowed length.".into());
        }
    }

    if let Some(number) = value.as_f64()
        && (schema["minimum"].as_f64().is_some_and(|minimum| number < minimum)
            || schema["maximum"].as_f64().is_some_and(|maximum| number > maximum))
    {
        return Err("Tool numeric argument is outside its allowed range.".into());
    }

    Ok(())
}

impl Stop {
    fn new() -> Self {
        let (signal, _) = watch::channel(false);
        Self { signal }
    }

    fn signal(&self) {
        self.signal.send_replace(true);
    }

    fn is_signaled(&self) -> bool {
        *self.signal.borrow()
    }

    async fn notified(&self) {
        let mut signal = self.signal.subscribe();

        if *signal.borrow() {
            return;
        }

        let _ = signal.changed().await;
    }
}

fn sentence(value: impl Into<String>) -> String {
    let mut value = value.into().trim().to_owned();

    while let Some(message) = value.strip_prefix("Denied: ") {
        value = message.trim().to_owned();
    }

    if value.is_empty() {
        return "Unknown error.".into();
    }

    if let Some(first) = value.chars().next() {
        let mut title = first.to_uppercase().collect::<String>();
        title.push_str(&value[first.len_utf8()..]);
        value = title;
    }

    if !matches!(value.chars().last(), Some('.' | '!' | '?')) {
        value.push('.');
    }

    value
}

fn format_cli_error(message: &str) -> String {
    if message.trim().is_empty() {
        return "The command could not be completed.".into();
    }

    message.lines().map(str::trim_start).collect::<Vec<_>>().join("\n")
}

fn toml_position(input: &str, offset: usize) -> (usize, usize) {
    let bytes = input.as_bytes();
    let offset = offset.min(bytes.len());
    let line = bytes[..offset].iter().filter(|byte| **byte == b'\n').count() + 1;
    let column = bytes[..offset]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(offset, |position| offset.saturating_sub(position + 1))
        + 1;

    (line, column)
}

fn diagnostic(value: impl AsRef<str>) -> String {
    redact_diagnostic(value)
}

pub fn redact_diagnostic(value: impl AsRef<str>) -> String {
    krabb_log::redact_diagnostic(value)
}

fn git_command() -> std::process::Command {
    let mut command = std::process::Command::new("git");

    for variable in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ] {
        command.env_remove(variable);
    }

    command
}

fn command_output(command: &mut std::process::Command) -> std::io::Result<std::process::Output> {
    command_output_limited(command, None, u64::MAX)
}

fn command_output_limited(
    command: &mut std::process::Command,
    root: Option<&Path>,
    limit: u64,
) -> std::io::Result<std::process::Output> {
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let stdout = child.stdout.take().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::BrokenPipe, "Process has no stdout.")
    })?;

    let stderr = child.stderr.take().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::BrokenPipe, "Process has no stderr.")
    })?;

    let (output_tx, output_rx) = std::sync::mpsc::channel();
    let (error_tx, error_rx) = std::sync::mpsc::channel();
    let _output_thread = thread::spawn(move || {
        let _ = output_tx.send(read_output(stdout));
    });

    let _error_thread = thread::spawn(move || {
        let _ = error_tx.send(read_output(stderr));
    });

    let deadline = Instant::now() + COMMAND_TIME;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }

        if let Some(root) = root {
            let size = match staging_size(root) {
                Ok(size) => size,

                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(error);
                }
            };

            if size > limit {
                let _ = child.kill();
                let _ = child.wait();
                return Err(std::io::Error::new(
                    std::io::ErrorKind::FileTooLarge,
                    "Process staging exceeds the size limit.",
                ));
            }
        }

        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Process exceeded the execution time limit.",
            ));
        }

        thread::sleep(Duration::from_millis(25));
    };

    if let Some(root) = root
        && staging_size(root)? > limit
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::FileTooLarge,
            "Process staging exceeds the size limit.",
        ));
    }

    let stdout = output_rx.recv_timeout(Duration::from_secs(1)).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::TimedOut, "Process output did not close.")
    })??;

    let stderr = error_rx.recv_timeout(Duration::from_secs(1)).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::TimedOut, "Process error output did not close.")
    })??;

    Ok(std::process::Output { status, stdout, stderr })
}

fn staging_size(root: &Path) -> std::io::Result<u64> {
    let mut size = 0_u64;

    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };

        size = size.saturating_add(if metadata.is_dir() {
            match staging_size(&path) {
                Ok(size) => size,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
                Err(error) => return Err(error),
            }
        } else {
            metadata.len()
        });
    }

    Ok(size)
}

fn read_output(input: impl Read) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::new();
    input.take((COMMAND_OUTPUT + 1) as u64).read_to_end(&mut output)?;

    if output.len() > COMMAND_OUTPUT {
        return Err(std::io::Error::new(
            std::io::ErrorKind::FileTooLarge,
            "Process output exceeds the size limit.",
        ));
    }

    Ok(output)
}

struct DaemonLock {
    _file: std::fs::File,
    _legacy: Option<Box<DaemonLock>>,
}

impl Drop for DaemonLock {
    fn drop(&mut self) {
        let _ = self._file.unlock();
    }
}

impl Drop for SourceRoot {
    fn drop(&mut self) {
        if let Some(path) = &self.temp {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

#[derive(Debug, Args)]
struct Name {
    #[arg(help = "Identifier.")]
    id: String,
    #[arg(short, long, help = "Skip the confirmation prompt.")]
    yes: bool,
}

#[derive(Debug, Args)]
struct PluginUninstall {
    #[arg(required = true, value_name = "ID", help = "Plugin identifiers.")]
    ids: Vec<String>,
    #[arg(short, long, help = "Skip the confirmation prompt.")]
    yes: bool,
    #[arg(
        long,
        help = "Purge session/delivery state when removing the last provider or channel plugin."
    )]
    force: bool,
}

struct PluginRemove {
    id: String,
    yes: bool,
}

#[derive(Debug, Args)]
struct Id {
    #[arg(help = "Identifier.")]
    id: String,
}

#[derive(Debug, Args)]
struct Ask {
    #[arg(long, help = "Intelligence provider (overrides environment and config).")]
    provider: Option<String>,
    #[arg(long, help = "Model identifier (overrides environment and config).")]
    model: Option<String>,
    #[arg(long, help = "Quoted message supplied as bounded reference context.")]
    quote: Option<String>,
    #[arg(required = true, trailing_var_arg = true, help = "Prompt words.")]
    prompt: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub(crate) struct Config {
    provider: Option<String>,
    model: Option<String>,
    name: String,
    update: String,
    #[serde(default)]
    shell: bool,
    approval: String,
    tools: bool,
    #[serde(default)]
    scheduler: SchedulerConfig,
    inference: inference::Config,
    #[serde(default)]
    operations: operations::Config,
    #[serde(default)]
    channels: BTreeMap<String, ChannelConfig>,
    #[serde(default)]
    visibility: Vec<sharing::Rule>,
    #[serde(default)]
    clients: BTreeMap<String, ClientConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            provider: None,
            model: None,
            name: "Krabb".into(),
            update: "prompt".into(),
            shell: false,
            approval: "off".into(),
            tools: false,
            scheduler: SchedulerConfig::default(),
            inference: inference::Config::default(),
            operations: operations::Config::default(),
            channels: BTreeMap::new(),
            visibility: Vec::new(),
            clients: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
#[serde(deny_unknown_fields)]
struct SchedulerConfig {
    timezone: Option<String>,
    #[serde(default = "skip_missed")]
    missed: String,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self { timezone: None, missed: skip_missed() }
    }
}

fn skip_missed() -> String {
    "skip".into()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ClientConfig {
    provider: Option<String>,
    model: Option<String>,
    #[serde(default)]
    tools: bool,
    shell: Option<bool>,
    #[serde(default = "default_tui_theme")]
    theme: bool,
    #[serde(default)]
    worktree: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    visibility: Option<Vec<sharing::Rule>>,
    #[serde(default)]
    approval: Option<String>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            provider: None,
            model: None,
            tools: false,
            shell: None,
            theme: true,
            worktree: false,
            visibility: None,
            approval: None,
        }
    }
}

fn default_tui_theme() -> bool {
    true
}

impl Config {
    fn tui_shell(&self) -> bool {
        self.client_shell(client::TUI)
    }

    fn client_shell(&self, id: &str) -> bool {
        self.clients.get(id).and_then(|client| client.shell).unwrap_or(self.shell)
    }

    fn validate(&self) -> Result<(), String> {
        context::validate(self)?;
        self.inference.validate()?;
        self.operations.validate()?;
        selection::validate(self.provider.as_deref(), self.model.as_deref())?;

        for (id, channel) in &self.channels {
            if !valid(id) {
                return Err(
                    "Channel IDs must contain lowercase letters, digits, or hyphens.".into()
                );
            }

            if client::known(id) {
                return Err(format!("Channel ID `{id}` is reserved for a built-in client."));
            }

            selection::validate(channel.provider.as_deref(), channel.model.as_deref())?;
        }

        for client in self.clients.values() {
            selection::validate(client.provider.as_deref(), client.model.as_deref())?;
        }

        if self.clients.keys().any(|id| !valid(id)) {
            return Err("Client IDs must contain lowercase letters, digits, or hyphens.".into());
        }

        if self.name.trim().is_empty()
            || self.name.len() > 64
            || self.name.chars().any(char::is_control)
        {
            return Err("The Krabb name must contain 1–64 visible characters.".into());
        }

        match self.update.as_str() {
            "off" | "check" | "prompt" | "auto" => Ok(()),
            value => Err(format!("Unsupported update mode: {value}.")),
        }?;

        match self.approval.as_str() {
            "off" | "prompt" | "auto" => Ok(()),
            value => Err(format!("Unsupported approval mode: {value}.")),
        }?;

        if let Some(timezone) = &self.scheduler.timezone {
            jiff::tz::TimeZone::get(timezone)
                .map_err(|_| format!("Scheduler timezone is invalid: {timezone}."))?;
        }

        match self.scheduler.missed.as_str() {
            "skip" | "coalesce" => Ok(()),
            value => Err(format!("Unsupported scheduler missed-run policy: {value}.")),
        }
    }

    fn approval_mode(&self) -> ApprovalMode {
        match self.approval.as_str() {
            "prompt" => ApprovalMode::Prompt,
            "auto" => ApprovalMode::Auto,
            _ => ApprovalMode::Off,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ApprovalMode {
    Off,
    Prompt,
    Auto,
}

impl ApprovalMode {
    const fn enabled(self) -> bool {
        !matches!(self, Self::Off)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ChannelConfig {
    provider: Option<String>,
    model: Option<String>,
    #[serde(default)]
    allow: Vec<String>,
    mention: Option<String>,
    owner: Option<String>,
    #[serde(default)]
    admin: Vec<String>,
    #[serde(default)]
    member: Vec<String>,
    #[serde(default)]
    topic: Vec<String>,
    #[serde(default)]
    thread: Vec<String>,
    #[serde(default)]
    worktree: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    visibility: Option<Vec<sharing::Rule>>,
    #[serde(default)]
    approval: Option<String>,
    #[serde(default)]
    tools: bool,
    #[serde(default)]
    shell: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Manifest {
    id: String,
    version: String,
    protocol: Protocol,
    capabilities: Vec<String>,
    #[serde(default)]
    permissions: Vec<String>,
    /// Names of secret environment variables, never credential values.
    #[serde(default, rename = "secrets")]
    variables: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    settings: Vec<setup::Setting>,
    /// Labels, explanations, and value kinds for secrets and settings, keyed by name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    setup: BTreeMap<String, setup::Prompt>,
    /// Whether the plugin answers `check` to test its saved credentials during setup.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    check: bool,
    #[serde(default)]
    commands: Vec<CommandSpec>,
}

impl Manifest {
    fn validate(&self) -> Result<(), String> {
        if !valid(&self.id) {
            return Err("Plugin IDs must contain lowercase letters, digits, or hyphens.".into());
        }

        if self.version.trim().is_empty() {
            return Err("Plugin versions must not be empty.".into());
        }

        if !Protocol::CURRENT.compatible(self.protocol) {
            return Err("Plugin protocol is not compatible with this daemon.".into());
        }

        let mut capabilities = self.capabilities.clone();
        capabilities.sort();
        capabilities.dedup();

        if capabilities.len() != self.capabilities.len()
            || self.capabilities.iter().any(|capability| {
                !(serde_json::from_value::<Capability>(Value::String(capability.clone())).is_ok()
                    || self.id == "timer" && capability == "timer")
            })
        {
            return Err("Plugin capabilities must be known and unique.".into());
        }

        let known_permissions =
            ["network", "process", "filesystem", "memory", "scheduler", "terminal", "audio"];

        if self
            .permissions
            .iter()
            .any(|permission| !known_permissions.contains(&permission.as_str()))
        {
            return Err("Plugin permissions must be known.".into());
        }

        let mut commands =
            self.commands.iter().map(|command| command.name.clone()).collect::<Vec<_>>();
        commands.sort();
        commands.dedup();

        if commands.len() != self.commands.len()
            || self
                .commands
                .iter()
                .any(|command| !valid(&command.name) || command.description.trim().is_empty())
        {
            return Err("Plugin commands must be named uniquely and described.".into());
        }

        let mut variables = self.variables.clone();
        variables.extend(self.settings.iter().map(|setting| setting.name.clone()));
        variables.sort();
        variables.dedup();

        if variables.len() != self.variables.len() + self.settings.len()
            || variables.iter().any(|name| {
                name.is_empty()
                    || !name.starts_with("KRABB_")
                    || !name.bytes().all(|byte| {
                        byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_'
                    })
            })
        {
            return Err(
                "Plugin secrets and settings must be unique KRABB_ environment variable names."
                    .into(),
            );
        }

        for setting in &self.settings {
            if let Some(default) = &setting.default {
                setup::validate(default)?;
            }
        }

        let defaults = self
            .settings
            .iter()
            .map(|setting| (setting.name.clone(), setting.default.clone()))
            .collect::<Vec<_>>();

        setup::validate_prompts(&self.setup, &variables, &defaults)?;

        Ok(())
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct Lock {
    plugins: BTreeMap<String, Entry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Entry {
    source: String,
    #[serde(default)]
    revision: String,
    #[serde(default)]
    pinned: bool,
    #[serde(default)]
    default: bool,
    #[serde(default)]
    hash: String,
    version: String,
    protocol: Protocol,
    capabilities: Vec<String>,
    #[serde(default)]
    permissions: Vec<String>,
    /// Names of secret environment variables, never credential values.
    #[serde(default, rename = "secrets")]
    variables: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    settings: Vec<setup::Setting>,
    #[serde(default)]
    linked: bool,
    #[serde(default)]
    commands: Vec<CommandSpec>,
}

struct Update {
    destination: PathBuf,
    backup: Option<PathBuf>,
    linked: bool,
}

pub async fn cli() -> ExitCode {
    let mut args = std::env::args().skip(1).collect::<Vec<_>>();

    if root_help_requested(&args) {
        return match print_help_with_plugins(&home()) {
            Ok(()) => ExitCode::SUCCESS,

            Err(error) => {
                eprintln!("{}", diagnostic(sentence(error.to_string())));
                ExitCode::FAILURE
            }
        };
    }

    if args.first().is_some_and(|name| name == krabb_core::client::TUI)
        && args.get(1).is_some_and(|argument| argument == "--help" || argument == "-h")
    {
        println!(
            "{}",
            style_cli_help(&external_command_help("tui", "Open the Krabb terminal interface."))
        );
        return ExitCode::SUCCESS;
    }

    if args.first().is_some_and(|name| name == "memory")
        && args.get(1).is_some_and(|argument| argument == "--help" || argument == "-h")
    {
        args[1] = "help".into();
    }

    if args.first().is_some_and(|name| name == "data")
        && args.get(1).is_some_and(|argument| argument == "--help" || argument == "-h")
    {
        args[1] = "help".into();
    }

    let matches = cli_command().get_matches_from(std::iter::once(NAME.to_owned()).chain(args));

    let mut cli = Cli::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());

    let runtime_command_args = match &cli.command {
        Command::Memory(command_args) | Command::Tui(command_args) => {
            Some((command_args.args.as_slice(), true))
        }

        Command::Data(command_args) => Some((command_args.args.as_slice(), false)),

        Command::External(args)
            if setup::requested(args.get(1..).unwrap_or_default())
                || args.first().is_some_and(|name| {
                    matches!(name.as_str(), "session" | "delivery" | "schedule")
                }) =>
        {
            Some((args.as_slice(), true))
        }

        _ => None,
    };

    if let Some((args, skip_first)) = runtime_command_args {
        for argument in args.iter().skip(usize::from(skip_first)) {
            match argument.as_str() {
                "--json" => cli.json = true,
                "--debug" => cli.debug = true,
                "--verbose" => cli.verbose = true,

                _ => {}
            }
        }
    }

    let log_path = init_logging_with_command_log(cli.verbose, cli.debug, cli.json);

    if let Some(path) = log_path {
        main_with_log(cli, Some(path)).await
    } else {
        main_with(cli).await
    }
}

fn root_help_requested(args: &[String]) -> bool {
    if args.is_empty() {
        return true;
    }

    let mut command_seen = false;

    for (index, argument) in args.iter().enumerate() {
        match argument.as_str() {
            "--json" | "--debug" | "--verbose" => {}

            "-h" | "-H" | "--help" if !command_seen => return true,

            "help"
                if !command_seen
                    && args[index + 1..].iter().all(|argument| {
                        matches!(argument.as_str(), "--json" | "--debug" | "--verbose")
                    }) =>
            {
                return true;
            }

            _ => command_seen = true,
        }
    }

    false
}

pub async fn daemon() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    logging::initialize(logging::Mode::Daemon);
    serve().await
}

#[cfg(windows)]
pub fn service_dispatch() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    logging::initialize(logging::Mode::Daemon);
    krabb_process::service::dispatch("Krabb", service_main)?;
    Ok(())
}

#[cfg(windows)]
fn service_main() {
    let stop = Arc::new(Stop::new());
    let signal = Arc::clone(&stop);
    let service = match krabb_process::service::Service::register("Krabb", move || signal.signal())
    {
        Ok(service) => service,

        Err(error) => {
            error!(error = %diagnostic(error.to_string()), "Service registration failed.");
            return;
        }
    };

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(async {
            let root = home();
            let daemon = serve_controlled(&root, stop, || service.running());
            tokio::pin!(daemon);
            let mut heartbeat = tokio::time::interval(Duration::from_secs(1));

            loop {
                tokio::select! {
                    result = &mut daemon => break result,
                    _ = heartbeat.tick() => service.heartbeat()?,
                }
            }
        })
    }));

    let failed = !matches!(&outcome, Ok(Ok(())));

    if let Ok(Err(error)) = outcome {
        error!(error = %diagnostic(error.to_string()), "Service failed.");
    }

    if let Err(error) = service.stopped(failed) {
        error!(error = %diagnostic(error.to_string()), "Service status failed.");
    }
}

pub fn init_logging(verbose: bool, debug_mode: bool, json: bool) {
    let verbosity = if debug_mode {
        logging::Verbosity::Debug
    } else if verbose {
        logging::Verbosity::Verbose
    } else {
        logging::Verbosity::Quiet
    };

    logging::initialize(logging::Mode::Command { verbosity, json });
}

fn init_logging_with_command_log(verbose: bool, debug_mode: bool, json: bool) -> Option<PathBuf> {
    let verbosity = if debug_mode {
        logging::Verbosity::Debug
    } else if verbose {
        logging::Verbosity::Verbose
    } else {
        logging::Verbosity::Quiet
    };

    logging::initialize_with_command_log(logging::Mode::Command { verbosity, json })
}

async fn main_with(cli: Cli) -> ExitCode {
    main_with_log(cli, None).await
}

async fn main_with_log(cli: Cli, log_path: Option<PathBuf>) -> ExitCode {
    let json = cli.json;
    let debug = cli.debug;
    let verbose = cli.verbose;
    let command = command_label(&cli.command);
    let started = Instant::now();

    if verbose || debug {
        info!(command, "Command started.");

        if verbose && !json {
            eprintln!("Running {command}…");
        }
    }

    match run(cli).await {
        Ok(()) => {
            if verbose || debug {
                let elapsed_ms = started.elapsed().as_millis();
                info!(command, elapsed_ms, "Command completed.");

                if verbose && !json {
                    eprintln!("Finished {command} in {elapsed_ms} ms.");
                }
            }

            if let Some(path) = log_path {
                info!(command, path = %path.display(), "Command log written.");

                if !json {
                    eprintln!("Command log written to {}.", path.display());
                }
            }

            ExitCode::SUCCESS
        }

        Err(error) => {
            if json {
                let message = diagnostic(sentence(error.to_string()));

                if debug {
                    error!(command, elapsed_ms = started.elapsed().as_millis(), "Command failed.");

                    let details =
                        format!("{error:?}").split_whitespace().collect::<Vec<_>>().join(" ");

                    debug!(
                        command,
                        error = %diagnostic(details),
                        elapsed_ms = started.elapsed().as_millis(),
                        "Command failure details."
                    );
                } else if verbose {
                    let causes = error_causes(error.as_ref()).join(": ");

                    if causes.is_empty() {
                        error!(
                            command,
                            error = %message,
                            elapsed_ms = started.elapsed().as_millis(),
                            "Command failed."
                        );
                    } else {
                        error!(
                            command,
                            error = %message,
                            causes = %diagnostic(&causes),
                            elapsed_ms = started.elapsed().as_millis(),
                            "Command failed."
                        );
                    }
                } else {
                    error!(command, error = %message, "Command failed.");
                }

                eprintln!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({"error": message}))
                        .expect("serializing a JSON error cannot fail")
                );
            } else {
                let message = sentence(error.to_string());
                let causes = error_causes(error.as_ref());
                let display = if verbose && !causes.is_empty() {
                    format!("{message}\n{}", causes.join("\n"))
                } else {
                    message.clone()
                };

                eprintln!("{}", format_cli_error(&diagnostic(&display)));

                if debug {
                    error!(command, elapsed_ms = started.elapsed().as_millis(), "Command failed.");
                    let details =
                        format!("{error:?}").split_whitespace().collect::<Vec<_>>().join(" ");
                    debug!(
                        command,
                        error = %diagnostic(details),
                        elapsed_ms = started.elapsed().as_millis(),
                        "Command failure details."
                    );
                } else if verbose {
                    let details = causes.join(": ");

                    if !details.is_empty() {
                        error!(
                            command,
                            causes = %diagnostic(&details),
                            elapsed_ms = started.elapsed().as_millis(),
                            "Command failed."
                        );
                    } else {
                        error!(
                            command,
                            elapsed_ms = started.elapsed().as_millis(),
                            "Command failed."
                        );
                    }
                } else {
                    error!(command, error = %diagnostic(&message), "Command failed.");
                }
            }

            if let Some(path) = log_path {
                info!(command, path = %path.display(), "Command log written.");

                if !json {
                    eprintln!("Command log written to {}.", path.display());
                }
            }

            ExitCode::FAILURE
        }
    }
}

fn command_label(command: &Command) -> &'static str {
    match command {
        Command::Init(_) => "init",
        Command::Hatch(_) => "hatch",
        Command::Doctor(_) => "doctor",
        Command::Config(_) => "config",
        Command::Worktree { .. } => "worktree",
        Command::Workspace { .. } => "workspace",
        Command::Status(_) => "status",
        Command::Version(_) => "version",
        Command::Completion { .. } => "completion",
        Command::Plugin { .. } => "plugin",
        Command::Ask(_) => "ask",
        Command::Schedule(_) => "schedule",
        Command::Memory(_) => "memory",
        Command::Tui(_) => "tui",
        Command::Data(_) => "data",
        Command::Service { .. } => "service",
        Command::Export(_) => "export",
        Command::Validate(_) => "validate",
        Command::Import(_) => "import",
        Command::Update(_) => "update",
        Command::External(args) => match args.first().map(String::as_str) {
            Some("session") => "session",
            Some("delivery") => "delivery",
            Some("schedule") => "schedule",
            Some("data") => "data",
            _ => "external",
        },
    }
}

fn error_causes(error: &(dyn std::error::Error + 'static)) -> Vec<String> {
    let mut causes = Vec::new();
    let mut source = error.source();

    while let Some(cause) = source {
        causes.push(cause.to_string());
        source = cause.source();
    }

    causes
}

async fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let json = cli.json;

    if schema_gated(&cli.command) {
        schema::check(&home())?;
    }

    match cli.command {
        Command::Init(args) => onboard::run(args, json).await?,
        Command::Hatch(args) => hatch::run(args, &home(), json).await?,
        Command::Doctor(args) => doctor(args.fix, json)?,
        Command::Config(args) => config_command(args.command, &home(), json).await?,
        Command::Worktree { command } => worktree::run(command, &home(), json).await?,

        Command::Workspace { path, owner } => {
            storage::assign(&home(), &workspace_root(), &path, &owner, json)?
        }

        Command::Status(output) => status_command(output.json || json).await?,
        Command::Version(output) => version(output.json || json),
        Command::Completion { shell } => completion(shell)?,
        Command::Plugin { command } => plugin(command, json).await?,
        Command::Ask(args) => ask_at(args, &home()).await?,

        Command::Schedule(args) => {
            let values = args.command.map(ScheduleCommand::into_values).unwrap_or_default();
            schedule_command(&values, &home(), json).await?;
        }

        Command::Memory(args) => {
            external_command(std::iter::once("memory".to_owned()).chain(args.args).collect(), json)
                .await?;
        }

        Command::Tui(args) => {
            external_command(std::iter::once("tui".to_owned()).chain(args.args).collect(), json)
                .await?;
        }

        Command::Data(args) => {
            external_command(std::iter::once("data".to_owned()).chain(args.args).collect(), json)
                .await?;
        }

        Command::Service { command } => service(command, json)?,
        Command::Export(args) => export_krabfile(args, json)?,
        Command::Validate(args) => validate_krabfile(args, json)?,
        Command::Import(args) => import_krabfile(args, json)?,
        Command::Update(options) => upgrade::run(options, &home(), json).await?,
        Command::External(args) => external_command(args, json).await?,
    }

    Ok(())
}

fn schema_gated(command: &Command) -> bool {
    !matches!(
        command,
        Command::Version(_)
            | Command::Completion { .. }
            | Command::Service { .. }
            | Command::Update(_)
            | Command::Init(InitArgs { force: true, .. })
    )
}

fn completion(shell: CompletionShell) -> std::io::Result<()> {
    let mut command = completion_command();
    let mut output = std::io::BufWriter::new(std::io::stdout().lock());
    let name = completion_name();

    generate_completion(shell, &mut command, name, &mut output);
    output.flush()
}

fn completion_command() -> clap::Command {
    let mut command = Cli::command();

    for name in ["schedule", "memory", "tui", "data"] {
        command = command.mut_subcommand(name, |subcommand| subcommand.hide(false));
    }

    command
}

fn generate_completion(
    shell: CompletionShell,
    command: &mut clap::Command,
    name: &str,
    output: &mut dyn Write,
) {
    match shell {
        CompletionShell::Bash => generate(shells::Bash, command, name, output),
        CompletionShell::Fish => generate(shells::Fish, command, name, output),
        CompletionShell::PowerShell => generate(shells::PowerShell, command, name, output),
        CompletionShell::Zsh => generate(shells::Zsh, command, name, output),
    }
}

fn completion_name() -> &'static str {
    completion_name_from(std::env::args_os().next().as_deref())
}

fn completion_name_from(argv0: Option<&std::ffi::OsStr>) -> &'static str {
    let Some(value) = argv0.and_then(|value| value.to_str()) else {
        return NAME;
    };

    let name = value.rsplit(['/', '\\']).next().unwrap_or(value);
    let name = match name.rsplit_once('.') {
        Some((stem, extension)) if extension.eq_ignore_ascii_case("exe") => stem,
        _ => name,
    };

    if name.eq_ignore_ascii_case("krabb") { "krabb" } else { NAME }
}

fn print_help_with_plugins(root: &Path) -> Result<(), PluginError> {
    print!("{}", style_cli_help(&help_text_with_plugins(root)?));
    Ok(())
}

fn cli_command() -> clap::Command {
    Cli::command().before_help(banner(terminal::columns()))
}

fn help_text_with_plugins(root: &Path) -> Result<String, PluginError> {
    let mut command = cli_command();
    let mut help = command.render_help().to_string();
    let commands = available_commands(root)?;

    let (conditional, plugins): (BTreeMap<_, _>, BTreeMap<_, _>) =
        commands.into_iter().partition(|(name, owners)| {
            native_command(name)
                && owners.iter().all(|(owner, _)| owner.starts_with(RUNTIME_COMMAND_OWNER))
        });

    let conditional_help = command_help_section("Conditional Commands", &conditional);
    let plugin_help = command_help_section("Plugin Commands", &plugins);

    if let Some(index) = help.find("\nOptions:") {
        help.insert_str(index, &format!("{conditional_help}{plugin_help}"));
    } else {
        help.push_str(&conditional_help);
        help.push_str(&plugin_help);
    }

    Ok(help)
}

fn command_help_section(
    title: &str,
    commands: &BTreeMap<String, Vec<(String, CommandSpec)>>,
) -> String {
    let mut help = format!("\n{title}:\n");

    if commands.is_empty() {
        help.push_str("  (none available)\n");
    } else {
        let width = commands.keys().map(String::len).max().unwrap_or_default();

        for (name, owners) in commands {
            let descriptions = owners
                .iter()
                .map(|(_, command)| command.description.as_str())
                .collect::<BTreeSet<_>>();

            let description = if descriptions.len() == 1 {
                descriptions.first().copied().unwrap_or_default()
            } else {
                "conflicting registrations"
            };

            help.push_str(&format!("  {name:<width$}  {description}\n"));
        }
    }

    help
}

async fn ask_at(ask: Ask, root: &Path) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let text = generate_at(ask, root, "cli").await?;

    println!("{text}");
    Ok(())
}

fn model_plugin_at(root: &Path, requested: &str) -> Result<String, PluginError> {
    let lock = load_lock_at(root)?;

    let available = lock
        .plugins
        .iter()
        .filter(|(id, entry)| model_plugin_available(id, entry, root))
        .map(|(id, _)| id.as_str())
        .collect::<Vec<_>>();

    if available.is_empty() {
        return Err(concat!(
            "No intelligence provider is installed and ready. ",
            "Install one with `krabb plugin install <id>` before running `krabb ask`."
        )
        .into());
    }

    if !available.contains(&requested) {
        return Err(format!(
            "Intelligence provider {requested} is unavailable. Ready providers: {}.",
            available.join(", ")
        )
        .into());
    }

    Ok(requested.to_owned())
}

fn model_plugin_available(id: &str, entry: &Entry, root: &Path) -> bool {
    entry.capabilities.iter().any(|capability| capability == "model")
        && binary_at(id, root).is_some()
}

fn session_model(model: Option<String>, root: &Path) -> Result<String, PluginError> {
    let config = load_config_at(root)?;

    config.validate()?;

    Ok(selection::client(&config, "cli", (None, model.as_deref())).1)
}

fn capture_model_selection(
    context: &mut serde_json::Value,
    provider: Option<String>,
    model: Option<String>,
) {
    if let Some(provider) = provider {
        context["provider"] = serde_json::json!(provider);
    }

    if let Some(model) = model.filter(|model| !model.trim().is_empty()) {
        context["model"] = serde_json::json!(model);
    }
}

async fn generate_at(
    ask: Ask,
    root: &Path,
    session: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let config = load_config_at(root)?;
    config.validate()?;
    let (provider, model) =
        selection::client(&config, "cli", (ask.provider.as_deref(), ask.model.as_deref()));
    let plugin = model_plugin_at(root, &provider)?;
    let prompt = ask.prompt.join(" ");
    let mut message = Message {
        id: "user".into(),
        session: session.into(),
        role: Role::User,
        sender: Some(session.into()),
        content: vec![Content::Text { text: prompt }],
    };

    if let Some(text) = ask.quote {
        let reply = krabb_core::reply::Reply::quoted(text).unwrap();
        message.content = quote(std::mem::take(&mut message.content), &reply);
    }

    let mut messages = vec![charter("cli")];

    messages.extend(context_at("cli", &root.join("data/system")));
    messages.push(message);

    if let Err(error) = append_installed_memory_context(root, &mut messages, "global").await {
        warn!(
            error = %diagnostic(sentence(error.to_string())),
            "Memory context unavailable for one-shot request, continuing without it."
        );
    }

    Ok(model_at(plugin, model, root, messages).await?.text)
}

async fn append_installed_memory_context(
    root: &Path,
    messages: &mut Vec<Message>,
    scope: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let lock = load_lock_at(root)?;
    let memory = lock
        .plugins
        .iter()
        .find(|(_, entry)| entry.capabilities.iter().any(|capability| capability == "memory"));

    let Some((id, _)) = memory else {
        return Ok(());
    };

    let config = load_config_at(root)?;

    config
        .validate()
        .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { error.into() })?;

    let plugins = Plugins::default();
    load_plugin(root, id, Some(Capability::Memory), false, &config, &plugins).await?;

    let mut call = 1;
    append_memory_context(&plugins, messages, scope, &mut call).await;

    for (_, plugin) in plugins.all().await {
        plugin.stop().await?;
    }

    Ok(())
}

async fn model_at(
    plugin: String,
    model: String,
    root: &Path,
    messages: Vec<Message>,
) -> Result<ModelReply, Box<dyn std::error::Error + Send + Sync>> {
    let path = binary_at(&plugin, root)
        .ok_or_else(|| format!("Plugin binary was not found: {plugin}."))?;

    let config = load_config_at(root)?;

    config
        .validate()
        .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { error.into() })?;

    let mut process = launch(root, path, &plugin, Some(Capability::Model), false, &config).await?;
    let result = async {
        let workspace = workspace_root_at(root);
        let protocol = process.hello.protocol;
        let request = if process.hello.capabilities.contains(&Capability::Context) {
            context_request(2, &model, &messages, &[], Some(&workspace))?
        } else {
            model_request(2, &model, &messages, &[], Some(&workspace), protocol)?
        };

        let response = process.call(request).await?;
        let value = response.result.ok_or_else(|| {
            response
                .error
                .map_or_else(|| "Empty model response.".to_string(), |error| error.message)
        })?;

        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(serde_json::from_value(value)?)
    }
    .await;

    let stopped = process.stop().await;

    match result {
        Err(error) => Err(error),

        Ok(reply) => {
            stopped?;
            Ok(reply)
        }
    }
}

async fn control_at(
    method: &str,
    params: serde_json::Value,
    root: &Path,
) -> Result<Option<serde_json::Value>, Box<dyn std::error::Error + Send + Sync>> {
    match ipc::call(root, method, params).await {
        Ok(value) => Ok(Some(value)),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound
                    | std::io::ErrorKind::ConnectionRefused
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::UnexpectedEof
                    | std::io::ErrorKind::TimedOut
            ) =>
        {
            Ok(None)
        }

        Err(error) => Err(error.into()),
    }
}

/// Saves one configuration value without disturbing the rest of the file, and rejects a
/// result the host could not load.
fn save_setting(root: &Path, key: &str, value: toml_edit::Value) -> Result<Config, PluginError> {
    let path = root.join("config.toml");
    let text = load_config_text(root)?.ok_or("Configuration was not found.")?;
    let parts = key.split('.').collect::<Vec<_>>();

    if parts.is_empty()
        || parts.iter().any(|part| {
            part.is_empty()
                || !part.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || byte == b'_'
                        || byte == b'-'
                })
        })
    {
        return Err("Configuration key is invalid.".into());
    }

    let mut document = text.parse::<toml_edit::DocumentMut>()?;
    let mut table = document.as_table_mut();

    for part in &parts[..parts.len() - 1] {
        if !table.contains_key(part) {
            table[*part] = toml_edit::Item::Table(toml_edit::Table::new());
        }

        table = table[*part]
            .as_table_mut()
            .ok_or_else(|| format!("{part} is not a configuration table."))?;
    }

    table[parts[parts.len() - 1]] = toml_edit::Item::Value(value);

    let rendered = document.to_string();
    let config: Config = toml::from_str(&rendered)?;
    config.validate().map_err(|error| -> PluginError { error.into() })?;
    secure(&path, rendered.as_bytes())?;
    info!(key, "Configuration value saved.");

    Ok(config)
}

async fn config_command(
    command: ConfigCommand,
    root: &Path,
    json: bool,
) -> Result<(), PluginError> {
    let text = load_config_text(root)?.ok_or("Configuration was not found.")?;

    match command {
        ConfigCommand::Get(args) => {
            let config: toml::Value = toml::from_str(&text)?;
            let value = args.key.as_deref().map_or(Some(&config), |key| {
                key.split('.').try_fold(&config, |value, part| value.get(part))
            });

            let Some(value) = value else {
                return Err("Configuration key was not found.".into());
            };

            if json {
                println!("{}", serde_json::to_string_pretty(value)?);
            } else if let Some(key) = args.key {
                println!("{key} = {}", value);
            } else {
                print!("{}", toml::to_string_pretty(&config)?);
            }
        }

        ConfigCommand::Set(args) => {
            let (key, raw_value) = args
                .assignment
                .split_once('=')
                .ok_or("Use KEY=VALUE, for example clients.tui.tools=true.")?;

            let key = match key.trim() {
                "clients.tui" => "clients.tui.tools",
                key => key,
            };

            let raw_value = raw_value.trim();
            let value = raw_value.parse::<toml_edit::Value>()?;
            let config = save_setting(root, key, value)?;

            let selection = selection::reloadable(key);
            let hot = key == "clients.tui.tools" || selection;
            let next_launch = key == "clients.tui.theme";
            let applied = if selection {
                control_at("config.selection.reload", serde_json::json!({}), root).await?.is_some()
            } else if hot {
                control_at(
                    "config.client.set",
                    serde_json::json!({
                        "client": "tui",
                        "tools": config.clients.get("tui").is_some_and(|client| client.tools),
                    }),
                    root,
                )
                .await?
                .is_some()
            } else {
                false
            };

            let restarted = if !hot && !next_launch && args.force {
                restart_daemon(root).await?
            } else {
                false
            };

            let effect = if applied {
                "applied"
            } else if restarted {
                "applied_after_restart"
            } else if hot {
                "apply_on_daemon_start"
            } else if next_launch {
                "apply_on_next_tui_launch"
            } else {
                "restart_required"
            };

            debug!(key, effect, "Configuration activation status determined.");

            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "key": key,
                        "value": raw_value,
                        "saved": true,
                        "applied": applied,
                        "restarted": restarted,
                        "restart_required": !hot && !next_launch && !restarted,
                    }))?
                );
            } else if applied {
                println!("Updated {key}, and the running daemon will use it for new turns.");
            } else if restarted {
                println!("Updated {key}, and the daemon was restarted to apply it.");
            } else if hot {
                println!("Updated {key}, which will apply when the daemon starts.");
            } else if next_launch {
                println!("Updated {key}, which will apply the next time you run `krabb tui`.");
            } else {
                println!("Updated {key}.");
                println!("Restart the daemon to apply this setting.");
            }
        }
    }

    Ok(())
}

fn service_managed() -> Result<bool, PluginError> {
    Ok(service_path().is_file() && service_manager_state()? == ServiceState::Active)
}

/// Restarts the daemon through its service manager, which starts the new process even when
/// the caller is the daemon itself. Returns false when no active service manages it.
fn restart_service() -> Result<bool, PluginError> {
    if !service_managed()? {
        return Ok(false);
    }

    #[cfg(target_os = "linux")]
    let result = std::process::Command::new("systemctl")
        .args(["--user", "restart", "krabb.service"])
        .status()?;

    #[cfg(target_os = "macos")]
    let result = {
        let uid = std::process::Command::new("id").arg("-u").output()?;
        let uid = String::from_utf8_lossy(&uid.stdout).trim().to_owned();
        std::process::Command::new("launchctl")
            .args(["kickstart", "-k", &format!("gui/{uid}/{MACOS_SERVICE_LABEL}")])
            .status()?
    };

    #[cfg(target_os = "windows")]
    {
        service_action(&service_path(), ServiceAction::Restart)?;
    }

    #[cfg(not(target_os = "windows"))]
    if !result.success() {
        return Err("Could not restart the Krabb service.".into());
    }

    Ok(true)
}

async fn restart_daemon(root: &Path) -> Result<bool, PluginError> {
    if restart_service()? {
        return Ok(true);
    }

    if control_at("shutdown", serde_json::json!({}), root).await?.is_none() {
        return Ok(false);
    }

    let mut stopped = false;

    for _ in 0..100 {
        if control_at("status", serde_json::json!({}), root).await?.is_none() {
            stopped = true;
            break;
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    if !stopped {
        return Err("The daemon did not stop, refusing to start a second instance.".into());
    }

    let executable = daemon_executable()?;
    std::process::Command::new(executable)
        .env("KRABB_HOME", root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    for _ in 0..100 {
        if control_at("status", serde_json::json!({}), root).await?.is_some() {
            return Ok(true);
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    Err("Daemon did not become ready after restart.".into())
}

fn sessions_at(root: &Path) -> Result<state::Store, Box<dyn std::error::Error + Send + Sync>> {
    let mut store = state::Store::load(root.join("data/sessions/index.json"))?;
    store.media_root = Some(media_root_at(root));
    Ok(store)
}

#[cfg(test)]
async fn purge_state_at(
    root: &Path,
    sessions: bool,
    deliveries: bool,
) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    let request = serde_json::json!({
        "sessions": sessions,
        "deliveries": deliveries,
        "yes": true,
    });

    if let Some(result) = control_at("state.purge", request, root).await? {
        return Ok(result);
    }

    let _lock = offline_lock(root)?;
    purge_offline_state(root, sessions, deliveries)
}

fn purge_offline_state(
    root: &Path,
    sessions: bool,
    deliveries: bool,
) -> Result<Value, PluginError> {
    let (session_ids, delivery_count) = sessions_at(root)?.purge(sessions, deliveries)?;
    let mut pending_worktrees = Vec::new();

    for id in &session_ids {
        if let Err(error) = state::remove_worktree(root, id) {
            pending_worktrees.push(serde_json::json!({
                "id": id,
                "error": sentence(error.to_string()),
            }));
        }
    }

    Ok(serde_json::json!({
        "sessions": session_ids.len(),
        "deliveries": delivery_count,
        "pending_worktrees": pending_worktrees,
    }))
}

fn turn_messages(session: &str, home_root: &Path, history: Vec<Message>) -> Vec<Message> {
    let mut messages = vec![charter(session), runtime_context(session)];

    messages.extend(context_at(session, &home_root.join("data/system")));
    messages.extend(history);
    messages
}

/// Krabb's built-in charter. It is compiled in, so personalizing KRAB.md or KLAW.md cannot
/// remove it; host policy still enforces every permission in code.
fn charter(session: &str) -> Message {
    Message {
        id: format!("{}charter", state::SYSTEM_CONTEXT),
        session: session.into(),
        role: Role::System,
        sender: None,
        content: vec![Content::Text { text: templates::CHARTER.into() }],
    }
}

/// Describes the running host from its own constants, so the model can explain Krabb accurately.
fn runtime_context(session: &str) -> Message {
    let commands = krabb_core::slash::SPECS
        .iter()
        .map(|spec| format!("/{}", spec.name))
        .collect::<Vec<_>>()
        .join(", ");

    let text = format!(
        "How Krabb works, for answering questions about yourself. Krabb {VERSION} is a \
         self-hosted agent: a host process connects chat channels and clients to model and tool \
         plugins.\n\
         - Each conversation stores up to {} messages. Tool calls and results count as messages, \
         and the oldest are dropped beyond that.\n\
         - When the model's context fills up, older turns are replaced by a summary in what you \
         see, while the stored history stays intact. Users can also send /compact. Models with \
         no known context window see a /compact summary or the opening request, plus the \
         latest {} messages.\n\
         - Users personalize your name, personality, tone, and writing style with `krabb \
         hatch` or by sending /hatch with the change; your model rewrites KRAB.md and \
         KLAW.md from their words.\n\
         - Users steer conversations with slash commands: {commands}. /help lists the ones \
         available in the current chat.\n\
         When this is not enough and you have a web tool, read Krabb's documentation and \
         source at {} (user guides are in krabb-docs/). They describe the latest release, \
         which may be newer than {VERSION}. Otherwise say you are unsure instead of guessing.",
        krabb_core::session::MESSAGE_HISTORY_LIMIT,
        compaction::UNBUDGETED,
        env!("CARGO_PKG_REPOSITORY"),
    );

    Message {
        id: format!("{}runtime", state::SYSTEM_CONTEXT),
        session: session.into(),
        role: Role::System,
        sender: None,
        content: vec![Content::Text { text }],
    }
}

fn context_at(session: &str, root: &Path) -> Vec<Message> {
    let instructions = [
        ("KRAB.md", "Krabb identity, values, and communication style."),
        ("KLAW.md", "Krabb behavioral instructions and workflows."),
    ];

    let header_bytes = instructions
        .iter()
        .map(|(name, description)| name.len() + description.len() + 32)
        .sum::<usize>();

    let file_limit = CONTEXT_LIMIT.saturating_sub(header_bytes) / instructions.len();
    let mut messages = Vec::with_capacity(instructions.len());

    for (name, description) in instructions {
        let Some(text) = context_file(&root.join(name), file_limit) else {
            continue;
        };

        messages.push(Message {
            id: format!("{}{name}", state::SYSTEM_CONTEXT),
            session: session.into(),
            role: Role::System,
            sender: None,
            content: vec![Content::Text { text: format!("{description}\n\n{text}") }],
        });
    }

    messages
}

fn context_file(path: &Path, limit: usize) -> Option<String> {
    if limit == 0 {
        return None;
    }

    let metadata = std::fs::symlink_metadata(path).ok()?;

    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return None;
    }

    let mut bytes = Vec::new();

    std::fs::File::open(path)
        .ok()?
        .take((limit as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .ok()?;

    if bytes.len() > limit {
        bytes.truncate(limit);
    }

    let mut value = String::from_utf8_lossy(&bytes).into_owned();

    while value.len() > limit {
        value.pop();
    }

    (!value.trim().is_empty()).then_some(value)
}

fn allowed(policy: &ChannelConfig, event: &serde_json::Value, chat: &str) -> bool {
    let private = event["private"] == true;

    if !private && policy.allow.is_empty() {
        return false;
    }

    if !private && !policy.allow.is_empty() && !policy.allow.iter().any(|value| value == chat) {
        return false;
    }

    // Native commands and bot-addressed commands are addressed as explicitly as a mention.
    if let Some(marker) = policy.mention.as_deref()
        && event["addressed"] != true
        && !event["text"].as_str().is_some_and(|text| mentions(text, marker))
    {
        return false;
    }

    if !policy.topic.is_empty() && !policy.topic.iter().any(|value| same_id(&event["topic"], value))
    {
        return false;
    }

    if !policy.thread.is_empty()
        && !policy.thread.iter().any(|value| same_id(&event["thread"], value))
    {
        return false;
    }

    let sender = event_id(&event["sender"]).unwrap_or_default();

    let roles =
        event["roles"].as_array().into_iter().flatten().filter_map(event_id).collect::<Vec<_>>();

    let restricted =
        policy.owner.is_some() || !policy.admin.is_empty() || !policy.member.is_empty();

    !restricted
        || policy.owner.as_deref() == Some(sender.as_str())
        || policy
            .admin
            .iter()
            .any(|value| value == &sender || roles.iter().any(|role| role == value))
        || policy
            .member
            .iter()
            .any(|value| value == &sender || roles.iter().any(|role| role == value))
}

fn tools_allowed(
    policy: &ChannelConfig,
    event: &serde_json::Value,
    chat: &str,
    approvals: bool,
) -> bool {
    if !approvals || !policy.tools || !allowed(policy, event, chat) {
        return false;
    }

    if event["private"] != true {
        return true;
    }

    let sender = event_id(&event["sender"]).unwrap_or_default();

    let roles =
        event["roles"].as_array().into_iter().flatten().filter_map(event_id).collect::<Vec<_>>();

    policy.allow.iter().any(|value| value == chat)
        || policy.owner.as_deref() == Some(sender.as_str())
        || policy
            .admin
            .iter()
            .any(|value| value == &sender || roles.iter().any(|role| role == value))
        || policy
            .member
            .iter()
            .any(|value| value == &sender || roles.iter().any(|role| role == value))
}

fn schedule_allowed(policy: &ChannelConfig, event: &serde_json::Value, chat: &str) -> bool {
    if !policy.tools || !allowed(policy, event, chat) {
        return false;
    }

    if event["private"] != true {
        return true;
    }

    let sender = event_id(&event["sender"]).unwrap_or_default();

    let roles =
        event["roles"].as_array().into_iter().flatten().filter_map(event_id).collect::<Vec<_>>();

    policy.allow.iter().any(|value| value == chat)
        || policy.owner.as_deref() == Some(sender.as_str())
        || policy
            .admin
            .iter()
            .any(|value| value == &sender || roles.iter().any(|role| role == value))
        || policy
            .member
            .iter()
            .any(|value| value == &sender || roles.iter().any(|role| role == value))
}

fn channel_shell_allowed(
    policy: &ChannelConfig,
    event: &serde_json::Value,
    chat: &str,
    approvals: bool,
    shell_enabled: bool,
) -> bool {
    shell_enabled || (policy.shell && tools_allowed(policy, event, chat, approvals))
}

fn mentions(text: &str, marker: &str) -> bool {
    let marker = marker.trim();

    if marker.is_empty() {
        return true;
    }

    text.split(|value: char| !value.is_alphanumeric() && value != '_' && value != '-')
        .any(|value| value.eq_ignore_ascii_case(marker.trim_start_matches('@')))
}

fn same_id(value: &serde_json::Value, expected: &str) -> bool {
    event_id(value).is_some_and(|value| value == expected)
}

fn isolate(session: &str) -> Result<Option<PathBuf>, Box<dyn std::error::Error + Send + Sync>> {
    isolate_at(session, &home())
}

fn worktree_command(root: &Path) -> std::process::Command {
    let mut command = git_command();

    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };

    command
        .env_remove("GIT_CONFIG")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env_remove("GIT_CONFIG_COUNT")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", null)
        .env("GIT_NO_LAZY_FETCH", "1")
        .args(["-c", &format!("core.hooksPath={null}")])
        .args(["-c", "core.fsmonitor=false", "-c", "submodule.recurse=false"])
        .args(["-c", "protocol.allow=never", "-c", "gc.auto=0", "-c", "maintenance.auto=false"])
        .arg("-C")
        .arg(worktree::git_path(root));

    command
}

fn check_checkout(root: &Path) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = command_output(worktree_command(root).args([
        "config",
        "--get-regexp",
        r"^(filter\..*\.(clean|smudge|process)|remote\..*\.promisor|extensions\.partialclone)$",
    ]))?;

    match config.status.code() {
        Some(1) => Ok(()),

        Some(0) => {
            Err("Automatic worktrees cannot use checkout filters or partial clones. Prepare the worktree through an explicitly approved shell operation.".into())
        }

        _ => Err("The workspace Git configuration could not be checked.".into()),
    }
}

fn isolate_at(
    session: &str,
    home: &Path,
) -> Result<Option<PathBuf>, Box<dyn std::error::Error + Send + Sync>> {
    if !valid(session) {
        return Ok(None);
    }

    let home = std::fs::canonicalize(home)?;

    let root = match std::fs::canonicalize(home.join("data/workspaces/repository")) {
        Ok(root) => root,

        Err(_) => {
            return Err("Worktrees are not initialized. Run krabb worktree setup first.".into());
        }
    };

    let check =
        command_output(worktree_command(&root).args(["rev-parse", "--is-inside-work-tree"]));

    if !check.is_ok_and(|output| output.status.success()) {
        return Err("The configured workspace is not a Git worktree.".into());
    }

    let path = krabb_file::directory(&home, Path::new("worktrees"))?.join(session);

    if std::fs::symlink_metadata(&path).is_ok() {
        if std::fs::symlink_metadata(&path)?.file_type().is_symlink() {
            return Err("The isolated worktree path cannot be a symbolic link.".into());
        }

        let path = std::fs::canonicalize(path)?;

        if !path.starts_with(home.join("worktrees")) {
            return Err("The isolated room worktree leaves the configured root.".into());
        }

        let check =
            command_output(worktree_command(&path).args(["rev-parse", "--is-inside-work-tree"]))?;

        if !check.status.success() {
            return Err("The isolated room worktree is not a Git worktree.".into());
        }

        return Ok(Some(path));
    }

    check_checkout(&root)?;
    worktree::baseline(&root)?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;

        let parent = std::fs::canonicalize(parent)?;

        if !parent.starts_with(home.join("worktrees")) {
            return Err("The isolated worktree path leaves the configured root.".into());
        }
    }

    let output = command_output(
        worktree_command(&root)
            .args(["worktree", "add", "--detach", "--no-checkout"])
            .arg(worktree::git_path(&path))
            .arg("HEAD"),
    )?;

    if !output.status.success() {
        return Err("The isolated room worktree could not be created.".into());
    }

    let checkout = check_checkout(&path).and_then(|()| {
        let output = command_output(worktree_command(&path).args(["reset", "--hard", "HEAD"]))?;

        if output.status.success() {
            Ok(())
        } else {
            Err("The isolated room worktree could not be checked out.".into())
        }
    });

    if let Err(error) = checkout {
        let removed = command_output(
            worktree_command(&root)
                .args(["worktree", "remove", "--force"])
                .arg(worktree::git_path(&path)),
        )?;

        if !removed.status.success() {
            return Err("The incomplete room worktree could not be removed.".into());
        }

        return Err(error);
    }

    let path = std::fs::canonicalize(path)?;

    if !path.starts_with(home.join("worktrees")) {
        return Err("The isolated worktree leaves the configured root.".into());
    }

    Ok(Some(path))
}

fn workspace_failure(
    store: &mut state::Store,
    channel: &str,
    event: &str,
    chat: &str,
    thread: Option<String>,
) -> std::io::Result<()> {
    store.queue_delivery(
        format!("workspace-{channel}-{event}"),
        channel,
        chat,
        false,
        thread,
        format!(
            "I could not create an isolated workspace for this conversation. Ask the operator \
             to check the daemon log and run krabb worktree setup {channel}, \
             or explicitly set channels.{channel}.worktree=false to use the shared workspace."
        ),
    )
}

fn version(json: bool) {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({"name": NAME, "version": VERSION}))
                .expect("serializing version JSON cannot fail")
        );
    } else {
        println!("{NAME} {VERSION}");
    }
}

/// Renews a progressing generation in bounded steps up to its admitted ceiling.
#[derive(Clone, Copy, Debug)]
struct Renewing {
    base: u64,
    total: u64,
    ceiling: u64,
    granted: u32,
    seen: usize,
    check: u64,
}

impl Renewing {
    const LIMIT: u32 = 3;

    fn new(total: u64, ceiling: u64) -> Self {
        Self {
            base: total,
            total,
            ceiling,
            granted: 0,
            seen: 0,
            check: total.saturating_mul(4) / 5,
        }
    }

    fn due(&self) -> Duration {
        Duration::from_millis(self.check)
    }

    fn exhausted(&self) -> bool {
        self.granted >= Self::LIMIT || self.total >= self.ceiling
    }

    /// Schedules another check before the deadline; false when none remains.
    fn defer(&mut self) -> bool {
        self.check = self.check.saturating_add((self.total / 20).max(1));
        self.check < self.total
    }

    /// Returns the next extension, or zero without progress since the last check.
    fn next(&self, progress: usize) -> u64 {
        if progress <= self.seen || self.granted >= Self::LIMIT {
            return 0;
        }

        (self.base / 2).max(1).min(self.ceiling.saturating_sub(self.total))
    }

    fn grant(&mut self, extra: u64, progress: usize) {
        self.total = self.total.saturating_add(extra);
        self.granted += 1;
        self.seen = progress;
        self.check = self.total.saturating_mul(4) / 5;
    }
}

#[cfg(test)]
mod renewal {
    use super::Renewing;

    #[test]
    fn a_late_first_note_keeps_renewal_until_progress_or_the_deadline() {
        let mut state = Renewing::new(1_000, 2_000);

        assert_eq!(state.due().as_millis(), 800);
        assert_eq!(state.next(0), 0);
        assert!(!state.exhausted());
        assert!(state.defer());
        assert_eq!(state.due().as_millis(), 850);

        let extra = state.next(1);

        assert_eq!(extra, 500);

        state.grant(extra, 1);

        assert_eq!((state.total, state.due().as_millis()), (1_500, 1_200));
        assert_eq!(state.next(1), 0);

        while state.defer() {}

        assert!(state.due().as_millis() >= 1_500);

        state.grant(state.next(2), 2);

        assert_eq!(state.total, 2_000);
        assert!(state.exhausted());
        assert_eq!(state.next(3), 0);
    }
}

fn health(
    root: &Path,
    path: PathBuf,
    expected: &str,
    config: &Config,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let root = root.to_path_buf();
    let expected = expected.to_owned();
    let config = config.clone();

    std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new()
            .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { error.into() })?;

        runtime.block_on(async move {
            let process = launch(&root, path, &expected, None, true, &config).await?;
            process.stop().await?;
            Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
        })
    })
    .join()
    .map_err(|_| "Plugin health check panicked.")??;

    Ok(())
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |value| value.as_nanos() as u64)
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |value| value.as_secs())
}

fn redact(value: &[u8]) -> String {
    krabb_log::redact_bytes(value)
}

fn link_binary(source: &Path, destination: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(source, destination)
    }

    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(source, destination)
            .or_else(|_| std::fs::copy(source, destination).map(|_| ()))
    }
}

#[cfg(test)]
fn remove(name: PluginRemove) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    remove_at(name, &home())
}

#[cfg(test)]
fn remove_at(
    name: PluginRemove,
    home_root: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let _offline = offline_lock(home_root)?;

    let id = name.id.clone();

    remove_guarded(name, home_root, |lock| ensure_no_orphaned_plugin_state(lock, &id, home_root))
        .map(drop)
}

fn remove_guarded(
    name: PluginRemove,
    home_root: &Path,
    guard: impl FnOnce(&Lock) -> Result<(), Box<dyn std::error::Error + Send + Sync>>,
) -> Result<revoke::Revoked, Box<dyn std::error::Error + Send + Sync>> {
    let _lock = lock_at(home_root, ".plugins.lock")?;

    if !valid(&name.id) {
        return Err(format!("Invalid plugin ID: {}.", name.id).into());
    }

    if !name.yes {
        return Err("Uninstalling a plugin requires --yes (-y).".into());
    }

    let mut lock = load_lock_at(home_root)?;
    let previous_lock = lock.clone();
    let dest = home_root.join("plugins").join(&name.id);
    let plugins = home_root.join("plugins");
    let backup = plugins.join(format!(".{}.backup-remove-{}", name.id, now()));
    let had_destination = dest.exists();

    if !lock.plugins.contains_key(&name.id) && !had_destination {
        return Err(format!("Plugin {} is not installed.", name.id).into());
    }

    guard(&lock)?;

    let revoked = revoke::revoke(home_root, &name.id, &lock)?;

    if had_destination {
        std::fs::rename(&dest, &backup)?;
    }

    lock.plugins.remove(&name.id);

    if let Err(error) = save_lock_at(home_root, &lock) {
        let restored_lock = save_lock_at(home_root, &previous_lock);

        let restored_plugin =
            if had_destination { std::fs::rename(&backup, &dest) } else { Ok(()) };

        if let Err(restore) = restored_lock {
            return Err(format!(
                "Could not save the plugin lock: {}. Rollback failed: {}.",
                sentence(error.to_string()),
                sentence(restore.to_string())
            )
            .into());
        }

        if let Err(restore) = restored_plugin {
            return Err(format!(
                "Could not save the plugin lock: {}. Rollback failed: {}.",
                sentence(error.to_string()),
                sentence(restore.to_string())
            )
            .into());
        }

        return Err(error);
    }

    if had_destination {
        let _ = std::fs::remove_dir_all(backup);
    }

    Ok(revoked)
}

fn ensure_no_orphaned_plugin_state(
    lock: &Lock,
    id: &str,
    home_root: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (model, channel) = last_plugin_state(lock, id, home_root);

    if !model && !channel {
        return Ok(());
    }

    let store = sessions_at(home_root)?;

    check_plugin_state(
        lock,
        id,
        home_root,
        &serde_json::json!({
            "sessions": !store.sessions.is_empty(),
            "deliveries": !store.outbox.is_empty(),
        }),
    )
}

fn last_plugin_state(lock: &Lock, id: &str, home_root: &Path) -> (bool, bool) {
    let Some(entry) = lock.plugins.get(id) else {
        return (false, false);
    };

    let removes_last_model = entry.capabilities.iter().any(|capability| capability == "model")
        && !lock.plugins.iter().any(|(other_id, entry)| {
            other_id != id && model_plugin_available(other_id, entry, home_root)
        });

    let removes_last_channel = entry.capabilities.iter().any(|capability| capability == "channel")
        && !lock.plugins.iter().any(|(other_id, entry)| {
            other_id != id
                && entry.capabilities.iter().any(|capability| capability == "channel")
                && binary_at(other_id, home_root).is_some()
        });

    (removes_last_model, removes_last_channel)
}

fn load_metadata(path: &Path, limit: u64) -> std::io::Result<Option<Vec<u8>>> {
    // Callers hold .metadata.lock across recovery and access.
    if std::fs::symlink_metadata(path).is_ok() {
        krabb_file::private(path)?;
    }

    krabb_file::load(path, limit)
}

fn load_config_text(root: &Path) -> Result<Option<String>, PluginError> {
    if !root.try_exists()? {
        return Ok(None);
    }

    let _lock = metadata_lock(root)?;

    load_metadata(&root.join("config.toml"), LOCK_LIMIT)?
        .map(String::from_utf8)
        .transpose()
        .map_err(Into::into)
}

fn load_config_at(root: &Path) -> Result<Config, PluginError> {
    match load_config_text(root)? {
        Some(text) => Ok(toml::from_str(&text)?),
        None => Ok(Config::default()),
    }
}

fn load_lock_at(root: &Path) -> Result<Lock, Box<dyn std::error::Error + Send + Sync>> {
    if !root.try_exists()? {
        return Ok(Lock::default());
    }

    let _lock = metadata_lock(root)?;

    let Some(bytes) = load_metadata(&root.join("plugins.lock"), LOCK_LIMIT)? else {
        return Ok(Lock::default());
    };

    Ok(serde_json::from_slice(&bytes)?)
}

fn save_lock_at(root: &Path, lock: &Lock) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    std::fs::create_dir_all(root)?;

    let bytes = serde_json::to_vec_pretty(lock)?;

    if bytes.len() as u64 > LOCK_LIMIT {
        return Err("Plugin lock exceeds the size limit.".into());
    }

    secure(root.join("plugins.lock"), bytes)?;

    Ok(())
}

fn secure(path: impl AsRef<Path>, content: impl AsRef<[u8]>) -> std::io::Result<()> {
    let path = path.as_ref();

    let _lock = if matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some("config.toml" | "plugins.lock")
    ) {
        let lock = metadata_lock(path.parent().unwrap_or(Path::new(".")))?;
        krabb_file::recover(path)?;
        Some(lock)
    } else {
        None
    };

    save_file(path, content)
}

fn list(page: usize, json: bool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    list_at(&home(), page, json)
}

fn list_at(
    root: &Path,
    page: usize,
    json: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    validate_page(page)?;

    let plugin_root = root.join("plugins");
    let mut rows = Vec::new();

    if plugin_root.exists() {
        for entry in std::fs::read_dir(&plugin_root)? {
            let entry = entry?;
            let path = entry.path();

            if !path.is_dir() || entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }

            let id = entry.file_name().to_string_lossy().into_owned();

            let manifest = read_manifest_checked(&path)
                .map_err(|error| format!("Plugin {id} is invalid: {error}."))?;

            let health =
                if path.join("bin").join(plugin_name(&id)).is_file() { "ready" } else { "missing" };

            rows.push(plugin_summary(&manifest, health));
        }
    }

    rows.sort_by(|left: &Value, right: &Value| left["id"].as_str().cmp(&right["id"].as_str()));

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "items": rows,
            }))?
        );
    } else if rows.is_empty() {
        println!("No plugins installed.");
    } else {
        println!("{}", format_plugin_list(&rows, page)?);
    }

    Ok(())
}

fn string_list(value: &Value) -> String {
    let values =
        value.as_array().into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>();

    if values.is_empty() { "none".into() } else { values.join(", ") }
}

fn read_manifest(root: &Path) -> Option<Manifest> {
    read_manifest_checked(root).ok()
}

fn read_manifest_checked(
    root: &Path,
) -> Result<Manifest, Box<dyn std::error::Error + Send + Sync>> {
    let path = root.join("krabb-plugin.toml");

    let bytes = read_bounded(&path, MANIFEST_LIMIT).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            format!("manifest not found at {}", path.display())
        } else {
            format!("could not read manifest at {}: {error}", path.display())
        }
    })?;

    let manifest: Manifest = toml::from_slice(&bytes)
        .map_err(|error| format!("manifest parse failed at {}: {error}", path.display()))?;

    manifest
        .validate()
        .map_err(|error| format!("manifest validation failed at {}: {error}", path.display()))?;

    Ok(manifest)
}

fn read_bounded(path: &Path, limit: u64) -> std::io::Result<Vec<u8>> {
    let metadata = std::fs::symlink_metadata(path)?;

    if metadata.file_type().is_symlink() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "File cannot be a symbolic link.",
        ));
    }

    if !metadata.is_file() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "File is not regular."));
    }

    if metadata.len() > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::FileTooLarge,
            "File exceeds the size limit.",
        ));
    }

    let mut bytes = Vec::new();

    std::fs::File::open(path)?.take(limit.saturating_add(1)).read_to_end(&mut bytes)?;

    if bytes.len() as u64 > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::FileTooLarge,
            "File exceeds the size limit.",
        ));
    }

    Ok(bytes)
}

#[cfg(test)]
mod tests;
