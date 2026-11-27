//! Runs plugin-provided commands and renders their help and output.

use crate::*;

pub(crate) async fn external_command(
    args: Vec<String>,
    json: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    external_command_at(args, json, home()).await
}

pub(crate) async fn external_command_at(
    args: Vec<String>,
    json: bool,
    root: PathBuf,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Some(name) = args.first() else {
        return Err("A plugin command is required.".into());
    };

    let commands = available_commands(&root)?;
    let Some(owners) = commands.get(name) else {
        return Err(format!(
            "Unknown command: {name}. Run `krabb --help` to list commands, including those \
             added by installed plugins."
        )
        .into());
    };

    if owners.len() != 1 {
        return Err(format!("Plugin command {name} has conflicting registrations.").into());
    }

    let (owner, _) =
        owners.first().cloned().ok_or_else(|| format!("Plugin command {name} was not found."))?;

    if let Some(id) = owner.strip_prefix(&format!("{RUNTIME_COMMAND_OWNER}setup:")) {
        return setup::command(&args[1..], id, &root, json).await;
    }

    let plugin = if owner.starts_with(RUNTIME_COMMAND_OWNER) {
        match name.as_str() {
            "session" => return session_command(&args[1..], &root, json).await,
            "delivery" => return delivery_command(&args[1..], &root, json).await,
            "schedule" => return schedule_command(&args[1..], &root, json).await,
            "memory" | "tui" => name.clone(),
            "data" => "sqlite".into(),
            _ => return Err(format!("Unknown native command {name}.").into()),
        }
    } else {
        owner
    };

    let has_setup = name == &plugin
        && load_lock_at(&root)?
            .plugins
            .get(&plugin)
            .is_some_and(|entry| !entry.variables.is_empty() || !entry.settings.is_empty());

    if has_setup && setup::requested(&args[1..]) {
        return setup::command(&args[1..], &plugin, &root, json).await;
    }

    let help_requested = external_command_help_requested(name, &args);

    if help_requested && !matches!(name.as_str(), "memory" | "data") {
        println!("{}", style_cli_help(&external_command_help(name, &owners[0].1.description)));

        if has_setup {
            println!(
                "\n  setup [--no-activate]  Securely configure declared secrets and settings."
            );
        }

        return Ok(());
    }

    let mut command_args = external_command_args(name, &args[1..]);
    let path = binary_at(&plugin, &root)
        .ok_or_else(|| format!("Plugin binary was not found: {plugin}."))?;

    let config = load_config_at(&root)?;

    if plugin == krabb_core::client::TUI {
        return tui_command(path, &command_args, &root, &config).await;
    }

    let acp_workspace = if name == "acp" {
        let workspace = workspace_root().canonicalize()?;
        let selected = command_args
            .iter()
            .rposition(|argument| argument == "--workspace")
            .map(|index| command_args.get(index + 1).ok_or("An ACP workspace is required."))
            .transpose()?;
        let workspace =
            selected_workspace(&serde_json::json!({"workspace": selected}), &workspace)?;
        command_args.extend(["--workspace".into(), workspace.display().to_string()]);
        Some(workspace)
    } else {
        None
    };

    let mut process = launch(&root, path, &plugin, None, false, &config).await?;
    let timeout = if name == "codex" && args.get(1).is_some_and(|argument| argument == "login") {
        std::time::Duration::from_secs(960)
    } else if matches!(name.as_str(), "code" | "acp") {
        TURN_LIMIT
    } else {
        std::time::Duration::from_secs(120)
    };

    let result = if matches!(name.as_str(), "code" | "acp") {
        let model = if name == "code" {
            let (model_id, _) = selection::current(&config, (None, None), (None, None));
            let model_path = binary_at(&model_id, &root).ok_or_else(|| {
                format!("The configured intelligence plugin {model_id} is not installed.")
            })?;

            Some(
                launch(&root, model_path, &model_id, Some(Capability::Model), false, &config)
                    .await?,
            )
        } else {
            None
        };

        let model = Arc::new(AsyncMutex::new(model));
        let tools = binary_at("tools", &root).map(|path| async {
            launch(&root, path, "tools", Some(Capability::Tool), false, &config).await
        });

        let tools = Arc::new(AsyncMutex::new(match tools {
            Some(future) => Some(future.await?),
            None => None,
        }));

        let workspace = acp_workspace.unwrap_or_else(|| {
            workspace_root().canonicalize().unwrap_or_else(|_| workspace_root())
        });

        let model_process = Arc::clone(&model);
        let tool_process = Arc::clone(&tools);
        let bridge = Bridge::new(&config);
        let (model_events, _) = tokio::sync::mpsc::unbounded_channel();
        process
            .call_full_timeout_async(
                Request::call(
                    1,
                    "command",
                    serde_json::json!({"name": name, "args": &command_args, "json": json}),
                ),
                timeout,
                |request| {
                    write_command_event(
                        &request,
                        json,
                        &mut std::io::stdout(),
                        &mut std::io::stderr(),
                    );

                    std::future::ready(Ok(()))
                },
                |request| {
                    agent_request(
                        request,
                        Arc::clone(&model_process),
                        Arc::clone(&tool_process),
                        workspace.clone(),
                        bridge,
                        json,
                        model_events.clone(),
                    )
                },
            )
            .await
    } else if name == "ag-ui" {
        let (model_id, _) = selection::current(&config, (None, None), (None, None));
        let model_path = binary_at(&model_id, &root).ok_or_else(|| {
            format!("The configured intelligence plugin {model_id} is not installed.")
        })?;
        let model = Arc::new(AsyncMutex::new(Some(
            launch(&root, model_path, &model_id, Some(Capability::Model), false, &config).await?,
        )));
        let tools = binary_at("tools", &root).map(|path| async {
            launch(&root, path, "tools", Some(Capability::Tool), false, &config).await
        });

        let tools = Arc::new(AsyncMutex::new(match tools {
            Some(future) => Some(future.await?),
            None => None,
        }));
        let model_process = Arc::clone(&model);
        let tool_process = Arc::clone(&tools);
        let workspace = workspace_root().canonicalize().unwrap_or_else(|_| workspace_root());
        let bridge = Bridge::new(&config);

        let listener = process.listen_stream_async(
            Request::call(
                1,
                "command",
                serde_json::json!({"name": name, "args": &command_args, "json": json}),
            ),
            |request| {
                if let Some(text) = command_event_text(&request) {
                    use std::io::Write;

                    if json {
                        eprintln!("{text}");
                        let _ = std::io::stderr().flush();
                    } else {
                        println!("{text}");
                        let _ = std::io::stdout().flush();
                    }
                }

                std::future::ready(Ok(()))
            },
            |request, events| {
                agent_request(
                    request,
                    Arc::clone(&model_process),
                    Arc::clone(&tool_process),
                    workspace.clone(),
                    bridge,
                    json,
                    events,
                )
            },
        );
        tokio::pin!(listener);

        tokio::select! {
            response = &mut listener => response,

            signal = tokio::signal::ctrl_c() => {
                signal?;
                Ok(Response::ok(1, serde_json::json!({"stopped": true})))
            }
        }
    } else {
        process
            .call_stream_timeout_async(
                Request::call(
                    1,
                    "command",
                    serde_json::json!({"name": name, "args": &command_args, "json": json}),
                ),
                timeout,
                |request| {
                    write_command_event(
                        &request,
                        json,
                        &mut std::io::stdout(),
                        &mut std::io::stderr(),
                    );

                    std::future::ready(Ok(()))
                },
            )
            .await
    };

    let stopped = process.stop().await;
    let response = result.map_err(|error| command_failure(name, timeout, error))?;
    stopped?;

    if let Some(error) = response.error {
        return Err(error.message.into());
    }

    let value = response.result.ok_or("Plugin command returned no result.")?;
    let output = if name == "data" && !json {
        format_data_command_output(&command_args, &value)
    } else {
        external_command_output(&value, json)?
    };

    let is_custom_help = !json
        && matches!(name.as_str(), "memory" | "data")
        && command_args
            .iter()
            .any(|argument| matches!(argument.as_str(), "help" | "--help" | "-h"));

    if is_custom_help {
        println!("{}", style_cli_help(&output));
    } else {
        println!("{output}");
    }

    Ok(())
}

