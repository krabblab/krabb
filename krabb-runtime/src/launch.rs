//! Recovers, launches, and readies plugin processes with their environment.

use crate::*;

pub(crate) fn recover_plugins(root: &Path) -> std::io::Result<()> {
    let _plugins_lock = lock_at(root, ".plugins.lock")?;

    updates::Image::clean(root)?;

    let lock = load_lock_at(root).map_err(std::io::Error::other)?;
    let plugins = root.join("plugins");

    let Ok(entries) = std::fs::read_dir(&plugins) else {
        return Ok(());
    };

    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();

        if let Some(id) =
            name.strip_prefix('.').and_then(|value| value.split_once(".backup-")).map(|(id, _)| id)
            && valid(id)
        {
            let backup = entry.path();
            let destination = plugins.join(id);

            let Some(committed) = lock.plugins.get(id) else {
                std::fs::remove_dir_all(backup)?;
                continue;
            };

            let matches = |directory: &Path| {
                let Ok(manifest) = read_manifest_checked(directory) else {
                    return false;
                };

                if manifest.id != id
                    || manifest.version != committed.version
                    || manifest.protocol != committed.protocol
                    || manifest.capabilities != committed.capabilities
                    || manifest.permissions != committed.permissions
                    || manifest.variables != committed.variables
                    || manifest.settings != committed.settings
                    || manifest.commands != committed.commands
                {
                    return false;
                }

                verify_plugin_binary(
                    id,
                    committed,
                    &directory.join("krabb-plugin.toml"),
                    &directory.join("bin").join(plugin_name(id)),
                )
                .is_ok()
            };

            if matches(&destination) {
                std::fs::remove_dir_all(backup)?;
            } else if matches(&backup) {
                if destination.exists() {
                    std::fs::remove_dir_all(&destination)?;
                }

                std::fs::rename(backup, destination)?;
            } else {
                return Err(std::io::Error::other(format!(
                    "Neither plugin {id} nor its backup matches the committed lock."
                )));
            }
        } else if name.starts_with('.') && name.contains(".stage-") {
            std::fs::remove_dir_all(entry.path())?;
        }
    }

    Ok(())
}

pub(crate) fn reclaim_worktrees(root: &Path, store: &state::Store) {
    let directory = root.join("worktrees");

    let Ok(entries) = std::fs::read_dir(&directory) else {
        return;
    };

    let active = store.sessions.keys().cloned().collect::<std::collections::BTreeSet<_>>();

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();

        if valid(&name) && !active.contains(&name) {
            let _ = state::remove_worktree(root, &name);
        }
    }
}

pub(crate) async fn launch(
    root: &Path,
    path: PathBuf,
    expected: &str,
    capability: Option<Capability>,
    force: bool,
    config: &Config,
) -> Result<Process, Box<dyn std::error::Error + Send + Sync>> {
    start(root, path, expected, capability, force, config, None).await
}

/// Prepares an unlocked replacement whose running image must match an already confirmed hash.
pub(crate) fn verify_checked(
    root: &Path,
    path: PathBuf,
    expected: &str,
    config: &Config,
    hash: &str,
) -> Result<Verified, PluginError> {
    verify(root, path, expected, None, true, config, Some(hash))
}

async fn start(
    root: &Path,
    path: PathBuf,
    expected: &str,
    capability: Option<Capability>,
    force: bool,
    config: &Config,
    hash: Option<&str>,
) -> Result<Process, PluginError> {
    verify(root, path, expected, capability, force, config, hash)?.start().await
}

/// A plugin's private image, verified against its manifest and hash and ready to start.
pub(crate) struct Verified {
    expected: String,
    capability: Option<Capability>,
    manifest: Manifest,
    capabilities: Vec<String>,
    commands: Vec<krabb_core::types::CommandSpec>,
    image: updates::Image,
    env: Vec<(String, String)>,
}

