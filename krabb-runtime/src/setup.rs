use std::{
    collections::BTreeMap,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
};

use clap::{Args, Parser, Subcommand};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub(crate) mod check;
mod prompt;

pub(super) use prompt::{Prompt, validate as validate_prompts};

type Error = Box<dyn std::error::Error + Send + Sync>;
const LIMIT: usize = 16 * 1024;
const ATTEMPTS: usize = 3;
const CLEAR: &str = "-";
pub(super) const CANCELLED: &str = "Setup cancelled, and credentials were not saved.";

const PASTED: &str = "Pasted secret must be single-line and at most 16 KiB.";

const LONG: &str = "Secret exceeds the 16 KiB limit.";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(super) struct Setting {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

#[derive(Debug, PartialEq)]
struct Field {
    name: String,
    default: Option<String>,
    secret: bool,
    prompt: Prompt,
    saved: bool,
}

impl Field {
    fn secret(name: String) -> Self {
        Self { name, default: None, secret: true, prompt: Prompt::default(), saved: false }
    }

    fn label(&self) -> &str {
        self.prompt.label.as_deref().unwrap_or(&self.name)
    }
}

impl From<Setting> for Field {
    fn from(setting: Setting) -> Self {
        Self {
            name: setting.name,
            default: setting.default,
            secret: false,
            prompt: Prompt::default(),
            saved: false,
        }
    }
}

#[derive(Debug, PartialEq)]
enum Answer {
    Set(String),
    Keep,
    Clear,
}

#[derive(Debug, Default, PartialEq)]
struct Answers {
    values: BTreeMap<String, String>,
    cleared: Vec<String>,
    kept: Vec<String>,
}

#[derive(Debug, Args)]
pub(super) struct Options {
    /// Installed plugin whose secrets and settings should be configured.
    id: String,
    /// Save credentials without contacting the running daemon.
    #[arg(long)]
    no_activate: bool,
    /// Suggested values that replace a setting's declared default, such as a local host.
    #[arg(skip)]
    defaults: BTreeMap<String, String>,
    /// Whether a guide runs setup right after it installed the plugin, so it needs no intro.
    #[arg(skip)]
    guided: bool,
}

impl Options {
    pub(super) fn new(id: &str, defaults: BTreeMap<String, String>, guided: bool) -> Self {
        Self { id: id.into(), no_activate: false, defaults, guided }
    }
}

#[derive(Parser)]
#[command(name = "krabb <plugin>", about = "Set up plugin secrets and settings.")]
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
    /// Securely prompt for secrets and settings without starting the plugin first.
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

    let options = Options { no_activate, ..Options::new(id, BTreeMap::new(), false) };

    run(options, root, json || cli.json).await?;
    Ok(())
}

pub(super) fn path(root: &Path) -> PathBuf {
    super::credentials::path(root)
}

/// Whether installed plugin ID declares secrets or settings for setup to ask about.
pub(super) fn needed(root: &Path, id: &str) -> Result<bool, Error> {
    let lock = super::load_lock_at(root)?;
    let entry = lock.plugins.get(id).ok_or("Install the plugin before running setup.")?;

    Ok(!entry.variables.is_empty() || !entry.settings.is_empty())
}

fn fields(root: &Path, id: &str) -> Result<Vec<Field>, Error> {
    if !super::valid(id) {
        return Err("Invalid plugin ID.".into());
    }

    let lock = super::load_lock_at(root)?;
    let entry = lock.plugins.get(id).ok_or("Install the plugin before running setup.")?;
    let manifest = super::read_manifest_checked(&root.join("plugins").join(id))?;

    if manifest.id != id
        || manifest.variables != entry.variables
        || manifest.settings != entry.settings
    {
        return Err("Plugin setup changed, so review and reinstall the plugin before setup.".into());
    }

    if entry.variables.is_empty() && entry.settings.is_empty() {
        return Err(
            "This plugin declares no secrets or settings, so use its own authentication flow."
                .into(),
        );
    }

    Ok(entry
        .variables
        .iter()
        .cloned()
        .map(Field::secret)
        .chain(entry.settings.iter().cloned().map(Field::from))
        .map(|mut field| {
            field.prompt = manifest.setup.get(&field.name).cloned().unwrap_or_default();
            field
        })
        .collect())
}