/// Names the limit a timed-out plugin command reached and how to try again.
fn command_failure(
    name: &str,
    limit: std::time::Duration,
    error: krabb_core::Error,
) -> Box<dyn std::error::Error + Send + Sync> {
    match error {
        krabb_core::Error::Io(error) if error.kind() == std::io::ErrorKind::TimedOut => format!(
            "`krabb {name}` stopped after its {}-second limit.\nRun it again with a smaller task.",
            limit.as_secs()
        )
        .into(),

        error => error.into(),
    }
}

/// Display variables a terminal client needs to read the clipboard; they carry no secrets.
const DISPLAY: [&str; 4] = ["DISPLAY", "WAYLAND_DISPLAY", "XDG_RUNTIME_DIR", "XAUTHORITY"];

pub(crate) async fn tui_command(
    path: PathBuf,
    args: &[String],
    root: &Path,
    config: &Config,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let running = matches!(service_manager_state(), Ok(ServiceState::Active));

    require_tui_daemon(root, running, DAEMON_STARTUP_CHECKS).await?;

    let tui_manifest = verify_plugin_integrity(root, "tui", &path)?;

    if !tui_manifest
        .commands
        .iter()
        .any(|command| command.name == krabb_core::client::TUI && command.interactive)
    {
        return Err("The TUI plugin does not advertise its interactive command.".into());
    }

    let one_shot = args.iter().any(|argument| argument == "--once");

    if !one_shot && (!std::io::stdin().is_terminal() || !std::io::stdout().is_terminal()) {
        return Err(
            "The TUI needs an interactive terminal. Use `krabb tui --once <prompt>` instead."
                .into(),
        );
    }

    config.validate()?;
    let (plugin, model) = selection::tui(config, args);

    let plugin_path = binary_at(&plugin, root);
    let launch_directory = std::env::current_dir()?;
    let mut command = tokio::process::Command::new(path);
    command
        .env_clear()
        .arg("--krabb-cli")
        .args(args)
        .env("KRABB_HOME", root)
        .env("KRABB_NAME", &config.name)
        .envs(std::env::var_os("LLVM_PROFILE_FILE").map(|value| ("LLVM_PROFILE_FILE", value)))
        .envs(DISPLAY.iter().filter_map(|name| std::env::var_os(name).map(|value| (name, value))))
        .env("KRABB_TUI_LAUNCH_DIR", launch_directory)
        .env(
            "KRABB_TUI_THEME",
            if config.clients.get("tui").is_none_or(|client| client.theme) { "on" } else { "off" },
        )
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit());

    if let Some(plugin_path) = plugin_path {
        let manifest = verify_plugin_integrity(root, &plugin, &plugin_path)?;

        if !manifest.capabilities.iter().any(|capability| capability == "model") {
            return Err(format!("Plugin {plugin} is not an intelligence plugin.").into());
        }

        command.envs(env_for(&manifest, config, root)?);
    }

    if let Ok(value) = std::env::var("KRABB_ROOT") {
        command.env("KRABB_ROOT", value);
    }

    command.env("KRABB_MODEL_PLUGIN", &plugin).env("KRABB_MODEL", model);

    let status = command.status().await?;

    if !status.success() {
        return Err(format!("TUI exited with status {status}.").into());
    }

    Ok(())
}