fn verify(
    root: &Path,
    path: PathBuf,
    expected: &str,
    capability: Option<Capability>,
    force: bool,
    config: &Config,
    hash: Option<&str>,
) -> Result<Verified, PluginError> {
    let manifest_path = path
        .parent()
        .and_then(Path::parent)
        .ok_or("Plugin manifest path is invalid.")?
        .join("krabb-plugin.toml");

    let manifest = read_manifest_checked(manifest_path.parent().unwrap_or(Path::new(".")))?;

    let mut expected_capabilities = manifest.capabilities.clone();

    expected_capabilities.sort();

    let mut expected_commands = manifest.commands.clone();

    expected_commands.sort_by(|left, right| left.name.cmp(&right.name));

    if !force {
        let lock = load_lock_at(root)?;

        let entry = lock
            .plugins
            .get(expected)
            .ok_or_else(|| format!("Plugin {expected} is not locked."))?;

        verify_plugin_binary(expected, entry, &manifest_path, &path)?;

        let mut locked_capabilities = entry.capabilities.clone();

        locked_capabilities.sort();

        if expected_capabilities != locked_capabilities {
            return Err(
                format!("Plugin {expected} lock capabilities do not match its manifest.").into()
            );
        }

        let mut locked_commands = entry.commands.clone();

        locked_commands.sort_by(|left, right| left.name.cmp(&right.name));

        if expected_commands != locked_commands {
            return Err(
                format!("Plugin {expected} lock commands do not match its manifest.").into()
            );
        }
    }

    let expected_hash = match hash {
        Some(hash) => hash.to_owned(),
        None => digest(&manifest_path, &path)?,
    };

    let image = updates::Image::new(root, &path)?;

    if digest(&manifest_path, &image.binary)? != expected_hash {
        return Err("Plugin executable changed while preparing its running image.".into());
    }

    let env = env_for(&manifest, config, root)?;

    Ok(Verified {
        expected: expected.to_owned(),
        capability,
        manifest,
        capabilities: expected_capabilities,
        commands: expected_commands,
        image,
        env,
    })
}

impl Verified {
    /// Starts the verified image and checks that it advertises what its manifest declares.
    pub(crate) async fn start(self) -> Result<Process, PluginError> {
        let Self { expected, capability, manifest, capabilities, commands, image, env } = self;

        let mut process =
            Process::start_with_env(&image.binary, std::iter::empty::<&std::ffi::OsStr>(), env)
                .await?;

        process.image = Some(image);

        if let Some(stderr) = process.take_stderr() {
            capture_plugin_stderr(expected.clone(), stderr);
        }

        let mut actual_capabilities = process
            .hello
            .capabilities
            .iter()
            .map(capability_name)
            .map(str::to_owned)
            .collect::<Vec<_>>();

        actual_capabilities.sort();

        let mut actual_commands = process.hello.commands.clone();

        actual_commands.sort_by(|left, right| left.name.cmp(&right.name));

        if process.hello.id != expected
            || process.hello.version != manifest.version
            || capability
                .is_some_and(|capability| !process.hello.capabilities.contains(&capability))
            || capabilities != actual_capabilities
            || commands != actual_commands
        {
            let actual = process.hello.id.clone();
            let _ = process.stop().await;
            return Err(format!("Plugin {actual} did not advertise {expected} correctly.").into());
        }

        Ok(process)
    }
}

pub(crate) fn capture_plugin_stderr(
    plugin: String,
    mut stderr: tokio::sync::mpsc::Receiver<Vec<u8>>,
) {
    const DIAGNOSTIC_LIMIT: usize = 8 * 1024;

    tokio::spawn(async move {
        let mut line = Vec::new();

        while let Some(chunk) = stderr.recv().await {
            for byte in &chunk {
                if line.len() < DIAGNOSTIC_LIMIT {
                    line.push(*byte);
                }

                if *byte == b'\n' || line.len() == DIAGNOSTIC_LIMIT {
                    let diagnostic = String::from_utf8_lossy(&line);
                    let diagnostic = redact_diagnostic(diagnostic.trim());
                    tracing::debug!(plugin = %plugin, diagnostic, "Plugin stderr.");
                    line.clear();
                }
            }
        }

        if !line.is_empty() {
            let diagnostic = String::from_utf8_lossy(&line);
            let diagnostic = redact_diagnostic(diagnostic.trim());
            tracing::debug!(plugin = %plugin, diagnostic, "Plugin stderr.");
        }
    });
}

