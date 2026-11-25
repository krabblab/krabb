//! Installs, inspects, and controls the daemon as an operating-system service.

use crate::*;

pub(crate) fn service(
    mut command: ServiceCommand,
    json: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let path = service_path();

    if let ServiceCommand::Remove(args) = &mut command
        && path.exists()
    {
        if !confirm_action(args.yes, json, "Uninstall the Krabb service?")? {
            print_cancelled("Uninstall", json);
            return Ok(());
        }

        args.yes = true;
    }

    if json { service_at_json(&path, command, true) } else { service_at(&path, command) }
}

pub(crate) const SERVICE_VARIABLES: &[&str] = &[
    "KRABB_ROOT",
    "KRABB_KEYRING",
    "KRABB_CHANNEL",
    "KRABB_MODEL_PLUGIN",
    "KRABB_MODEL",
    "KRABB_MEMORY",
    "KRABB_TIMER",
    "KRABB_DB",
    "KRABB_MEDIA",
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
    "KRABB_SIGNAL_COMMAND",
    "KRABB_SLACK_CHANNELS",
    "KRABB_OLLAMA_HOST",
    "KRABB_OLLAMA_DEPLOYMENT",
    "KRABB_OLLAMA_REVISION",
    "KRABB_OLLAMA_THINK",
    "KRABB_OLLAMA_CONTEXT_LENGTH",
    "KRABB_DISCORD_GATEWAY_URL",
    "KRABB_DISCORD_INTENTS",
    "KRABB_WHATSAPP_PHONE",
    "KRABB_WHATSAPP_LISTEN",
    "KRABB_WHATSAPP_GRAPH_URL",
    "KRABB_CODEX_HOME",
    "KRABB_CODEX_BINARY",
    "KRABB_SANDBOX_RUNTIME",
    "KRABB_SANDBOX_IMAGE",
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

pub(crate) const SERVICE_SECRETS: &[&str] = &[
    "KRABB_TELEGRAM_TOKEN",
    "KRABB_DISCORD_TOKEN",
    "KRABB_CODEX_KEY",
    "KRABB_OPENAI_KEY",
    "KRABB_A2A_TOKEN",
    "KRABB_OPENROUTER_KEY",
    "KRABB_ANTHROPIC_KEY",
    "KRABB_GEMINI_KEY",
    "KRABB_OLLAMA_API_KEY",
    "KRABB_WHATSAPP_TOKEN",
    "KRABB_WHATSAPP_APP_SECRET",
    "KRABB_WHATSAPP_VERIFY",
    "KRABB_SIGNAL_ACCOUNT",
    "KRABB_SLACK_BOT_TOKEN",
    "KRABB_SLACK_APP_TOKEN",
];

pub(crate) fn service_environment(
    path: &Path,
) -> Result<Vec<(String, String)>, Box<dyn std::error::Error + Send + Sync>> {
    let environment = std::env::vars().collect::<BTreeMap<_, _>>();

    service_environment_from(path, home(), &environment)
}

pub(crate) fn service_environment_from(
    path: &Path,
    root: PathBuf,
    environment: &BTreeMap<String, String>,
) -> Result<Vec<(String, String)>, Box<dyn std::error::Error + Send + Sync>> {
    let mut values = vec![("KRABB_HOME".into(), service_path_value(root.clone())?)];

    for name in SERVICE_VARIABLES {
        if let Some(value) = environment.get(*name) {
            let value = PathBuf::from(value);

            let value = if matches!(
                *name,
                "KRABB_ROOT"
                    | "KRABB_MEDIA"
                    | "KRABB_MEMORY"
                    | "KRABB_TIMER"
                    | "KRABB_DB"
                    | "KRABB_CODEX_HOME"
            ) {
                service_path_value(value)?
            } else {
                value.to_string_lossy().into_owned()
            };

            values.push(((*name).into(), value));
        }
    }

    if !environment.contains_key("KRABB_CODEX_HOME")
        && let Some(home) = environment.get("HOME").or_else(|| environment.get("USERPROFILE"))
    {
        values.push((
            "KRABB_CODEX_HOME".into(),
            service_path_value(PathBuf::from(home).join(".codex"))?,
        ));
    }

    if !environment.contains_key("KRABB_CODEX_BINARY")
        && let Some(binary) = environment.get("PATH").and_then(|path| codex_binary_from_path(path))
    {
        values.push(("KRABB_CODEX_BINARY".into(), binary.display().to_string()));
    }

    let configured = environment
        .get("KRABB_CREDENTIALS")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            let path = root.join("credentials.json");
            path.exists().then_some(path)
        });

    let key = environment
        .get("KRABB_CREDENTIALS_KEY")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("credentials.key"));

    let mut credentials = match configured.as_deref() {
        Some(path) => credentials::read(path, &key)?,
        None => BTreeMap::new(),
    };

    let mut direct = false;

    let mut names = SERVICE_SECRETS.iter().map(|name| (*name).to_owned()).collect::<Vec<_>>();

    if let Ok(entries) = std::fs::read_dir(root.join("plugins")) {
        for entry in entries.flatten() {
            if let Some(manifest) = read_manifest(&entry.path()) {
                for name in manifest.variables {
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
            }
        }
    }

    for name in names {
        if let Some(value) = environment.get(&name)
            && !value.trim().is_empty()
        {
            direct = true;
            credentials.insert(name, value.clone());
        }
    }

    if direct {
        let path = service_credentials_path(path);

        let preference =
            credentials::preference_from(environment.get("KRABB_KEYRING").map(String::as_str))?;

        // Saving merges into an existing file, so a fresh staged copy drops an older install's
        // values, and the rename keeps the previous copy until the new one is complete.
        let staged = path.with_extension("json.new");
        let _ = std::fs::remove_file(&staged);

        if let Err(error) = credentials::save(&staged, &key, credentials, preference) {
            let _ = std::fs::remove_file(&staged);

            return Err(error);
        }

        if let Err(error) = std::fs::rename(&staged, &path) {
            let _ = std::fs::remove_file(&staged);

            return Err(error.into());
        }

        values.push(("KRABB_CREDENTIALS".into(), service_path_value(path)?));
    } else if let Some(path) = configured.as_ref() {
        values.push(("KRABB_CREDENTIALS".into(), service_path_value(path.clone())?));
    }

    if direct || configured.is_some() {
        values.push(("KRABB_CREDENTIALS_KEY".into(), service_path_value(key)?));
    }

    Ok(values)
}