/// Asks for and saves the plugin's secrets and settings.
pub(super) async fn run(options: Options, root: &Path, json: bool) -> Result<(), Error> {
    let mut fields = fields(root, &options.id)?;

    for field in &mut fields {
        if let Some(default) = options.defaults.get(&field.name) {
            field.default = Some(default.clone());
        }
    }

    let destination = path(root);
    let key = super::credentials::key_path(root);
    let preference = super::credentials::preference()?;

    // Validate the destination before asking for sensitive input; re-read it under the save lock.
    let saved = super::credentials::read(&destination, &key)?;

    for field in &mut fields {
        field.saved = saved.get(&field.name).is_some_and(|value| !value.is_empty())
            && !options.defaults.contains_key(&field.name);
    }

    let color = crate::style::colors();

    if !fields.is_empty() {
        interactive()?;

        if !options.guided {
            eprintln!("Let's set up the {} plugin.", options.id);
        }

        eprintln!(
            "{}",
            crate::style::dim("Nothing is saved until the last answer, and Esc cancels.", color)
        );
    }

    let answers = collect(&fields, |field, error| prompt(field, color, error))?;
    let changes = summary(&fields, &answers);

    let backend = super::credentials::update(
        &destination,
        &key,
        answers.values,
        &answers.cleared,
        preference,
    )?;

    let destination = destination.canonicalize()?;
    let test = check::run(root, &options.id).await;
    let mut result = finish(root, &options.id, &destination, options.no_activate).await;
    result["check"] = test;
    result["storage"] = backend.into();
    result["changes"] =
        changes.iter().map(|(name, _, change)| json!({"name": name, "change": change})).collect();

    if backend == "file" {
        result["key_path"] = key.canonicalize()?.display().to_string().into();
    }

    if json {
        println!("{}", serde_json::to_string(&result)?);
    } else {
        println!("\n{} Saved the {} settings.", crate::style::accent("✓", color), options.id);

        let rows = changes
            .iter()
            .map(|(_, label, change)| format!("{label}: {change}"))
            .collect::<Vec<_>>();

        if !rows.is_empty() {
            println!("{}", super::format_tree_list(&rows));
        }

        if let Some(line) = check::line(&result["check"]) {
            println!("{line}");
        }

        println!("{}", result["message"].as_str().unwrap_or_default());

        let key = result["key_path"].as_str().unwrap_or("the system keyring");

        println!(
            "Krabb keeps these settings encrypted in {}, with the key in {key}.",
            destination.display()
        );
    }

    Ok(())
}

async fn finish(root: &Path, id: &str, destination: &Path, offline: bool) -> Value {
    let (active, message) = if offline {
        (false, "Activation skipped.\nStart or restart the runtime when ready.".to_owned())
    } else {
        match super::control_at("plugin.setup", json!({"id": id, "credentials": destination}), root)
            .await
        {
            Ok(Some(result)) if result["loaded"] == true => {
                (true, "Plugin loaded in the running daemon.".to_owned())
            }

            Ok(None) => (false, "Start the runtime to activate the plugin.".to_owned()),
            // Plugin errors can contain provider responses. Never include them in setup output.
            _ => (
                false,
                concat!(
                    "Credentials saved, and activation is pending.\n",
                    "Check that the daemon uses the same credential file and has no overriding ",
                    "setup environment variables.\n",
                    "Restart an older daemon, then run krabb doctor."
                )
                .to_owned(),
            ),
        }
    };

    json!({"id": id, "saved": true, "active": active, "path": destination, "message": message})
}

/// Prompts each field, asking again after invalid input so earlier answers are kept.
fn collect(
    fields: &[Field],
    mut prompt: impl FnMut(&Field, Option<&str>) -> Result<String, Error>,
) -> Result<Answers, Error> {
    let mut answers = Answers::default();

    for field in fields {
        let mut error: Option<String> = None;
        let mut attempt = 1;

        let answer = loop {
            let input = prompt(field, error.as_deref())?;

            match resolve(field, input) {
                Ok(answer) => break answer,
                Err(reason) if attempt >= ATTEMPTS => return Err(reason.into()),

                Err(reason) => {
                    error = Some(reason);
                    attempt += 1;
                }
            }
        };

        match answer {
            Answer::Set(value) => {
                answers.values.insert(field.name.clone(), value);
            }

            Answer::Keep => answers.kept.push(field.name.clone()),
            Answer::Clear => answers.cleared.push(field.name.clone()),
        }
    }

    Ok(answers)
}

fn resolve(field: &Field, input: String) -> Result<Answer, String> {
    let required = field.prompt.required;

    if input == CLEAR {
        if required {
            return Err(format!("{} is required and cannot be cleared.", field.label()));
        }

        return Ok(Answer::Clear);
    }

    if input.is_empty() {
        if field.saved {
            return Ok(Answer::Keep);
        }

        if let Some(default) = &field.default {
            return Ok(Answer::Set(default.clone()));
        }

        if required {
            return Err(format!("{} is required.", field.label()));
        }

        if field.secret {
            return Ok(Answer::Clear);
        }
    }

    validate(&input)?;
    field.prompt.check(&input)?;

    Ok(Answer::Set(input))
}

/// Names, labels, and what setup did to each field; never values.
fn summary(fields: &[Field], answers: &Answers) -> Vec<(String, String, &'static str)> {
    fields
        .iter()
        .map(|field| {
            let change = if answers.kept.contains(&field.name) {
                "kept"
            } else if answers.cleared.contains(&field.name) {
                if field.saved { "cleared" } else { "not set" }
            } else if field.saved {
                "changed"
            } else {
                "set"
            };

            (field.name.clone(), field.label().to_owned(), change)
        })
        .collect()
}