pub(crate) async fn load_plugin(
    root: &Path,
    id: &str,
    capability: Option<Capability>,
    force: bool,
    config: &Config,
    plugins: &Plugins,
) -> Result<Hello, Box<dyn std::error::Error + Send + Sync>> {
    if !valid(id) {
        return Err(format!("Invalid plugin ID: {id}.").into());
    }

    let _loading = plugins.loading.lock().await;

    let path = binary_at(id, root).ok_or_else(|| format!("Plugin binary was not found: {id}."))?;
    let process = launch(root, path, id, capability, force, config).await?;
    let hello = process.hello.clone();

    if let Some(previous) = plugins.insert(Live::new(process)).await {
        tokio::spawn(async move {
            if let Err(error) = previous.stop().await {
                warn!(error = %diagnostic(sentence(error.to_string())), "Replaced plugin could not be stopped.");
            }
        });
    }

    info!(plugin = %hello.id, version = %hello.version, "Plugin loaded.");
    plugins.detached.write().await.remove(id);
    Ok(hello)
}

pub(crate) async fn unload_plugin(
    id: &str,
    plugins: &Plugins,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    if !valid(id) {
        return Err(format!("Invalid plugin ID: {id}.").into());
    }

    let _loading = plugins.loading.lock().await;
    let Some(plugin) = plugins.remove(id).await else {
        return Ok(false);
    };

    plugin.stop().await?;
    info!(plugin = id, "Plugin unloaded.");
    Ok(true)
}

pub(crate) fn capability_name(capability: &Capability) -> &'static str {
    match capability {
        Capability::Model => "model",
        Capability::Context => "context",
        Capability::Vision => "vision",
        Capability::Channel => "channel",
        Capability::Store => "store",
        Capability::Memory => "memory",
        Capability::Scheduler => "scheduler",
        Capability::Tool => "tool",
        Capability::Mcp => "mcp",
        Capability::Speech => "speech",
        Capability::Client => "client",
        Capability::Resource => "resource",
        Capability::Agent => "agent",
        Capability::Runner => "runner",
    }
}

pub(crate) fn native_command(name: &str) -> bool {
    matches!(
        name,
        "help"
            | "init"
            | "hatch"
            | "config"
            | "worktree"
            | "workspace"
            | "doctor"
            | "status"
            | "version"
            | "completion"
            | "plugin"
            | "session"
            | "delivery"
            | "schedule"
            | "memory"
            | "tui"
            | "data"
            | "service"
            | "ask"
            | "export"
            | "validate"
            | "import"
    )
}

pub(crate) fn krabb_owned_plugin_command(name: &str, owner: &str) -> bool {
    matches!((name, owner), ("memory", "memory") | ("tui", "tui") | ("data", "sqlite"))
}

pub(crate) fn validate_commands(
    root: &Path,
    id: &str,
    commands: &[CommandSpec],
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    for command in commands {
        if native_command(&command.name) && !krabb_owned_plugin_command(&command.name, id) {
            return Err(
                format!("Plugin {id} command {} is reserved by Krabb.", command.name).into()
            );
        }

        for (other_id, entry) in &load_lock_at(root)?.plugins {
            if other_id != id && entry.commands.iter().any(|other| other.name == command.name) {
                return Err(format!(
                    "Plugin {id} command {} conflicts with plugin {other_id}.",
                    command.name
                )
                .into());
            }
        }
    }

    Ok(())
}