/// How many half-second checks the TUI and init wait for a service that runs but does not
/// answer yet, since the daemon starts every plugin before it answers.
pub(crate) const DAEMON_STARTUP_CHECKS: u32 = 60;

/// Requires a daemon that answers. A RUNNING service may still be loading plugins or waiting
/// for keychain access, so the TUI gives it up to CHECKS half-second checks first.
pub(crate) async fn require_tui_daemon(
    root: &Path,
    running: bool,
    checks: u32,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if control_at("status", serde_json::json!({}), root).await?.is_some() {
        return Ok(());
    }

    if !running {
        return Err(concat!(
            "The Krabb daemon is not running.\n",
            "Start it with `krabb service start` or run `krabb-daemon`."
        )
        .into());
    }

    for _ in 0..checks {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        if control_at("status", serde_json::json!({}), root).await?.is_some() {
            return Ok(());
        }
    }

    Err(format!(
        "The Krabb service is running but not answering yet.\nIf the keychain asks for \
         access, choose Always Allow; otherwise see the log in {}.",
        root.join("logs").display()
    )
    .into())
}

pub(crate) fn verify_plugin_integrity(
    root: &Path,
    id: &str,
    path: &Path,
) -> Result<Manifest, Box<dyn std::error::Error + Send + Sync>> {
    let manifest_path = path
        .parent()
        .and_then(Path::parent)
        .ok_or("Plugin manifest path is invalid.")?
        .join("krabb-plugin.toml");

    let manifest = read_manifest_checked(manifest_path.parent().unwrap_or(Path::new(".")))?;

    let lock = load_lock_at(root)?;
    let entry = lock.plugins.get(id).ok_or_else(|| format!("Plugin {id} is not locked."))?;

    if manifest.id != id {
        return Err(format!("Plugin integrity check failed: {id}.").into());
    }

    verify_plugin_binary(id, entry, &manifest_path, path)?;

    let mut capabilities = manifest.capabilities.clone();
    capabilities.sort();
    let mut locked_capabilities = entry.capabilities.clone();
    locked_capabilities.sort();

    if capabilities != locked_capabilities {
        return Err(format!("Plugin integrity check failed: {id}.").into());
    }

    let mut commands = manifest.commands.clone();
    commands.sort_by(|left, right| left.name.cmp(&right.name));
    let mut locked_commands = entry.commands.clone();
    locked_commands.sort_by(|left, right| left.name.cmp(&right.name));

    if commands != locked_commands {
        return Err(format!("Plugin integrity check failed: {id}.").into());
    }

    Ok(manifest)
}

