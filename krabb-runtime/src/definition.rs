//! Keeps an installed service definition in the current release's format.
//!
//! A definition is rewritten only when its layout, with the executable and environment left
//! blank, is one a Krabb release wrote and every value decodes exactly. Anything else counts as
//! edited by hand and is left alone. A refresh keeps the installed executable and environment.

use super::*;

/// Layouts earlier releases wrote, with the executable and environment left blank. Add the
/// outgoing layout here whenever `service_text` changes.
const RETIRED: &[&str] = &[];

#[derive(Debug, PartialEq)]
struct Installed {
    executable: PathBuf,
    environment: Vec<(String, String)>,
    layout: String,
}

/// The definition this release would write for the installed service, when it differs.
pub(crate) fn stale(path: &Path) -> Option<String> {
    stale_among(path, RETIRED)
}

fn stale_among(path: &Path, retired: &[&str]) -> Option<String> {
    let metadata = std::fs::symlink_metadata(path).ok()?;

    if !metadata.is_file() {
        return None;
    }

    let current = std::fs::read_to_string(path).ok()?;
    let installed = parse(&current)?;
    let layout = parse(&service_text(Path::new(""), &[]))?.layout;

    if installed.layout != layout && !retired.contains(&installed.layout.as_str()) {
        return None;
    }

    let expected = service_text(&installed.executable, &installed.environment);

    (expected != current).then_some(expected)
}

/// Rewrites a stale definition and makes the service manager read it. Returns whether it did.
pub(crate) fn refresh(path: &Path) -> Result<bool, PluginError> {
    let Some(expected) = stale(path) else {
        return Ok(false);
    };

    #[cfg(target_os = "windows")]
    install(path, &expected)?;

    #[cfg(not(target_os = "windows"))]
    {
        secure(path, expected.as_bytes())?;
        reload(path)?;
    }

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
    install(path, &std::fs::read_to_string(path)?)?;

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    let _ = path;

    Ok(())
}

/// Windows keeps the definition in the service registry, so installing writes and reloads it.
#[cfg(target_os = "windows")]
fn install(path: &Path, definition: &str) -> Result<(), PluginError> {
    let installed = parse(definition).ok_or("The service definition could not be read.")?;

    windows_service_install(&installed.executable, &installed.environment, true, path, definition)?;

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
    let mut layout = String::new();
    let mut after = false;

    for line in definition.split_inclusive('\n') {
        let text = line.strip_suffix('\n').unwrap_or(line);

        if let Some(value) = text.strip_prefix("ExecStart=") {
            if executable.is_some() {
                return None;
            }

            executable = Some(PathBuf::from(unescape(quoted(value)?, false)?));
            layout.push_str("ExecStart=\n");
            after = true;
        } else if let Some(value) = text.strip_prefix("Environment=") {
            if !after {
                return None;
            }

            let assignment = unescape(quoted(value)?, true)?;
            let (name, value) = assignment.split_once('=')?;

            environment.push(variable(name, value)?);
        } else {
            layout.push_str(line);
            after = false;
        }
    }

    Some(Installed { executable: executable?, environment, layout })
}

fn quoted(value: &str) -> Option<&str> {
    value.strip_prefix('"')?.strip_suffix('"')
}

/// Reverses exactly the systemd quoting Krabb applies, refusing anything it never writes.
fn unescape(value: &str, environment: bool) -> Option<String> {
    let mut output = String::with_capacity(value.len());
    let mut characters = value.chars();

    while let Some(character) = characters.next() {
        let decoded = match character {
            '\\' => match characters.next()? {
                '\\' => '\\',
                '"' => '"',
                '$' if environment => '$',
                'n' if environment => '\n',
                _ => return None,
            },

            '%' if environment => match characters.next()? {
                '%' => '%',
                _ => return None,
            },

            '"' => return None,
            '$' if environment => return None,
            character => character,
        };

        output.push(decoded);
    }

    Some(output)
}

/// Reads the program and environment from the launchd property list Krabb writes.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn launchd(definition: &str) -> Option<Installed> {
    let (program, rest) =
        blank(definition, "<key>ProgramArguments</key><array><string>", "</string>")?;

    let (inner, layout) = blank(&rest, "<key>EnvironmentVariables</key><dict>", "</dict>")?;
    let mut environment = Vec::new();
    let mut variables = inner.as_str();

    while !variables.is_empty() {
        let (name, after) = variables.strip_prefix("<key>")?.split_once("</key>")?;
        let (value, after) = after.strip_prefix("<string>")?.split_once("</string>")?;

        environment.push(variable(&entities(name)?, &entities(value)?)?);
        variables = after;
    }

    Some(Installed { executable: PathBuf::from(entities(&program)?), environment, layout })
}

/// Reads the command and variables from the Windows service record Krabb writes.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn windows(definition: &str) -> Option<Installed> {
    let (command, rest) = blank(definition, "<command>", "</command>")?;
    let (inner, layout) = blank(&rest, "<environment>", "</environment>")?;
    let mut environment = Vec::new();
    let mut variables = inner.as_str();

    while !variables.is_empty() {
        let (name, after) = variables.strip_prefix("<variable name=\"")?.split_once('"')?;
        let (value, after) = after.strip_prefix(" value=\"")?.split_once("\"/>")?;

        environment.push(variable(&entities(name)?, &entities(value)?)?);
        variables = after;
    }

    Some(Installed { executable: PathBuf::from(entities(&command)?), environment, layout })
}

/// Splits out the text between two markers, returning it and the text without it.
fn blank(text: &str, start: &str, end: &str) -> Option<(String, String)> {
    let (before, after) = text.split_once(start)?;
    let (inner, after) = after.split_once(end)?;

    Some((inner.to_owned(), format!("{before}{start}{end}{after}")))
}