pub(crate) fn available_commands(
    root: &Path,
) -> Result<BTreeMap<String, Vec<(String, CommandSpec)>>, PluginError> {
    let lock = load_lock_at(root)?;

    let mut commands = BTreeMap::new();

    for (id, entry) in &lock.plugins {
        if (!entry.variables.is_empty() || !entry.settings.is_empty())
            && !native_command(id)
            && !entry.commands.iter().any(|command| command.name == *id)
            && binary_at(id, root).is_some()
        {
            commands.entry(id.clone()).or_insert_with(Vec::new).push((
                format!("{RUNTIME_COMMAND_OWNER}setup:{id}"),
                CommandSpec {
                    name: id.clone(),
                    description: format!("Set up {id} credentials."),
                    interactive: true,
                },
            ));
        }

        for command in &entry.commands {
            let krabb_owned = krabb_owned_plugin_command(&command.name, id);

            if krabb_owned && binary_at(id, root).is_none() {
                continue;
            }

            let owner = if krabb_owned {
                format!("{RUNTIME_COMMAND_OWNER}{}", command.name)
            } else {
                id.clone()
            };

            commands
                .entry(command.name.clone())
                .or_insert_with(Vec::new)
                .push((owner, command.clone()));
        }
    }

    let has_channel = lock.plugins.iter().any(|(id, entry)| {
        entry.capabilities.iter().any(|capability| capability == "channel")
            && binary_at(id, root).is_some()
    });

    let has_scheduler = lock.plugins.iter().any(|(id, entry)| {
        entry.capabilities.iter().any(|capability| capability == "scheduler")
            && binary_at(id, root).is_some()
    });

    for (available, name, description) in [
        (
            lock.plugins.iter().any(|(id, entry)| model_plugin_available(id, entry, root)),
            "session",
            "Manage durable agent sessions.",
        ),
        (has_channel, "delivery", "Manage pending channel deliveries."),
        (has_scheduler, "schedule", "Manage scheduled agent tasks."),
    ] {
        if available {
            commands.entry(name.into()).or_default().push((
                format!("{RUNTIME_COMMAND_OWNER}{name}"),
                CommandSpec {
                    name: name.into(),
                    description: description.into(),
                    interactive: false,
                },
            ));
        }
    }

    Ok(commands)
}