pub(crate) fn codex_binary_from_path(path: &str) -> Option<PathBuf> {
    #[cfg(windows)]
    let executable = "codex.exe";

    #[cfg(not(windows))]
    let executable = "codex";

    std::env::split_paths(path)
        .filter(|directory| directory.is_absolute())
        .map(|directory| directory.join(executable))
        .find_map(|candidate| {
            let metadata = std::fs::metadata(&candidate).ok()?;

            if !metadata.is_file() {
                return None;
            }

            #[cfg(unix)]
            if metadata.permissions().mode() & 0o111 == 0 {
                return None;
            }

            candidate.canonicalize().ok()
        })
}

pub(crate) fn service_path_value(path: PathBuf) -> std::io::Result<String> {
    if path.is_absolute() {
        return Ok(path.display().to_string());
    }

    Ok(std::env::current_dir()?.join(path).display().to_string())
}

pub(crate) fn service_credentials_path(path: &Path) -> PathBuf {
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("krabb.service");

    path.with_file_name(format!("{name}.credentials.json"))
}

/// Deletes the secrets copy `krabb service install` writes next to the service definition.
pub(crate) fn remove_service_credentials(path: &Path) -> std::io::Result<()> {
    let copy = service_credentials_path(path);
    let staged = copy.with_extension("json.new");
    let mut files = vec![copy.clone(), staged.clone()];

    for file in [&copy, &staged] {
        if let (Some(parent), Some(name)) = (file.parent(), file.file_name()) {
            files.push(parent.join(format!(".{}.lock", name.to_string_lossy())));
        }
    }

    for file in files {
        match std::fs::remove_file(&file) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),

            _ => {}
        }
    }

    Ok(())
}

pub(crate) fn service_at(
    path: &Path,
    command: ServiceCommand,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    service_at_with(path, command, service_action)
}

pub(crate) fn service_at_json(
    path: &Path,
    command: ServiceCommand,
    json: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let report = service_report(&command, json);

    service_at_with_mode(
        path,
        command,
        |path, action| service_action_with(path, action, report),
        json,
    )
}

/// How a service command's own stop or start reports: uninstall's stop is one step of its single
/// JSON result, while `service start`, `stop`, and `restart` report themselves.
pub(crate) fn service_report(command: &ServiceCommand, json: bool) -> ServiceReport {
    match (json, command) {
        (false, _) => ServiceReport::Text,
        (true, ServiceCommand::Remove(_)) => ServiceReport::Silent,
        (true, _) => ServiceReport::Json,
    }
}

pub(crate) fn service_at_with(
    path: &Path,
    command: ServiceCommand,
    mut action: impl FnMut(&Path, ServiceAction) -> Result<(), Box<dyn std::error::Error + Send + Sync>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    service_at_with_mode(path, command, &mut action, false)
}

pub(crate) fn service_at_with_mode(
    path: &Path,
    command: ServiceCommand,
    action: impl FnMut(&Path, ServiceAction) -> Result<(), Box<dyn std::error::Error + Send + Sync>>,
    json: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    service_at_with_status(path, command, action, service_manager_state, json)
}

