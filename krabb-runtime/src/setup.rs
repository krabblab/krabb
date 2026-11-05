use std::{
    collections::BTreeMap,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
};

use clap::{Args, Parser, Subcommand};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use serde_json::{Value, json};

type Error = Box<dyn std::error::Error + Send + Sync>;
const LIMIT: usize = 16 * 1024;

#[derive(Debug, Args)]
pub(super) struct Options {
    /// Installed plugin whose declared secrets should be configured.
    id: String,
    /// Save credentials without contacting the running daemon.
    #[arg(long)]
    no_activate: bool,
}

#[derive(Parser)]
#[command(name = "krabb <plugin>", about = "Set up plugin credentials.")]
struct Setup {
    #[arg(long, global = true)]
    json: bool,
    #[arg(long, global = true, hide = true)]
    debug: bool,
    #[arg(long, global = true, hide = true)]
    verbose: bool,
    #[command(subcommand)]
    command: SetupCommand,
}

#[derive(Subcommand)]
enum SetupCommand {
    /// Securely prompt for declared secrets without starting the plugin first.
    Setup {
        #[arg(long)]
        no_activate: bool,
    },
}

pub(super) fn requested(args: &[String]) -> bool {
    args.iter()
        .find(|argument| !matches!(argument.as_str(), "--json" | "--debug" | "--verbose"))
        .is_some_and(|argument| argument == "setup")
}

pub(super) async fn command(
    args: &[String],
    id: &str,
    root: &Path,
    json: bool,
) -> Result<(), Error> {
    let cli =
        match Setup::try_parse_from(std::iter::once(format!("krabb {id}")).chain(args.to_vec())) {
            Ok(cli) => cli,
            Err(error) if error.use_stderr() => return Err(error.into()),

            Err(error) => {
                print!("{error}");
                return Ok(());
            }
        };

    let SetupCommand::Setup { no_activate } = cli.command;

    run(Options { id: id.into(), no_activate }, root, json || cli.json).await
}

pub(super) fn path(root: &Path) -> PathBuf {
    super::credentials::path(root)
}

fn names(root: &Path, id: &str) -> Result<Vec<String>, Error> {
    if !super::valid(id) {
        return Err("Invalid plugin ID.".into());
    }

    let lock = super::load_lock_at(root)?;
    let entry = lock.plugins.get(id).ok_or("Install the plugin before running setup.")?;
    let manifest = super::read_manifest_checked(&root.join("plugins").join(id))?;

    if manifest.id != id || manifest.variables != entry.variables {
        return Err("Plugin secrets changed; review and reinstall the plugin before setup.".into());
    }

    if entry.variables.is_empty() {
        return Err("This plugin declares no secrets; use its own authentication flow.".into());
    }

    Ok(entry.variables.clone())
}

pub(super) async fn run(options: Options, root: &Path, json: bool) -> Result<(), Error> {
    let names = names(root, &options.id)?;
    let destination = path(root);
    let key = super::credentials::key_path(root);
    let preference = super::credentials::preference()?;

    // Validate the destination before asking for sensitive input; re-read it under the save lock.
    super::credentials::read(&destination, &key)?;

    let values = collect(&names, prompt)?;
    let backend = super::credentials::save(&destination, &key, values, preference)?;
    let destination = destination.canonicalize()?;
    let mut result = finish(root, &options.id, &destination, options.no_activate).await;
    result["storage"] = backend.into();

    if backend == "file" {
        result["key_path"] = key.canonicalize()?.display().to_string().into();
    }

    if json {
        println!("{}", serde_json::to_string(&result)?);
    } else {
        println!(
            "Saved encrypted credentials for {} in {} (key: {backend}).",
            options.id,
            destination.display()
        );
        println!("{}", result["message"].as_str().unwrap_or_default());

        if let Some(path) = result["key_path"].as_str() {
            println!("Encryption key file: {path}");
        }
    }

    Ok(())
}

async fn finish(root: &Path, id: &str, destination: &Path, offline: bool) -> Value {
    let (active, message) = if offline {
        (false, "Activation skipped. Start or restart the runtime when ready.".to_owned())
    } else {
        match super::control_at(
            "plugin.setup",
            json!({"id": id, "credentials": destination}),
            root,
        )
        .await
        {
            Ok(Some(result)) if result["loaded"] == true => {
                (true, "Plugin loaded in the running daemon.".to_owned())
            }

            Ok(None) => (false, "Start the runtime to activate the plugin.".to_owned()),
            // Plugin errors can contain provider responses. Never include them in setup output.
            _ => (false, concat!(
                "Credentials saved; activation is pending. Check that the daemon uses the same ",
                "credential file, has no overriding secret environment variables, and selects ",
                "this channel/model. Restart an older daemon, then run krabb doctor."
            ).to_owned()),
        }
    };

    json!({"id": id, "saved": true, "active": active, "path": destination, "message": message})
}

fn collect(
    names: &[String],
    mut prompt: impl FnMut(&str) -> Result<String, Error>,
) -> Result<BTreeMap<String, String>, Error> {
    let mut values = BTreeMap::new();

    for name in names {
        let value = prompt(name)?;

        if value.trim().is_empty() || value.len() > LIMIT || value.chars().any(char::is_control) {
            return Err("Secret must be nonempty, single-line, and at most 16 KiB.".into());
        }

        values.insert(name.clone(), value);
    }

    Ok(values)
}

struct Terminal;

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = crossterm::execute!(io::stderr(), crossterm::event::DisableBracketedPaste);
        let _ = crossterm::terminal::disable_raw_mode();

        eprintln!();
    }
}

fn prompt(name: &str) -> Result<String, Error> {
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return Err("Setup requires an interactive terminal; secret input cannot be piped.".into());
    }

    crossterm::terminal::enable_raw_mode()?;

    let _terminal = Terminal;

    crossterm::execute!(io::stderr(), crossterm::event::EnableBracketedPaste)?;
    eprint!("{name}: ");
    io::stderr().flush()?;

    input(crossterm::event::read)
}

fn input(mut next: impl FnMut() -> io::Result<Event>) -> Result<String, Error> {
    let mut value = String::new();

    loop {
        let event = next()?;

        if let Event::Paste(text) = event {
            if text.chars().any(char::is_control) || value.len() + text.len() > LIMIT {
                return Err("Pasted secret must be single-line and at most 16 KiB.".into());
            }

            value.push_str(&text);
        } else if let Event::Key(key) = event {
            if key.kind == KeyEventKind::Release {
                continue;
            }

            match key.code {
                KeyCode::Enter => return Ok(value),
                KeyCode::Esc => return Err("Setup cancelled; credentials were not saved.".into()),

                KeyCode::Char('c' | 'd') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Err("Setup cancelled; credentials were not saved.".into());
                }

                KeyCode::Backspace => {
                    value.pop();
                }

                KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    value.push(character);

                    if value.len() > LIMIT {
                        return Err("Secret exceeds the 16 KiB limit.".into());
                    }
                }

                _ => {}
            }
        }
    }
}

pub(super) fn activation(root: &Path, id: &str, destination: &Path) -> Result<(), Error> {
    if path(root).canonicalize()? != destination {
        return Err("The daemon uses a different credential file.".into());
    }

    super::credentials::read(destination, &super::credentials::key_path(root))?;

    let names = names(root, id)?;

    if names.iter().any(|name| super::env_nonempty(name)) {
        return Err("The daemon has overriding secret environment variables; restart it.".into());
    }

    super::load_lock_at(root)?.plugins.remove(id).ok_or("Plugin is not installed.")?;

    Ok(())
}

#[cfg(test)]
mod tests;