pub(crate) fn env_for(
    manifest: &Manifest,
    config: &Config,
    root: &Path,
) -> Result<Vec<(String, String)>, PluginError> {
    const SAFE: &[&str] = &[
        "PATH",
        "SystemRoot",
        "WINDIR",
        "TEMP",
        "TMP",
        "TMPDIR",
        "RUST_BACKTRACE",
        "LLVM_PROFILE_FILE",
        "KRABB_HOME",
        "KRABB_NAME",
        "KRABB_MEDIA",
        "KRABB_CODEX_HOME",
        "KRABB_CODEX_BINARY",
        "KRABB_ROOT",
        "KRABB_MODEL_PLUGIN",
        "KRABB_MODEL",
        "KRABB_KEYRING",
        "KRABB_MEMORY",
        "KRABB_TIMER",
        "KRABB_DB",
        "KRABB_WHISPER",
        "KRABB_WHISPER_COMMAND",
        "KRABB_CODEX_BASE_URL",
        "KRABB_CODEX_CONTEXT_LENGTH",
        "KRABB_OPENAI_BASE_URL",
        "KRABB_OPENAI_MODEL",
        "KRABB_OPENAI_CONTEXT_LENGTH",
        "KRABB_ACP_COMMAND",
        "KRABB_ACP_ARGS",
        "KRABB_A2A_AGENT_CARD",
        "KRABB_A2A_RPC_ORIGIN",
        "KRABB_RUNNER_AGENT_CARD",
        "KRABB_RUNNER_RPC_ORIGIN",
        "KRABB_AG_UI_LISTEN",
        "KRABB_AG_UI_ORIGINS",
        "KRABB_OPENROUTER_MODEL",
        "KRABB_OPENROUTER_BASE_URL",
        "KRABB_OPENROUTER_REFERER",
        "KRABB_OPENROUTER_TITLE",
        "KRABB_ANTHROPIC_BASE_URL",
        "KRABB_GEMINI_BASE_URL",
        "KRABB_OLLAMA_HOST",
        "KRABB_OLLAMA_DEPLOYMENT",
        "KRABB_OLLAMA_REVISION",
        "KRABB_SIGNAL_COMMAND",
        "KRABB_SLACK_CHANNELS",
        "KRABB_DISCORD_GATEWAY_URL",
        "KRABB_DISCORD_INTENTS",
        "KRABB_WHATSAPP_PHONE",
        "KRABB_WHATSAPP_LISTEN",
        "KRABB_WHATSAPP_GRAPH_URL",
        "KRABB_PLAYWRIGHT_MODULE",
        "KRABB_PLAYWRIGHT_BROWSERS_PATH",
        "KRABB_NODE",
        "KRABB_BROWSER_ALLOWED_HOSTS",
        "KRABB_BROWSER_EGRESS_PROXY",
        "KRABB_COMPUTER_MODE",
        "KRABB_COMPUTER_ALLOW_HOST_CONTROL",
        "KRABB_COMPUTER_DISPLAY",
        "KRABB_XVFB",
        "KRABB_COMPUTER_STARTUP",
        "KRABB_COMPUTER_STARTUP_ARGS",
    ];

    let mut values = SAFE
        .iter()
        .filter_map(|name| std::env::var(name).ok().map(|value| ((*name).to_owned(), value)))
        .collect::<Vec<_>>();

    ensure_home(&mut values, root);
    ensure_root(&mut values, root);
    values.retain(|(name, _)| name != "KRABB_NAME");
    values.push(("KRABB_NAME".into(), config.name.clone()));
    ensure_codex_home(&mut values);
    let (provider, model) = selection::current(config, (None, None), (None, None));
    values.retain(|(name, _)| !matches!(name.as_str(), "KRABB_MODEL_PLUGIN" | "KRABB_MODEL"));
    values.push(("KRABB_MODEL_PLUGIN".into(), provider));
    values.push(("KRABB_MODEL".into(), model));

    if manifest.id == "scheduler" {
        values.retain(|(name, _)| name != "KRABB_HOME");
        values.push(("KRABB_HOME".into(), root.display().to_string()));
        values.push(("KRABB_MISSED_RUNS".into(), config.scheduler.missed.clone()));

        if let Some(timezone) = &config.scheduler.timezone {
            values.push(("KRABB_TIMEZONE".into(), timezone.clone()));
        }
    }

    if manifest.id == "tools" {
        ensure_root(&mut values, root);
        let shell_enabled = config.tui_shell()
            || (config.tools && config.shell)
            || config.channels.values().any(|channel| channel.tools && channel.shell);
        values.push(("KRABB_SHELL".into(), if shell_enabled { "on" } else { "off" }.into()));

        for name in ["KRABB_SANDBOX_RUNTIME", "KRABB_SANDBOX_IMAGE"] {
            if let Some(value) = std::env::var_os(name) {
                values.push((name.into(), value.to_string_lossy().into_owned()));
            }
        }
    }

    let names = manifest
        .variables
        .iter()
        .cloned()
        .chain(manifest.settings.iter().map(|setting| setting.name.clone()))
        .collect::<Vec<_>>();
    let mut configured = credentials::resolve(&names, root)?;

    for setting in &manifest.settings {
        if let Some(default) = &setting.default {
            configured.entry(setting.name.clone()).or_insert_with(|| default.clone());
        }
    }

    values.retain(|(name, _)| !configured.contains_key(name));
    values.extend(configured);

    Ok(values)
}

pub(crate) fn ensure_home(values: &mut Vec<(String, String)>, root: &Path) {
    if !values.iter().any(|(name, _)| name == "KRABB_HOME") {
        values.push(("KRABB_HOME".into(), root.display().to_string()));
    }
}

pub(crate) fn ensure_root(values: &mut Vec<(String, String)>, home_root: &Path) {
    if !values.iter().any(|(name, _)| name == "KRABB_ROOT") {
        values.push(("KRABB_ROOT".into(), home_root.join("workspace").display().to_string()));
    }
}

