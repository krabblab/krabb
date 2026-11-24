//! Keeps an installed service definition in the current release's format.
//!
//! A refresh keeps the installed executable and environment and only changes the template, so
//! variables saved at install time are never dropped.

use super::*;

type Installed = (PathBuf, Vec<(String, String)>);

/// The definition this release would write for the installed service, when it differs.
pub(crate) fn stale(path: &Path) -> Option<String> {
    let metadata = std::fs::symlink_metadata(path).ok()?;

    if !metadata.is_file() {
        return None;
    }

    let current = std::fs::read_to_string(path).ok()?;
    let (executable, environment) = parse(&current)?;
    let expected = service_text(&executable, &environment);

    (expected != current).then_some(expected)
}

/// Rewrites a stale definition and makes the service manager read it. Returns whether it did.
pub(crate) fn refresh(path: &Path) -> Result<bool, PluginError> {
    let Some(expected) = stale(path) else {
        return Ok(false);
    };

    write(path, &expected)?;
    reload(path)?;

    Ok(true)
}

/// Makes the service manager read the definition file again, such as after a rollback.
pub(crate) fn reload(path: &Path) -> Result<(), PluginError> {
    #[cfg(target_os = "linux")]
    {
        let _ = path;
        let output = command_output(
            std::process::Command::new("systemctl").args(["--user", "daemon-reload"]),
        )?;

        if !output.status.success() {
            return Err("The service manager could not reload the service definition.".into());
        }
    }

    #[cfg(target_os = "windows")]
    {
        let definition = std::fs::read_to_string(path)?;
        let (executable, environment) =
            parse(&definition).ok_or("The service definition could not be read.")?;

        windows_service_install(&executable, &environment, true, path, &definition)?;
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    let _ = path;

    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn write(path: &Path, definition: &str) -> Result<(), PluginError> {
    secure(path, definition.as_bytes())?;

    Ok(())
}

#[cfg(target_os = "windows")]
fn write(path: &Path, definition: &str) -> Result<(), PluginError> {
    let (executable, environment) =
        parse(definition).ok_or("The service definition could not be read.")?;

    windows_service_install(&executable, &environment, true, path, definition)?;

    Ok(())
}

fn parse(definition: &str) -> Option<Installed> {
    #[cfg(target_os = "linux")]
    return systemd_unit(definition);

    #[cfg(target_os = "macos")]
    return launchd(definition);

    #[cfg(target_os = "windows")]
    return windows(definition);

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = definition;
        None
    }
}

/// Reads the `ExecStart` and `Environment` lines Krabb writes for systemd.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn systemd_unit(definition: &str) -> Option<Installed> {
    let mut executable = None;
    let mut environment = Vec::new();

    for line in definition.lines() {
        if let Some(value) = line.strip_prefix("ExecStart=") {
            executable = Some(PathBuf::from(unescape(quoted(value)?)));
        } else if let Some(value) = line.strip_prefix("Environment=") {
            let (name, value) = unescape(quoted(value)?)
                .split_once('=')
                .map(|(name, value)| (name.to_owned(), value.to_owned()))?;

            environment.push((name, value));
        }
    }

    Some((executable?, environment))
}

fn quoted(value: &str) -> Option<&str> {
    value.strip_prefix('"')?.strip_suffix('"')
}

/// Reverses the systemd quoting Krabb applies: backslash escapes and doubled `%`.
fn unescape(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut characters = value.chars().peekable();

    while let Some(character) = characters.next() {
        match character {
            '\\' => match characters.next() {
                Some('n') => output.push('\n'),
                Some(next) => output.push(next),
                None => output.push('\\'),
            },

            '%' if characters.peek() == Some(&'%') => {
                characters.next();
                output.push('%');
            }

            character => output.push(character),
        }
    }

    output
}