pub(super) fn validate(value: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.len() > LIMIT || value.chars().any(char::is_control) {
        return Err("Setup value must be nonempty, single-line, and at most 16 KiB.".into());
    }

    Ok(())
}

struct Terminal;

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = crossterm::execute!(io::stderr(), crossterm::event::DisableBracketedPaste);
        let _ = crossterm::terminal::disable_raw_mode();

        eprintln!();
    }
}

fn interactive() -> Result<(), Error> {
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return Err(
            "Setup requires an interactive terminal, because secret input cannot be piped.".into(),
        );
    }

    Ok(())
}

fn prompt(field: &Field, color: bool, error: Option<&str>) -> Result<String, Error> {
    interactive()?;

    if let Some(error) = error {
        eprintln!("{error}\nTry again, or press Esc to cancel.");
    } else {
        eprint!("{}", heading(field, color));
    }

    read(!field.secret)
}

/// Reads one visible line after HEADING, such as a model name. Enter with nothing typed returns
/// an empty string, Esc cancels with [`CANCELLED`], and [`rejected`] tells an oversized answer.
pub(super) fn line(heading: &str) -> Result<String, Error> {
    interactive()?;
    eprint!("{heading}");

    read(true)
}

/// Whether ERROR refused a pasted or typed answer as too long or multi-line.
pub(super) fn rejected(error: &Error) -> bool {
    [PASTED, LONG].contains(&error.to_string().as_str())
}

fn read(echo: bool) -> Result<String, Error> {
    crossterm::terminal::enable_raw_mode()?;

    let _terminal = Terminal;

    crossterm::execute!(io::stderr(), crossterm::event::EnableBracketedPaste)?;
    eprint!("|> ");
    io::stderr().flush()?;

    input(echo, crossterm::event::read)
}

/// What the person sets and how to answer, shown once per field; never a saved value.
fn heading(field: &Field, color: bool) -> String {
    let prompt = &field.prompt;
    let mut title = field.label().to_owned();

    if field.label() != field.name {
        title.push_str(&crate::style::dim(&format!("  {}", field.name), color));
    }

    let mut heading = format!("\n{title}\n");

    if let Some(help) = &prompt.help {
        heading.push_str(&format!("{help}\n"));
    }

    let dim = |text: &str| crate::style::dim(text, color);

    if let Some(url) = &prompt.url {
        heading.push_str(&format!("{} {}\n", dim("Get it at"), crate::style::strong(url, color)));
    }

    if let Some(example) = &prompt.example {
        let example = crate::style::example(example, color);

        heading.push_str(&format!("{} {example}\n", dim("For example:")));
    }

    for line in [hint(field), field.secret.then(|| "Your typing stays hidden.".to_owned())]
        .into_iter()
        .flatten()
    {
        heading.push_str(&format!("{}\n", dim(&line)));
    }

    heading
}

/// What Enter does, when it does something other than submit the typed value.
fn hint(field: &Field) -> Option<String> {
    let required = field.prompt.required;

    if field.saved {
        let clear = if required { "" } else { ", or type - to clear it" };

        Some(format!("Press Enter to keep the saved value{clear}."))
    } else if let Some(default) = &field.default {
        Some(format!("Press Enter to use {default}."))
    } else if field.secret && !required {
        Some("Press Enter to skip it.".to_owned())
    } else if required {
        Some("This one is required.".to_owned())
    } else {
        None
    }
}

fn input(echo: bool, mut next: impl FnMut() -> io::Result<Event>) -> Result<String, Error> {
    let mut value = String::new();

    let show = |text: &str| {
        if echo {
            eprint!("{text}");
            let _ = io::stderr().flush();
        }
    };

    loop {
        let event = next()?;

        if let Event::Paste(text) = event {
            if text.chars().any(char::is_control) || value.len() + text.len() > LIMIT {
                return Err(PASTED.into());
            }

            value.push_str(&text);
            show(&text);
        } else if let Event::Key(key) = event {
            if key.kind == KeyEventKind::Release {
                continue;
            }

            match key.code {
                KeyCode::Enter => return Ok(value),

                KeyCode::Esc => {
                    return Err(CANCELLED.into());
                }

                KeyCode::Char('c' | 'd') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Err(CANCELLED.into());
                }

                KeyCode::Backspace => {
                    if value.pop().is_some() {
                        show("\u{8} \u{8}");
                    }
                }

                KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    value.push(character);
                    show(character.encode_utf8(&mut [0; 4]));

                    if value.len() > LIMIT {
                        return Err(LONG.into());
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

    let fields = fields(root, id)?;

    if fields.iter().any(|field| super::env_nonempty(&field.name)) {
        return Err("The daemon has overriding setup environment variables, so restart it.".into());
    }

    super::load_lock_at(root)?.plugins.remove(id).ok_or("Plugin is not installed.")?;

    Ok(())
}

#[cfg(test)]
mod tests;