pub(crate) fn ensure_codex_home(values: &mut Vec<(String, String)>) {
    if values.iter().any(|(name, _)| name == "KRABB_CODEX_HOME") {
        return;
    }

    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        values.push((
            "KRABB_CODEX_HOME".into(),
            PathBuf::from(home).join(".codex").display().to_string(),
        ));
    }
}

pub(crate) fn ready(id: &str) -> bool {
    ready_at(id, Some(&setup::path(&home())), &credentials::key_path(&home()))
}

pub(crate) fn ready_at(id: &str, credentials: Option<&Path>, key: &Path) -> bool {
    ready_with(id, credentials, key, keyring_ready)
}

pub(crate) fn ready_with(
    id: &str,
    credentials: Option<&Path>,
    key: &Path,
    keyring: impl Fn(&str) -> bool,
) -> bool {
    match id {
        "telegram" => {
            env_nonempty("KRABB_TELEGRAM_TOKEN")
                || keyring("telegram")
                || file_credential_at(credentials, key, &["KRABB_TELEGRAM_TOKEN"])
        }

        "discord" => {
            env_nonempty("KRABB_DISCORD_TOKEN")
                || keyring("discord")
                || file_credential_at(credentials, key, &["KRABB_DISCORD_TOKEN"])
        }

        "codex" => {
            env_nonempty("KRABB_CODEX_KEY")
                || keyring("codex")
                || file_credential_at(credentials, key, &["KRABB_CODEX_KEY"])
                || codex_auth()
        }

        "anthropic" => {
            env_nonempty("KRABB_ANTHROPIC_KEY")
                || keyring("anthropic")
                || file_credential_at(credentials, key, &["KRABB_ANTHROPIC_KEY"])
        }

        "ollama" => true,

        "openai" => {
            env_nonempty("KRABB_OPENAI_KEY")
                || keyring("openai")
                || file_credential_at(credentials, key, &["KRABB_OPENAI_KEY"])
        }

        "openrouter" => {
            env_nonempty("KRABB_OPENROUTER_KEY")
                || file_credential_at(credentials, key, &["KRABB_OPENROUTER_KEY"])
        }

        "gemini" => {
            env_nonempty("KRABB_GEMINI_KEY")
                || file_credential_at(credentials, key, &["KRABB_GEMINI_KEY"])
        }

        "signal" => {
            env_nonempty("KRABB_SIGNAL_ACCOUNT")
                || file_credential_at(credentials, key, &["KRABB_SIGNAL_ACCOUNT"])
        }

        "slack" => {
            env_nonempty("KRABB_SLACK_BOT_TOKEN")
                || file_credential_at(credentials, key, &["KRABB_SLACK_BOT_TOKEN"])
        }

        "whatsapp" => {
            (env_nonempty("KRABB_WHATSAPP_TOKEN")
                || file_credential_at(credentials, key, &["KRABB_WHATSAPP_TOKEN"]))
                && (env_nonempty("KRABB_WHATSAPP_APP_SECRET")
                    || file_credential_at(credentials, key, &["KRABB_WHATSAPP_APP_SECRET"]))
                && (env_nonempty("KRABB_WHATSAPP_VERIFY")
                    || file_credential_at(credentials, key, &["KRABB_WHATSAPP_VERIFY"]))
                && (env_nonempty("KRABB_WHATSAPP_PHONE")
                    || file_credential_at(credentials, key, &["KRABB_WHATSAPP_PHONE"]))
                && (env_nonempty("KRABB_WHATSAPP_GRAPH_URL")
                    || file_credential_at(credentials, key, &["KRABB_WHATSAPP_GRAPH_URL"]))
        }

        _ => binary_at(id, &home()).is_some(),
    }
}