/// Reads the program and environment from the launchd property list Krabb writes.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn launchd(definition: &str) -> Option<Installed> {
    let program = between(definition, "<key>ProgramArguments</key><array><string>", "</string>")?;
    let variables = between(definition, "<key>EnvironmentVariables</key><dict>", "</dict>")?;
    let mut environment = Vec::new();
    let mut rest = variables;

    while let Some(start) = rest.find("<key>") {
        let after = &rest[start + "<key>".len()..];
        let name = after.split("</key>").next()?;
        let value = between(after, "<string>", "</string>")?;

        environment.push((entities(name), entities(value)));
        rest = &after[after.find("</string>")? + "</string>".len()..];
    }

    Some((PathBuf::from(entities(program)), environment))
}

/// Reads the command and variables from the Windows service record Krabb writes.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn windows(definition: &str) -> Option<Installed> {
    let command = between(definition, "<command>", "</command>")?;
    let mut environment = Vec::new();
    let mut rest = definition;

    while let Some(start) = rest.find("<variable name=\"") {
        let after = &rest[start + "<variable name=\"".len()..];
        let name = after.split('"').next()?;
        let value = between(after, "value=\"", "\"/>")?;

        environment.push((entities(name), entities(value)));
        rest = &after[after.find("\"/>")? + 3..];
    }

    Some((PathBuf::from(entities(command)), environment))
}

fn between<'a>(text: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let after = &text[text.find(start)? + start.len()..];

    Some(&after[..after.find(end)?])
}

/// Reverses the XML escaping Krabb applies, with `&amp;` last so escapes are not doubled.
fn entities(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_every_service_format_krabb_writes() {
        let unit = "[Service]\nExecStart=\"/opt/krabb \\\"daemon\\\"\"\nEnvironment=\"KRABB_HOME=/home/a b\"\nEnvironment=\"PATTERN=50%% \\$HOME\\nnext\"\nRestart=always\n";

        assert_eq!(
            systemd_unit(unit),
            Some((
                PathBuf::from("/opt/krabb \"daemon\""),
                vec![
                    ("KRABB_HOME".into(), "/home/a b".into()),
                    ("PATTERN".into(), "50% $HOME\nnext".into()),
                ]
            ))
        );

        let plist = "<plist><dict><key>ProgramArguments</key><array><string>/opt/k&amp;rabb</string></array><key>EnvironmentVariables</key><dict><key>KRABB_HOME</key><string>/Users/a &lt;b&gt;</string><key>EMPTY</key><string></string></dict></dict></plist>";

        assert_eq!(
            launchd(plist),
            Some((
                PathBuf::from("/opt/k&rabb"),
                vec![("KRABB_HOME".into(), "/Users/a <b>".into()), ("EMPTY".into(), String::new()),]
            ))
        );

        let record = "<service><command>C:\\Krabb\\krabb-daemon.exe</command><environment><variable name=\"KRABB_HOME\" value=\"C:\\a &quot;b&quot;\"/></environment></service>";

        assert_eq!(
            windows(record),
            Some((
                PathBuf::from("C:\\Krabb\\krabb-daemon.exe"),
                vec![("KRABB_HOME".into(), "C:\\a \"b\"".into())]
            ))
        );

        assert!(systemd_unit("[Service]\nRestart=always\n").is_none());
        assert!(launchd("<plist/>").is_none());
        assert!(windows("<service/>").is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn refreshes_only_outdated_definitions_and_keeps_their_environment() {
        let root = crate::tests::test_root("service-definition");
        let path = root.join("krabb.service");
        let environment = vec![
            ("KRABB_HOME".to_owned(), "/home/owner/.krabb".to_owned()),
            ("KRABB_CREDENTIALS".to_owned(), "/home/owner/krabb.credentials.json".to_owned()),
        ];

        let current = service_text(Path::new("/usr/local/bin/krabb-daemon"), &environment);

        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&path, &current).unwrap();

        assert_eq!(stale(&path), None);

        let older = current.replace("Restart=always\n", "");

        std::fs::write(&path, &older).unwrap();

        assert_eq!(stale(&path), Some(current));

        std::fs::write(&path, "[Service]\nExecStart=/bin/custom\n").unwrap();

        assert_eq!(stale(&path), None);
        assert_eq!(stale(&root.join("missing.service")), None);

        let _ = std::fs::remove_dir_all(root);
    }
}