pub(crate) fn service_at_with_status(
    path: &Path,
    command: ServiceCommand,
    mut action: impl FnMut(&Path, ServiceAction) -> Result<(), Box<dyn std::error::Error + Send + Sync>>,
    mut state: impl FnMut() -> Result<ServiceState, Box<dyn std::error::Error + Send + Sync>>,
    json: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match command {
        ServiceCommand::Install(args) => {
            match std::fs::symlink_metadata(path) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err("Service definition cannot be a symbolic link.".into());
                }

                Ok(_) if !args.force => {
                    return Err(format!(
                        "A service definition already exists at {}.\nUse `--force` to replace it.",
                        path.display()
                    )
                    .into());
                }

                Ok(_) => {}

                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }

            let executable = daemon_executable()?;
            let environment = service_environment(path)?;

            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            let definition = service_text(&executable, &environment);
            #[cfg(target_os = "windows")]
            {
                windows_service_install(&executable, &environment, args.force, path, &definition)?;
            }

            #[cfg(not(target_os = "windows"))]
            secure(path, definition.as_bytes())?;

            let copy = service_path_value(service_credentials_path(path))?;

            if !environment
                .iter()
                .any(|(name, value)| name == "KRABB_CREDENTIALS" && *value == copy)
            {
                remove_service_credentials(path)?;
            }

            info!("Service installed.");

            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "action": "install",
                        "path": path,
                        "status": "installed",
                    }))?
                );
            } else {
                println!("The service has been installed.\nStart it with `krabb service start`.");
            }
        }

        ServiceCommand::Remove(args) => {
            if !args.yes {
                return Err("To uninstall the service, confirm with `--yes`.".into());
            }

            let installed = path.exists();

            if installed {
                action(path, ServiceAction::Stop)?;
            }

            #[cfg(target_os = "windows")]
            if installed {
                windows_service_remove()?;
            }

            if installed {
                std::fs::remove_file(path)?;
            }

            remove_service_credentials(path)?;

            if installed {
                info!("Service uninstalled.");
            } else {
                debug!("Service uninstall skipped because no installation exists.");
            }

            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "action": "uninstall",
                        "path": path,
                        "status": if installed { "uninstalled" } else { "not installed" },
                    }))?
                );
            } else if installed {
                println!("The service has been uninstalled.");
            } else {
                println!("The service is not installed.");
            }
        }

        ServiceCommand::Status => {
            let installed = path.is_file();
            let status = if installed { state()? } else { ServiceState::NotInstalled };

            if status == ServiceState::Failed {
                warn!("Service is in a failed state.");
            } else {
                debug!(installed, state = status.as_str(), "Service status checked.");
            }

            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "name": service_name(),
                        "path": path,
                        "status": if installed { "installed" } else { "not installed" },
                        "service_status": status.as_str(),
                    }))?
                );
            } else {
                println!("{}", service_status_text(service_name(), status, path, installed));
            }
        }

        ServiceCommand::Start => action(path, ServiceAction::Start)?,
        ServiceCommand::Stop => action(path, ServiceAction::Stop)?,

        ServiceCommand::Restart => {
            if !path.is_file() {
                return Err(
                    "The service is not installed.\nInstall it with `krabb service install`."
                        .into(),
                );
            }

            action(path, ServiceAction::Restart)?;
        }
    }

    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ServiceState {
    Active,
    Inactive,
    #[cfg_attr(not(any(target_os = "linux", target_os = "windows")), allow(dead_code))]
    Starting,
    #[cfg_attr(not(any(target_os = "linux", target_os = "windows")), allow(dead_code))]
    Stopping,
    Failed,
    NotInstalled,
}

impl ServiceState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Inactive => "inactive",
            Self::Starting => "starting",
            Self::Stopping => "stopping",
            Self::Failed => "failed",
            Self::NotInstalled => "not installed",
        }
    }
}

pub(crate) fn service_status_text(
    name: &str,
    status: ServiceState,
    path: &Path,
    installed: bool,
) -> String {
    if installed {
        let state = match status {
            ServiceState::Active => "running",
            ServiceState::Inactive => "stopped",
            ServiceState::Starting => "starting",
            ServiceState::Stopping => "stopping",
            ServiceState::Failed => "in a failed state",
            ServiceState::NotInstalled => "not installed",
        };

        let definition = format_tree_list(&[format!("Definition: {}", path.display())]);

        let next = match status {
            ServiceState::Inactive => "\n\nStart it with `krabb service start`.",

            ServiceState::Failed => "\n\nRestart it with `krabb service restart`.",

            _ => "",
        };

        format!("The {name} service is {state}\n{definition}{next}")
    } else {
        format!("The {name} service is not installed.\nInstall it with `krabb service install`.")
    }
}

pub(crate) fn service_name() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        "Krabb"
    }

    #[cfg(target_os = "macos")]
    {
        "Krabb"
    }

    #[cfg(target_os = "windows")]
    {
        "Krabb"
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        "Krabb"
    }
}

#[cfg(target_os = "macos")]
pub(crate) const MACOS_SERVICE_LABEL: &str = "it.airscript.krabb";

pub(crate) fn daemon_executable() -> std::io::Result<PathBuf> {
    let current = std::env::current_exe()?;

    let name = if cfg!(windows) { "krabb-daemon.exe" } else { "krabb-daemon" };

    let sibling = current.parent().unwrap_or(Path::new(".")).join(name);

    if sibling.is_file() {
        return Ok(sibling);
    }

    #[cfg(test)]
    return Ok(current);

    #[cfg(not(test))]
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "The krabb-daemon executable was not found next to krabb.",
    ))
}

#[cfg(target_os = "windows")]
pub(crate) struct WindowsServiceSnapshot {
    pub(crate) bin_path: String,
    pub(crate) start: &'static str,
    pub(crate) environment: Option<String>,
}