pub(crate) fn verify_plugin_binary(
    id: &str,
    entry: &Entry,
    manifest: &Path,
    binary: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let valid = if entry.linked {
        let metadata = std::fs::symlink_metadata(binary)?;
        let source = entry.source.strip_prefix("file://").unwrap_or(&entry.source);
        let source = source.split('#').next().unwrap_or(source);
        let source = relocated_plugin_source(Path::new(source)).unwrap_or_else(|| source.into());
        let source_root = std::fs::canonicalize(source)?;
        let expected = plugin_binary(&source_root, id, entry.default)
            .ok_or_else(|| format!("Linked plugin source is unavailable: {id}."))?;

        let actual = std::fs::canonicalize(binary)?;
        let expected = std::fs::canonicalize(expected)?;
        let source_manifest = std::fs::read(source_root.join("krabb-plugin.toml"))?;
        let installed_manifest = std::fs::read(manifest)?;

        metadata.file_type().is_symlink()
            && actual == expected
            && source_manifest == installed_manifest
    } else {
        digest(manifest, binary)? == entry.hash
    };

    if !valid {
        return Err(format!("Plugin integrity check failed: {id}.").into());
    }

    Ok(())
}

pub(crate) fn cli_styles() -> clap::builder::Styles {
    use clap::builder::styling::{RgbColor, Style};

    if !cli_colors_enabled() {
        return clap::builder::Styles::plain();
    }

    let (red, green, blue) = crate::style::CORAL;
    let accent = Style::new().fg_color(Some(RgbColor(red, green, blue).into())).bold();

    clap::builder::Styles::styled().header(accent).usage(accent).literal(Style::new().bold())
}

pub(crate) fn cli_colors_enabled() -> bool {
    std::io::stdout().is_terminal() && crate::style::allowed()
}

pub(crate) fn style_cli_help(help: &str) -> String {
    if !cli_colors_enabled() {
        return help.to_owned();
    }

    let mut output = String::with_capacity(help.len() + 64);

    for line in help.split_inclusive('\n') {
        let (content, newline) =
            line.strip_suffix('\n').map_or((line, ""), |content| (content, "\n"));

        if matches!(
            content,
            "Commands:"
                | "Options:"
                | "Notes:"
                | "Timing:"
                | "History options:"
                | "Conditional Commands:"
                | "Plugin Commands:"
        ) {
            output.push_str(crate::style::accent_code());
            output.push_str(content);
            output.push_str("\x1b[0m");
        } else if let Some(usage) = content.strip_prefix("Usage: ") {
            output.push_str(crate::style::accent_code());
            output.push_str("Usage:\x1b[0m \x1b[1m");
            output.push_str(usage);
            output.push_str("\x1b[0m");
        } else if let Some(gap) = cli_help_row_gap(content) {
            output.push_str("\x1b[1m");
            output.push_str(&content[..gap]);
            output.push_str("\x1b[0m\x1b[90m");
            output.push_str(&content[gap..]);
            output.push_str("\x1b[0m");
        } else {
            output.push_str(content);
        }

        output.push_str(newline);
    }

    output
}