/// Keeps only variable names a shell could set, so a refresh never writes a new directive.
fn variable(name: &str, value: &str) -> Option<(String, String)> {
    let mut characters = name.chars();
    let first = characters.next()?;
    let valid = (first.is_ascii_alphabetic() || first == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_');

    valid.then(|| (name.to_owned(), value.to_owned()))
}

/// Reverses exactly the XML escaping Krabb applies, refusing anything it never writes.
fn entities(value: &str) -> Option<String> {
    let mut output = String::with_capacity(value.len());
    let mut rest = value;

    while let Some(character) = rest.chars().next() {
        if character == '&' {
            let (entity, decoded) =
                [("&amp;", '&'), ("&lt;", '<'), ("&gt;", '>'), ("&quot;", '"'), ("&apos;", '\'')]
                    .into_iter()
                    .find(|(entity, _)| rest.starts_with(entity))?;

            output.push(decoded);
            rest = &rest[entity.len()..];

            continue;
        }

        if matches!(character, '<' | '>' | '"' | '\'') {
            return None;
        }

        output.push(character);
        rest = &rest[character.len_utf8()..];
    }

    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed(definition: Option<Installed>) -> Option<(PathBuf, Vec<(String, String)>)> {
        definition.map(|installed| (installed.executable, installed.environment))
    }

    #[test]
    fn reads_every_service_format_krabb_writes() {
        let unit = "[Service]\nExecStart=\"/opt/krabb \\\"daemon\\\" 50%\"\nEnvironment=\"KRABB_HOME=/home/a b\"\nEnvironment=\"PATTERN=50%% \\$HOME\\nnext\"\nRestart=always\n";

        assert_eq!(
            installed(systemd_unit(unit)),
            Some((
                PathBuf::from("/opt/krabb \"daemon\" 50%"),
                vec![
                    ("KRABB_HOME".into(), "/home/a b".into()),
                    ("PATTERN".into(), "50% $HOME\nnext".into()),
                ]
            ))
        );

        assert_eq!(systemd_unit(unit).unwrap().layout, "[Service]\nExecStart=\nRestart=always\n");

        let plist = "<plist><dict><key>ProgramArguments</key><array><string>/opt/k&amp;rabb</string></array><key>EnvironmentVariables</key><dict><key>KRABB_HOME</key><string>/Users/a &lt;b&gt;</string><key>EMPTY</key><string></string></dict></dict></plist>";

        assert_eq!(
            installed(launchd(plist)),
            Some((
                PathBuf::from("/opt/k&rabb"),
                vec![("KRABB_HOME".into(), "/Users/a <b>".into()), ("EMPTY".into(), String::new()),]
            ))
        );

        let record = "<service><command>C:\\Krabb\\krabb-daemon.exe</command><environment><variable name=\"KRABB_HOME\" value=\"C:\\a &quot;b&quot;\"/></environment></service>";

        assert_eq!(
            installed(windows(record)),
            Some((
                PathBuf::from("C:\\Krabb\\krabb-daemon.exe"),
                vec![("KRABB_HOME".into(), "C:\\a \"b\"".into())]
            ))
        );

        assert_eq!(
            windows(record).unwrap().layout,
            "<service><command></command><environment></environment></service>"
        );
    }

    #[test]
    fn refuses_definitions_krabb_did_not_write() {
        for unit in [
            "[Service]\nRestart=always\n",
            "ExecStart=\"/a\" \"--verbose\"\n",
            "ExecStart=\"/a\"\nExecStart=\"/b\"\n",
            "ExecStart=\"/a\"\nEnvironment=\"A=1\" \"B=2\"\n",
            "ExecStart=\"/a\"\nEnvironment=\"X\\nExecStartPre=/tmp/evil\\n=1\"\n",
            "ExecStart=\"/a\"\nEnvironment=\"A=\\t\"\n",
            "ExecStart=\"/a\"\nEnvironment=\"A=50%\"\n",
            "ExecStart=\"/a\"\nEnvironment=\"A=$HOME\"\n",
            "ExecStart=\"/a\\$\"\n",
            "Environment=\"A=1\"\nExecStart=\"/a\"\n",
            "ExecStart=\"/a\"\nRestart=always\nEnvironment=\"A=1\"\n",
        ] {
            assert!(systemd_unit(unit).is_none(), "{unit}");
        }

        assert!(launchd("<plist/>").is_none());
        assert!(windows("<service/>").is_none());

        for value in ["a & b", "a &copy;", "a \"b\"", "a 'b'"] {
            assert!(entities(value).is_none(), "{value}");
        }

        let extra = "<dict><key>ProgramArguments</key><array><string>/a</string></array><key>EnvironmentVariables</key><dict><key>A</key><string>1</string><key>B</key></dict></dict>";

        assert!(launchd(extra).is_none());
        assert!(variable("1A", "").is_none());
        assert!(variable("", "").is_none());
        assert_eq!(variable("_A1", "x"), Some(("_A1".into(), "x".into())));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn refreshes_only_known_layouts_and_keeps_their_environment() {
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
        let retired = systemd_unit(&older).unwrap().layout;

        std::fs::write(&path, &older).unwrap();

        assert_eq!(stale(&path), None);
        assert_eq!(stale_among(&path, &[retired.as_str()]), Some(current.clone()));

        let edited = current.replace("Restart=always\n", "Restart=always\nNice=10\n");

        std::fs::write(&path, &edited).unwrap();

        assert_eq!(stale_among(&path, &[retired.as_str()]), None);
        assert_eq!(stale(&root.join("missing.service")), None);

        let _ = std::fs::remove_dir_all(root);
    }
}