#[cfg(target_os = "windows")]
pub(crate) fn windows_service_install(
    executable: &Path,
    environment: &[(String, String)],
    force: bool,
    path: &Path,
    definition: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let snapshot = if force { windows_service_snapshot()? } else { None };

    if snapshot.is_some() {
        let output = command_output(std::process::Command::new("powershell.exe").args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Stop-Service -Name 'Krabb' -Force -ErrorAction Stop",
        ]))?;

        if !output.status.success() {
            return Err(format!(
                "Could not stop the service before replacing its account. {}",
                windows_command_detail(&output)
            )
            .into());
        }
    }

    krabb_file::service(executable)?;

    for (name, value) in environment {
        for path in windows_service_private(name, Path::new(value)) {
            krabb_file::service(path)?;
        }
    }

    let executable = format!("\"{}\" --service", executable.display());

    let previous_definition = match std::fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };

    let mut created = false;

    let result = (|| {
        if snapshot.is_some() {
            windows_service_configure(&executable, "auto")?;
        } else {
            windows_service_create(&executable)?;
            created = true;
        }

        // Only an unrestricted service SID puts NT SERVICE\Krabb in the service token, and
        // the binaries and state grant access to that SID rather than to LocalService.
        let output = command_output(std::process::Command::new("sc.exe").args([
            "sidtype",
            "Krabb",
            "unrestricted",
        ]))?;

        if !output.status.success() {
            return Err(format!(
                "Could not give the service its own identity. {}",
                windows_command_detail(&output)
            )
            .into());
        }

        let output = command_output(std::process::Command::new("sc.exe").args([
            "privs",
            "Krabb",
            "SeChangeNotifyPrivilege",
        ]))?;

        if !output.status.success() {
            return Err(format!(
                "Could not restrict service privileges. {}",
                windows_command_detail(&output)
            )
            .into());
        }

        for (name, value) in environment {
            if let Some(directory) = windows_service_directory(name, Path::new(value)) {
                std::fs::create_dir_all(&directory)?;
                let output = command_output(&mut windows_service_access(&directory, true))?;

                if !output.status.success() {
                    return Err(format!(
                        "Could not grant service access to {name}. {}",
                        windows_command_detail(&output)
                    )
                    .into());
                }
            }

            if let Some(dependency) = windows_service_dependency(name, Path::new(value)) {
                let output = command_output(&mut windows_service_access(&dependency, false))?;

                if !output.status.success() {
                    return Err(format!(
                        "Could not grant service read/execute access to {name}. {}",
                        windows_command_detail(&output)
                    )
                    .into());
                }
            }
        }

        windows_service_recovery()?;

        let data = environment
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("\\0");

        let output = command_output(std::process::Command::new("reg.exe").args([
            "add",
            r"HKLM\SYSTEM\CurrentControlSet\Services\Krabb",
            "/v",
            "Environment",
            "/t",
            "REG_MULTI_SZ",
            "/d",
            data.as_str(),
            "/f",
        ]))?;

        if !output.status.success() {
            return Err(format!(
                "Could not install the service environment. {}",
                windows_command_detail(&output)
            )
            .into());
        }

        secure(path, definition.as_bytes())?;

        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    })();

    if let Err(error) = result {
        let rollback = windows_service_rollback(
            created,
            snapshot.as_ref(),
            path,
            previous_definition.as_deref(),
        );

        return match rollback {
            Ok(()) => Err(error),

            Err(rollback_error) => {
                Err(format!("{error}, and service rollback failed: {rollback_error}").into())
            }
        };
    }

    Ok(())
}

#[cfg(any(target_os = "windows", test))]
pub(crate) fn windows_service_private(name: &str, path: &Path) -> Vec<PathBuf> {
    match name {
        name if [
            "KRABB_HOME",
            "KRABB_CREDENTIALS",
            "KRABB_CREDENTIALS_KEY",
            "KRABB_TIMER",
            "KRABB_CODEX_HOME",
        ]
        .contains(&name) =>
        {
            vec![path.to_path_buf()]
        }

        "KRABB_MEMORY" => vec![path.to_path_buf(), path.with_extension("lock")],
        _ => Vec::new(),
    }
}

#[cfg(any(target_os = "windows", test))]
pub(crate) fn windows_service_directory(name: &str, path: &Path) -> Option<PathBuf> {
    match name {
        "KRABB_ROOT" | "KRABB_MEDIA" => Some(path.to_path_buf()),
        "KRABB_MEMORY" | "KRABB_DB" => Some(
            path.parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or(Path::new("."))
                .to_path_buf(),
        ),

        _ => None,
    }
}

#[cfg(any(target_os = "windows", test))]
pub(crate) fn windows_service_dependency(name: &str, path: &Path) -> Option<PathBuf> {
    match name {
        "KRABB_PLAYWRIGHT_MODULE" => path
            .ancestors()
            .find(|ancestor| ancestor.file_name().is_some_and(|name| name == "node_modules"))
            .or_else(|| path.parent())
            .map(|parent| if parent.as_os_str().is_empty() { Path::new(".") } else { parent })
            .map(Path::to_path_buf),

        "KRABB_PLAYWRIGHT_BROWSERS_PATH" => Some(path.to_path_buf()),

        "KRABB_NODE" | "KRABB_CODEX_BINARY" if path.components().count() > 1 || path.is_file() => {
            Some(path.to_path_buf())
        }

        _ => None,
    }
}