pub(crate) fn cli_help_row_gap(line: &str) -> Option<usize> {
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();

    if indent == 0 {
        return None;
    }

    if indent + 1 >= line.len() {
        return None;
    }

    line[indent + 1..]
        .match_indices("  ")
        .map(|(index, _)| indent + 1 + index)
        .find(|index| *index > indent + 1)
}

pub(crate) fn external_command_help(name: &str, description: &str) -> String {
    if name == krabb_core::client::TUI {
        return format!(
            "{description}\n\nUsage: krabb tui [OPTIONS]\n\nOptions:\n  --once <prompt>   Send one prompt without opening the full-screen interface\n  --session <id>    Select a session (default: tui)\n  --model <name>    Select the model (default: environment, config, or default)\n  --plugin <id>     Select the intelligence plugin (default: environment, config, or codex)\n  -h, --help        Print help"
        );
    }

    if name == "codex" {
        return format!(
            "{description}\n\nUsage: krabb codex <COMMAND>\n\nCommands:\n  login [--device]  Sign in to your Codex account.\n  status            Show whether Codex is signed in.\n  logout            Sign out of Codex.\n  models            List models available to this Codex account.\n  help              Show this help.\n\nRun `krabb codex <command> --help` for command-specific help."
        );
    }

    format!("Usage: krabb {name} [arguments...]\n{description}")
}

pub(crate) fn external_command_help_requested(name: &str, args: &[String]) -> bool {
    args.get(1).is_some_and(|argument| {
        argument == "--help"
            || argument == "-h"
            || (name == krabb_core::client::TUI && argument == "help")
    })
}

pub(crate) fn external_command_args(name: &str, args: &[String]) -> Vec<String> {
    if (name == "data" && args.iter().any(|arg| arg == "--help" || arg == "-h"))
        || (name == "memory" && args.first().is_some_and(|arg| arg == "--help" || arg == "-h"))
    {
        vec!["help".into()]
    } else if name == "data" {
        args.iter()
            .filter(|argument| !matches!(argument.as_str(), "--json" | "--debug" | "--verbose"))
            .cloned()
            .collect()
    } else {
        args.to_vec()
    }
}