pub(crate) fn updates(
    root: &Path,
    mode: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if !matches!(mode, "off" | "check" | "prompt" | "auto") {
        return Err(format!("Unsupported update mode: {mode}.").into());
    }

    if mode == "off" {
        return Ok(());
    }

    let lock = load_lock_at(root)?;
    let mut changed_ids = Vec::new();

    for (id, entry) in &lock.plugins {
        match changed(root, id, entry) {
            Ok(true) => changed_ids.push(id.as_str()),

            Ok(false) => {}
            Err(error) => warn!(
                plugin = %id,
                error = %diagnostic(sentence(error.to_string())),
                "Plugin update check failed."
            ),
        }
    }

    let changed = changed_ids;

    if changed.is_empty() {
        return Ok(());
    }

    match mode {
        "check" => info!(plugins = %changed.join(", "), "Plugin updates are available."),
        "prompt" => info!("Plugin updates are available; run `krabb plugin update` to review."),
        "auto" => update_at(root, false, false)?,
        _ => unreachable!("update mode was validated above"),
    }

    Ok(())
}

pub(crate) fn changed(
    root: &Path,
    id: &str,
    entry: &Entry,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    if let Some((official, version)) = index::official(entry) {
        let index = index::load(root)?;

        let choice = match index::select(&index, official, version, Protocol::CURRENT) {
            Ok(choice) => choice,

            Err(error) => {
                debug!(plugin = %id, error = %error, "No plugin version is selectable.");
                return Ok(false);
            }
        };

        return Ok(
            index::newer(&choice.release.version, entry) && root.join("plugins").join(id).is_dir()
        );
    }

    let pin = entry.pinned.then_some(entry.revision.as_str());
    let source = resolve(&entry.source, pin)?;
    let Some(binary) = plugin_binary(&source.path, id, entry.default) else {
        return Ok(false);
    };

    let hash = digest(&source.path.join("krabb-plugin.toml"), &binary)?;
    let revision_changed = entry.revision != "local"
        && !entry.revision.is_empty()
        && revision(&source.path) != entry.revision;

    Ok((hash != entry.hash || revision_changed) && root.join("plugins").join(id).is_dir())
}

pub(crate) fn plugin_name(id: &str) -> String {
    krabb_core::plugin::binary_name(id)
}

pub(crate) fn plugin_binary(source: &Path, id: &str, default: bool) -> Option<PathBuf> {
    let name = plugin_name(id);
    let binary = source.join("bin").join(&name);

    if binary.is_file() {
        return Some(binary);
    }

    if !default {
        return None;
    }

    let mut root = source;

    while let Some(parent) = root.parent() {
        let binary = parent.join("target/debug").join(&name);

        if binary.is_file() {
            return Some(binary);
        }

        root = parent;
    }

    None
}

pub(crate) fn env_nonempty(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| !value.trim().is_empty())
}

pub(crate) fn keyring_ready(name: &str) -> bool {
    if std::env::var("KRABB_KEYRING").ok().as_deref() != Some("1") {
        return false;
    }

    keyring::Entry::new("it.airscript.krabb", name)
        .ok()
        .and_then(|entry| entry.get_password().ok())
        .is_some_and(|value| !value.trim().is_empty())
}

pub(crate) fn file_credential_at(path: Option<&Path>, key: &Path, names: &[&str]) -> bool {
    let Some(path) = path else {
        return false;
    };

    credential(path, key, names)
}

pub(crate) fn credential(path: impl AsRef<Path>, key: &Path, names: &[&str]) -> bool {
    let Ok(values) = credentials::read(path.as_ref(), key) else {
        return false;
    };

    names.iter().any(|name| values.get(*name).is_some_and(|value| !value.trim().is_empty()))
}

pub(crate) fn codex_auth() -> bool {
    let binary = std::env::var_os("KRABB_CODEX_BINARY").unwrap_or_else(|| "codex".into());
    let mut command = std::process::Command::new(binary);
    command
        .args(["login", "status"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    if let Some(path) = codex_home() {
        command.env("CODEX_HOME", path);
    }

    let Ok(mut child) = command.spawn() else {
        return false;
    };

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);

    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),

            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }

            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

pub(crate) fn codex_home() -> Option<PathBuf> {
    std::env::var_os("KRABB_CODEX_HOME").map(PathBuf::from).or_else(|| {
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(|home| PathBuf::from(home).join(".codex"))
    })
}