#[cfg(any(target_os = "windows", test))]
pub(crate) fn windows_service_access(path: &Path, writable: bool) -> std::process::Command {
    let mut command = std::process::Command::new("icacls.exe");

    let directory = writable || path.is_dir();

    let permissions = if writable {
        r"NT SERVICE\Krabb:(OI)(CI)M"
    } else if directory {
        r"NT SERVICE\Krabb:(OI)(CI)RX"
    } else {
        r"NT SERVICE\Krabb:RX"
    };

    command.arg(path).args(["/grant", permissions]);

    if directory {
        command.arg("/T");
    }

    command.arg("/L");

    command
}

#[cfg(any(target_os = "windows", test))]
pub(crate) fn windows_service_command(
    action: &str,
    executable: &str,
    start: &str,
) -> std::process::Command {
    let mut command = std::process::Command::new("sc.exe");
    let account = r"NT AUTHORITY\LocalService";

    command.args([
        action,
        "Krabb",
        "binPath=",
        executable,
        "start=",
        start,
        "obj=",
        account,
        "password=",
        "",
    ]);

    command
}

#[cfg(target_os = "windows")]
pub(crate) fn windows_service_create(
    executable: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let output = command_output(&mut windows_service_command("create", executable, "disabled"))?;

    if !output.status.success() {
        return Err(
            format!("Could not install the service. {}", windows_command_detail(&output)).into()
        );
    }

    if let Err(error) = windows_service_configure(executable, "auto") {
        let _ = command_output(std::process::Command::new("sc.exe").args(["delete", "Krabb"]));

        return Err(error);
    }

    Ok(())
}

#[cfg(target_os = "windows")]
pub(crate) fn windows_service_configure(
    executable: &str,
    start: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let output = command_output(&mut windows_service_command("config", executable, start))?;

    if !output.status.success() {
        return Err(format!(
            "Could not configure the service. {}",
            windows_command_detail(&output)
        )
        .into());
    }

    Ok(())
}

#[cfg(target_os = "windows")]
pub(crate) fn windows_service_recovery() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let output = command_output(std::process::Command::new("sc.exe").args([
        "failure",
        "Krabb",
        "reset=",
        "0",
        "actions=",
        "restart/5000/restart/5000/restart/5000",
    ]))?;

    if !output.status.success() {
        return Err(format!(
            "Could not configure service recovery. {}",
            windows_command_detail(&output)
        )
        .into());
    }

    Ok(())
}

#[cfg(target_os = "windows")]
pub(crate) fn windows_service_snapshot()
-> Result<Option<WindowsServiceSnapshot>, Box<dyn std::error::Error + Send + Sync>> {
    let output = command_output(std::process::Command::new("sc.exe").args(["qc", "Krabb"]))?;

    if !output.status.success() {
        let detail = windows_command_detail(&output);
        let lower = detail.to_ascii_lowercase();

        if detail.contains("1060") || lower.contains("does not exist") {
            return Ok(None);
        }

        return Err(format!("Could not query the service state. {detail}").into());
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let bin_path = windows_service_field(&text, "BINARY_PATH_NAME")
        .ok_or("Service query did not return a binary path.")?;
    let start_type = windows_service_field(&text, "START_TYPE")
        .ok_or("Service query did not return a start type.")?;
    let start = match start_type.split_whitespace().next() {
        Some("0") => "boot",
        Some("1") => "system",
        Some("2") => "auto",
        Some("3") => "demand",
        Some("4") => "disabled",
        _ => return Err("Service query returned an unsupported start type.".into()),
    };

    Ok(Some(WindowsServiceSnapshot {
        bin_path,
        start,
        environment: windows_service_environment()?,
    }))
}

#[cfg(target_os = "windows")]
pub(crate) fn windows_service_field(output: &str, name: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let (field, value) = line.split_once(':')?;

        (field.trim() == name).then(|| value.trim().to_owned())
    })
}

#[cfg(target_os = "windows")]
pub(crate) fn windows_service_environment()
-> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
    let output = command_output(std::process::Command::new("reg.exe").args([
        "query",
        r"HKLM\SYSTEM\CurrentControlSet\Services\Krabb",
        "/v",
        "Environment",
    ]))?;

    if !output.status.success() {
        let detail = windows_command_detail(&output);
        let lower = detail.to_ascii_lowercase();

        if lower.contains("unable to find") || lower.contains("not found") {
            return Ok(None);
        }

        return Err(format!("Could not inspect the service environment. {detail}").into());
    }

    let text = String::from_utf8_lossy(&output.stdout);
    Ok(windows_registry_field(&text, "Environment"))
}

#[cfg(target_os = "windows")]
pub(crate) fn windows_registry_field(output: &str, name: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let line = line.trim_start();
        let rest = line.strip_prefix(name)?;

        if !rest.chars().next().is_some_and(char::is_whitespace) {
            return None;
        }

        let rest = rest.trim_start();
        let type_end = rest.find(char::is_whitespace)?;
        let value = rest[type_end..].trim_start();

        Some(value.to_owned())
    })
}