pub(crate) fn format_data_command_output(args: &[String], value: &Value) -> String {
    if let Some(help) = value.as_str() {
        return help.to_owned();
    }

    let args = if args.first().is_some_and(|argument| argument == "--scope") {
        args.get(2..).unwrap_or_default()
    } else {
        args
    };

    let args = args.iter().map(String::as_str).collect::<Vec<_>>();

    match args.as_slice() {
        ["database", "list"] => {
            let Some(databases) = value["databases"].as_array() else {
                return value.to_string();
            };

            if databases.is_empty() {
                return "No databases found.".into();
            }

            format!(
                "Databases:\n{}",
                format_tree_list(
                    &databases
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                )
            )
        }

        ["database", "create", database] => {
            if value["created"] == true {
                format!("Created database {database}.")
            } else {
                format!("Database {database} already exists.")
            }
        }

        ["database", "drop", database] => {
            if value["deleted"] == true {
                format!("Deleted database {database}.")
            } else {
                format!("Database {database} was not found.")
            }
        }

        ["table", "list", database] => {
            let Some(tables) = value["tables"].as_array() else {
                return value.to_string();
            };

            if tables.is_empty() {
                return format!("No tables found in database {database}.");
            }

            format!(
                "Tables in {database}:\n{}",
                format_tree_list(
                    &tables.iter().filter_map(Value::as_str).map(str::to_owned).collect::<Vec<_>>()
                )
            )
        }

        ["table", "create", database, table, ..] => {
            let Some(columns) = value["columns"].as_array() else {
                return value.to_string();
            };

            if columns.is_empty() {
                format!(
                    "Created empty table {database}.{table}.\n\nAdd a column with:\n  krabb data column add {database} {table} NAME TYPE"
                )
            } else {
                let columns = columns.iter().map(format_data_column).collect::<Vec<_>>().join(", ");
                format!("Created table {database}.{table} with columns: {columns}.")
            }
        }

        ["table", "describe", database, table] => {
            let Some(columns) = value["columns"].as_array() else {
                return value.to_string();
            };

            if columns.is_empty() {
                format!("Table {database}.{table} has no columns yet.")
            } else {
                format!(
                    "Columns in {database}.{table}:\n{}",
                    format_tree_list(&columns.iter().map(format_data_column).collect::<Vec<_>>())
                )
            }
        }

        ["column", "add", database, table, ..] => {
            let column = format_data_column(&value["column"]);
            format!("Added column {column} to {database}.{table}.")
        }

        ["column", "remove", database, table, column] => {
            format!("Removed column {column} from {database}.{table}.")
        }

        ["table", "drop", database, table] => {
            if value["deleted"] == true {
                format!("Deleted table {database}.{table}.")
            } else {
                format!("Table {database}.{table} was not found.")
            }
        }

        ["insert", database, table, ..] => {
            format!("Inserted record #{} into {database}.{table}.", value["id"])
        }

        ["query", database, table, ..] => {
            let Some(items) = value["items"].as_array() else {
                return value.to_string();
            };

            if items.is_empty() {
                return format!("No records found in {database}.{table}.");
            }

            let records = items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    let details = item["record"]
                        .as_object()
                        .into_iter()
                        .flatten()
                        .map(|(name, value)| format!("{name}={}", format_data_value(value)))
                        .collect::<Vec<_>>();

                    format_tree_entry(
                        &format!("#{}", item["id"]),
                        &details,
                        index + 1 == items.len(),
                    )
                })
                .collect::<Vec<_>>();
            let mut output = format!("Records in {database}.{table}:\n{}", records.join("\n"));

            output.push_str(&format!("Showing {} record(s).", items.len()));

            if value["truncated"] == true {
                output.push_str(&format!(" Continue after ID {}.", value["next_after"]));
            }

            output
        }

        ["update", database, table, id, ..] => {
            format!("Updated record #{id} in {database}.{table}.")
        }

        ["delete", database, table, id] => {
            if value["deleted"] == true {
                format!("Deleted record #{id} from {database}.{table}.")
            } else {
                format!("Record #{id} was not found in {database}.{table}.")
            }
        }

        _ => value.to_string(),
    }
}

pub(crate) fn format_data_column(value: &Value) -> String {
    let name = value["name"].as_str().unwrap_or("column");
    let kind = value["type"].as_str().unwrap_or("json");
    let required = if value["required"] == true { ", required" } else { "" };

    format!("{name} ({kind}{required})")
}

pub(crate) fn format_data_value(value: &Value) -> String {
    if let Some(value) = value.as_str() {
        return value.replace('\n', "\\n");
    }

    value.to_string()
}

pub(crate) fn command_event_text(request: &Request) -> Option<&str> {
    if let Request::Note { method, params, .. } = request
        && method == "event"
    {
        return params["event"]["text"].as_str();
    }

    None
}

pub(crate) fn write_command_event(
    request: &Request,
    json: bool,
    stdout: &mut impl std::io::Write,
    stderr: &mut impl std::io::Write,
) {
    if let Some(text) = command_event_text(request) {
        let output: &mut dyn std::io::Write = if json { stderr } else { stdout };

        let _ = output.write_all(text.as_bytes());
        let _ = output.flush();
    }
}

pub(crate) fn external_command_output(
    value: &Value,
    json: bool,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    if json {
        return Ok(serde_json::to_string_pretty(value)?);
    }

    if let Some(text) = value.as_str().or_else(|| value["text"].as_str()) {
        return Ok(text.to_owned());
    }

    Ok(serde_json::to_string_pretty(value)?)
}

#[cfg(test)]
mod tests {
    #[test]
    fn timed_out_commands_name_their_limit_and_next_action() {
        let error = krabb_core::Error::Io(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "plugin response timed out",
        ));

        let text = super::command_failure("code", super::TURN_LIMIT, error).to_string();

        assert_eq!(
            text,
            "`krabb code` stopped after its 300-second limit.\nRun it again with a smaller task."
        );
    }
}