#[cfg(target_os = "windows")]
pub(crate) fn windows_service_rollback(
    created: bool,
    snapshot: Option<&WindowsServiceSnapshot>,
    path: &Path,
    previous_definition: Option<&[u8]>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut errors = Vec::new();

    if let Some(snapshot) = snapshot {
        if let Err(error) = windows_service_configure(&snapshot.bin_path, snapshot.start) {
            errors.push(error.to_string());
        }

        let environment = match &snapshot.environment {
            Some(value) => command_output(std::process::Command::new("reg.exe").args([
                "add",
                r"HKLM\SYSTEM\CurrentControlSet\Services\Krabb",
                "/v",
                "Environment",
                "/t",
                "REG_MULTI_SZ",
                "/d",
                value,
                "/f",
            ])),
            None => command_output(std::process::Command::new("reg.exe").args([
                "delete",
                r"HKLM\SYSTEM\CurrentControlSet\Services\Krabb",
                "/v",
                "Environment",
                "/f",
            ])),
        };

        match environment {
            Ok(output) if output.status.success() => {}
            Ok(output) => errors.push(format!(
                "could not restore service environment: {}",
                windows_command_detail(&output)
            )),
            Err(error) => errors.push(format!("could not restore service environment: {error}")),
        }
    } else if created {
        if let Err(error) = windows_service_remove() {
            errors.push(error.to_string());
        }
    }

    match previous_definition {
        Some(bytes) => {
            if let Err(error) = secure(path, bytes) {
                errors.push(format!("could not restore service definition: {error}"));
            }
        }

        None => {
            if let Err(error) = std::fs::remove_file(path)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                errors.push(format!("could not restore service definition: {error}"));
            }
        }
    }

    if errors.is_empty() { Ok(()) } else { Err(errors.join("\n").into()) }
}

#[cfg(target_os = "windows")]
pub(crate) fn windows_command_detail(output: &std::process::Output) -> String {
    let detail = format!(
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let detail = detail.trim();

    if detail.is_empty() {
        "the service manager returned no details".into()
    } else {
        sentence(detail)
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn windows_service_remove() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let output = command_output(std::process::Command::new("sc.exe").args(["delete", "Krabb"]))?;

    if !output.status.success() {
        let detail = format!(
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        return Err(format!("Could not remove the service. {}", sentence(detail.trim())).into());
    }

    Ok(())
}

pub(crate) fn service_action(
    path: &Path,
    action: ServiceAction,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    service_action_with(path, action, ServiceReport::Text)
}

/// How a service action reports itself: as its own text or JSON result, or not at all when it is
/// one step of a command whose single JSON document reports the whole outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ServiceReport {
    Text,
    Json,
    Silent,
}

impl ServiceReport {
    /// The report for a step inside another command's output.
    pub(crate) fn nested(json: bool) -> Self {
        if json { Self::Silent } else { Self::Text }
    }
}

pub(crate) fn service_action_with(
    path: &Path,
    action: ServiceAction,
    report: ServiceReport,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if !path.is_file() {
        return Err(
            "The service is not installed.\nInstall it with `krabb service install`.".into()
        );
    }

    let previous = service_manager_state()?;

    if let Some(start) = service_action_start(action)
        && service_action_is_noop(start, previous)
    {
        return service_action_message(action, Some(previous), report);
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = action;
        return Err("Native service actions are unsupported on this platform.".into());
    }

    #[cfg(target_os = "linux")]
    let (program, args): (&str, Vec<String>) = match action {
        ServiceAction::Start => (
            "systemctl",
            vec!["--user".into(), "enable".into(), "--now".into(), "krabb.service".into()],
        ),
        ServiceAction::Stop => (
            "systemctl",
            vec!["--user".into(), "disable".into(), "--now".into(), "krabb.service".into()],
        ),

        ServiceAction::Restart => {
            ("systemctl", vec!["--user".into(), "restart".into(), "krabb.service".into()])
        }
    };

    #[cfg(target_os = "macos")]
    let (program, args): (&str, Vec<String>) = match action {
        ServiceAction::Start => {
            ("launchctl", vec!["load".into(), "-w".into(), path.display().to_string()])
        }

        ServiceAction::Stop => {
            ("launchctl", vec!["unload".into(), "-w".into(), path.display().to_string()])
        }

        ServiceAction::Restart => {
            let uid = command_output(std::process::Command::new("id").arg("-u"))?;

            if !uid.status.success() {
                return Err(
                    "Could not restart the service because the current user is unknown.".into()
                );
            }

            let domain = format!("gui/{}", String::from_utf8_lossy(&uid.stdout).trim());

            if previous == ServiceState::NotInstalled {
                ("launchctl", vec!["bootstrap".into(), domain, path.display().to_string()])
            } else {
                (
                    "launchctl",
                    vec![
                        "kickstart".into(),
                        "-k".into(),
                        format!("{domain}/{MACOS_SERVICE_LABEL}"),
                    ],
                )
            }
        }
    };

    #[cfg(target_os = "windows")]
    let (program, args): (&str, Vec<String>) = match action {
        ServiceAction::Start => ("sc.exe", vec!["start".into(), "Krabb".into()]),
        ServiceAction::Stop => ("sc.exe", vec!["stop".into(), "Krabb".into()]),
        ServiceAction::Restart
            if matches!(
                previous,
                ServiceState::Active | ServiceState::Starting | ServiceState::Stopping
            ) =>
        {
            (
                "powershell.exe",
                vec![
                    "-NoProfile".into(),
                    "-NonInteractive".into(),
                    "-Command".into(),
                    "Restart-Service -Name 'Krabb' -Force -ErrorAction Stop".into(),
                ],
            )
        }

        ServiceAction::Restart => ("sc.exe", vec!["start".into(), "Krabb".into()]),
    };

    let message = match action {
        ServiceAction::Start => "Starting the Krabb service...",
        ServiceAction::Stop => "Stopping the Krabb service...",
        ServiceAction::Restart => "Restarting the Krabb service...",
    };

    let spinner =
        if report == ServiceReport::Text { terminal::Spinner::start(message) } else { None };

    let output = command_output(std::process::Command::new(program).args(args))?;
    drop(spinner);

    if !output.status.success() {
        let detail = format!(
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let detail = detail.trim();
        let detail =
            if detail.is_empty() { "the service manager returned no details" } else { detail };

        let action = match action {
            ServiceAction::Start => "start",
            ServiceAction::Stop => "stop",
            ServiceAction::Restart => "restart",
        };

        return Err(format!("Could not {action} the service: {}", sentence(detail)).into());
    }

    service_action_message(action, None, report)
}

pub(crate) fn service_action_message(
    action: ServiceAction,
    previous: Option<ServiceState>,
    report: ServiceReport,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let action_name = match action {
        ServiceAction::Start => "start",
        ServiceAction::Stop => "stop",
        ServiceAction::Restart => "restart",
    };

    let status = match (action, previous) {
        (ServiceAction::Start, Some(_)) => "already active",
        (ServiceAction::Stop, Some(ServiceState::Failed)) => "failed",
        (ServiceAction::Stop, Some(ServiceState::NotInstalled)) => "not installed",
        (ServiceAction::Stop, Some(_)) => "already inactive",
        (ServiceAction::Start, None) => "started",
        (ServiceAction::Stop, None) => "stopped",
        (ServiceAction::Restart, _) => "restarted",
    };

    if previous.is_some() {
        debug!(action = action_name, status, "Service action required no transition.");
    } else {
        info!(action = action_name, "Service action completed.");
    }

    if report == ServiceReport::Json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "action": action_name,
                "name": service_name(),
                "status": status,
            }))?
        );
    } else if report == ServiceReport::Text {
        println!("{}", service_action_text(action, previous));
    }

    Ok(())
}

pub(crate) fn service_action_text(
    action: ServiceAction,
    previous: Option<ServiceState>,
) -> &'static str {
    match (action, previous) {
        (ServiceAction::Start, Some(ServiceState::Active)) => "The service is already running.",
        (ServiceAction::Start, Some(ServiceState::Starting)) => "The service is already starting.",
        (ServiceAction::Start, Some(ServiceState::Stopping)) => "The service is stopping.",

        (ServiceAction::Start, Some(ServiceState::Inactive | ServiceState::Failed)) => {
            "The service is not running."
        }

        (ServiceAction::Start, Some(ServiceState::NotInstalled)) => "The service is not installed.",

        (ServiceAction::Stop, Some(ServiceState::Failed)) => {
            "The service has failed, so there is nothing to stop."
        }

        (ServiceAction::Stop, Some(ServiceState::NotInstalled)) => "The service is not installed.",
        (ServiceAction::Stop, Some(ServiceState::Starting)) => "The service is starting.",
        (ServiceAction::Stop, Some(ServiceState::Stopping)) => "The service is already stopping.",
        (ServiceAction::Stop, Some(ServiceState::Active)) => "The service is running.",
        (ServiceAction::Stop, Some(_)) => "The service is already stopped.",
        (ServiceAction::Start, None) => "The service has been started.",
        (ServiceAction::Stop, None) => "The service has been stopped.",
        (ServiceAction::Restart, _) => "The service has been restarted.",
    }
}

pub(crate) fn service_action_start(action: ServiceAction) -> Option<bool> {
    match action {
        ServiceAction::Start => Some(true),
        ServiceAction::Stop => Some(false),
        ServiceAction::Restart => None,
    }
}

pub(crate) fn service_action_is_noop(start: bool, state: ServiceState) -> bool {
    (start && state == ServiceState::Active)
        || (!start
            && matches!(
                state,
                ServiceState::Inactive | ServiceState::Failed | ServiceState::NotInstalled
            ))
}

pub(crate) fn service_manager_state()
-> Result<ServiceState, Box<dyn std::error::Error + Send + Sync>> {
    #[cfg(target_os = "linux")]
    {
        let output = command_output(std::process::Command::new("systemctl").args([
            "--user",
            "is-active",
            "krabb.service",
        ]))?;

        linux_service_state(
            output.status.success(),
            &String::from_utf8_lossy(&output.stdout),
            &String::from_utf8_lossy(&output.stderr),
        )
    }

    #[cfg(target_os = "macos")]
    {
        let uid = std::process::Command::new("id").arg("-u").output()?;

        if !uid.status.success() {
            return Err(
                "Could not read the service status because the current user is unknown.".into()
            );
        }

        let target =
            format!("gui/{}/{MACOS_SERVICE_LABEL}", String::from_utf8_lossy(&uid.stdout).trim());
        let output =
            command_output(std::process::Command::new("launchctl").args(["print", &target]))?;

        if !output.status.success() {
            let detail = format!(
                "{} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );

            if detail.contains("Could not find service") || detail.contains("No such process") {
                return Ok(ServiceState::NotInstalled);
            }

            return Err(
                format!("Could not read the service status. {}", sentence(detail.trim())).into()
            );
        }

        let text = String::from_utf8_lossy(&output.stdout);

        return Ok(if text.lines().any(|line| line.trim() == "state = running") {
            ServiceState::Active
        } else {
            ServiceState::Inactive
        });
    }

    #[cfg(target_os = "windows")]
    {
        let output = command_output(std::process::Command::new("sc.exe").args(["query", "Krabb"]))?;

        if !output.status.success() {
            let detail = format!(
                "{} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );

            let lower = detail.to_ascii_lowercase();

            if detail.contains("1060") || lower.contains("does not exist") {
                return Ok(ServiceState::NotInstalled);
            }

            return Err(
                format!("Could not query the service state. {}", sentence(detail.trim())).into()
            );
        }

        let state = String::from_utf8_lossy(&output.stdout);

        return Ok(if state.contains("RUNNING") {
            ServiceState::Active
        } else if state.contains("START_PENDING") {
            ServiceState::Starting
        } else if state.contains("STOP_PENDING") {
            ServiceState::Stopping
        } else {
            ServiceState::Inactive
        });
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Err("Native service status is unsupported on this platform.".into())
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn linux_service_state(
    success: bool,
    stdout: &str,
    stderr: &str,
) -> Result<ServiceState, Box<dyn std::error::Error + Send + Sync>> {
    let value = stdout.trim().to_ascii_lowercase();

    match value.as_str() {
        "active" => Ok(ServiceState::Active),
        "activating" => Ok(ServiceState::Starting),
        "deactivating" => Ok(ServiceState::Stopping),
        "failed" => Ok(ServiceState::Failed),
        "inactive" | "unknown" | "not-found" => Ok(ServiceState::Inactive),

        _ if !success && stderr.trim().is_empty() => {
            Err("Could not read the service status because systemd returned no details.".into())
        }

        _ => Err(format!("Could not read the service status. {}", sentence(stderr.trim())).into()),
    }
}

pub(crate) fn service_path() -> PathBuf {
    let user = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    #[cfg(target_os = "linux")]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| user.join(".config"))
            .join("systemd/user/krabb.service")
    }

    #[cfg(target_os = "macos")]
    {
        user.join("Library/LaunchAgents").join(format!("{MACOS_SERVICE_LABEL}.plist"))
    }

    #[cfg(target_os = "windows")]
    {
        user.join("krabb-service.xml")
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        user.join("krabb.service")
    }
}

pub(crate) fn service_text(executable: &Path, environment: &[(String, String)]) -> String {
    let executable = executable.display().to_string();

    #[cfg(target_os = "linux")]
    {
        let environment = environment
            .iter()
            .map(|(name, value)| format!("Environment=\"{name}={}\"\n", systemd(value)))
            .collect::<String>();

        format!(
            "[Unit]\nDescription=Krabb agent\nAfter=network-online.target\nStartLimitIntervalSec=0\n\n[Service]\nExecStart=\"{}\"\n{}Restart=always\nRestartSec=5s\n\n[Install]\nWantedBy=default.target\n",
            executable.replace('\\', "\\\\").replace('"', "\\\""),
            environment
        )
    }

    #[cfg(target_os = "macos")]
    {
        let environment = environment
            .iter()
            .map(|(name, value)| {
                format!("<key>{}</key><string>{}</string>", xml(name.into()), xml(value.clone()))
            })
            .collect::<String>();

        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Label</key><string>{}</string><key>ProgramArguments</key><array><string>{}</string></array><key>EnvironmentVariables</key><dict>{}</dict><key>RunAtLoad</key><true/><key>KeepAlive</key><true/></dict></plist>\n",
            MACOS_SERVICE_LABEL,
            xml(executable),
            environment
        )
    }

    #[cfg(target_os = "windows")]
    {
        let environment = environment
            .iter()
            .map(|(name, value)| {
                format!(
                    "<variable name=\"{}\" value=\"{}\"/>",
                    xml(name.into()),
                    xml(value.clone())
                )
            })
            .collect::<String>();

        format!(
            "<service><name>Krabb</name><command>{}</command><arguments>--service</arguments><account>NT SERVICE\\Krabb</account><environment>{}</environment><start>auto</start></service>\n",
            xml(executable),
            environment
        )
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        format!("{}\n", executable)
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn systemd(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
        .replace('%', "%%")
        .replace('\n', "\\n")
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) fn xml(value: String) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
