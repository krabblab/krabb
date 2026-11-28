//! Host-written facts about a tool call, which open its approval prompt.
//!
//! Each fact line comes from the tool name and arguments alone, never from the model, so the
//! approver can trust what kind of action it is before reading the exact arguments below it.

use super::shell;
use crate::*;

const SHOWN: usize = 5;
const WORD_LIMIT: usize = 80;

/// The fact line for the call of NAME with ARGS.
pub(crate) fn fact(name: &str, args: &Value) -> String {
    match name {
        name if tasks::action(name).is_some() => {
            let action = tasks::action(name).unwrap_or_default();

            let detail = match action {
                "steer" => format!(" with the instruction: {}", text(&args["instruction"])),
                "defer" => format!(" until {}", text(&args["when"])),
                "extend" => format!(" by {} minutes", args["minutes"]),
                _ => String::new(),
            };

            let fact = format!("{} the task {}{detail}", capitalized(action), text(&args["id"]));

            if fact.ends_with(['.', '!', '?']) { fact } else { format!("{fact}.") }
        }

        routing::NAME => format!(
            "Start a background goal that continues on its own and sends updates to {} / {}. \
                Unattended workspace tools: {}, shell: {}, workspace: {}.",
            text(&args["context"]["origin"]["channel"]),
            text(&args["context"]["origin"]["chat"]),
            args["context"]["tools"] == true,
            args["context"]["shell"] == true,
            text(&args["context"]["workspace"]),
        ),

        "schedule_propose" => schedule(args),

        name if name.starts_with("schedule_") => {
            let action = name.trim_start_matches("schedule_");

            format!("{} the schedule {}.", capitalized(action), text(&args["id"]))
        }

        "message_send" => {
            format!("Send a message to {} / {}.", text(&args["channel"]), text(&args["chat"]))
        }

        "file_write" => {
            let content = args["text"].as_str().unwrap_or_default();
            let lines = content.lines().count();

            let kind = format!(
                "Write the file {} ({lines} {}, {} {})",
                word(args["path"].as_str().unwrap_or("at a workspace path")),
                if lines == 1 { "line" } else { "lines" },
                content.len(),
                if content.len() == 1 { "byte" } else { "bytes" },
            );

            sentence(&kind, &["replaces any file already there".into()])
        }

        "file_patch" => patch(args["text"].as_str().unwrap_or_default()),
        "git_run" => git(args),

        "shell_run" => match args["command"].as_str() {
            Some(command) => shell::fact(command),
            None => "Run a shell command.".into(),
        },

        "memory_remember" => format!("Save the memory {}.", text(&args["key"])),
        "data_query" => format!("Change the database: {}.", text(&args["operation"])),

        name if name.starts_with("browser_") => {
            format!("Use the browser: {}.", word(name.trim_start_matches("browser_")))
        }

        name if name.starts_with("computer_") => {
            format!("Use the computer: {}.", word(name.trim_start_matches("computer_")))
        }

        name => format!("Run the {} tool.", visible(name)),
    }
}

/// The part of an approval above the exact arguments: FACT, then PURPOSE when the model gave
/// one.
pub(crate) fn summary(fact: String, purpose: Option<&str>) -> String {
    match purpose {
        Some(purpose) => format!("{fact}\n{}", super::purpose::line(purpose)),
        None => fact,
    }
}

/// Whether CHARACTER is a format or separator character that a reader cannot see, which
/// could hide text, reorder it, or start a new line: Unicode's Cf, Zl, and Zp characters.
fn invisible(character: char) -> bool {
    matches!(
        character,
        '\u{00ad}'
            | '\u{061c}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
            | '\u{e0000}'..='\u{e007f}'
    )
}

/// TEXT with control characters and invisible marks escaped, so it stays on one line and
/// cannot hide or reorder what surrounds it.
pub(crate) fn visible(text: &str) -> String {
    let mut shown = String::with_capacity(text.len());

    for character in text.chars() {
        match character {
            '\n' => shown.push_str("\\n"),
            '\r' => shown.push_str("\\r"),
            '\t' => shown.push_str("\\t"),

            character if character.is_control() || invisible(character) => {
                shown.push_str(&format!("\\u{{{:04x}}}", u32::from(character)));
            }

            character => shown.push(character),
        }
    }

    shown
}

/// ITEMS as a short list: the first few, each clipped, then how many more there are.
pub(crate) fn list(items: &[String]) -> String {
    let mut shown = items.iter().take(SHOWN).map(|item| word(item)).collect::<Vec<_>>();

    if items.len() > SHOWN {
        shown.push(format!("{} more", items.len() - SHOWN));
    }

    shown.join(", ")
}

/// KIND as a sentence, followed by one sentence that joins FLAGS.
pub(crate) fn sentence(kind: &str, flags: &[String]) -> String {
    let flags = match flags {
        [] => return format!("{kind}."),
        [only] => only.clone(),
        [first, second] => format!("{first} and {second}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    };

    format!("{kind}. It {flags}.")
}

pub(crate) fn clipped(text: &str, limit: usize) -> String {
    let shown = visible(text);

    if shown.chars().count() <= limit {
        return shown;
    }

    let mut clipped = shown.chars().take(limit.saturating_sub(1)).collect::<String>();
    clipped.push('…');
    clipped
}

/// TEXT as one item in a fact line: clipped, and quoted when it has spaces, so a name the
/// model chose cannot read as a sentence the host wrote.
fn word(text: &str) -> String {
    let shown = clipped(text, WORD_LIMIT);

    if text.chars().all(|character| character.is_ascii_graphic()) {
        return shown;
    }

    let shown = shown.replace('“', "\\u{201c}").replace('”', "\\u{201d}");

    format!("“{shown}”")
}

fn text(value: &Value) -> String {
    match value {
        Value::String(text) => word(text),
        Value::Null => "(none)".into(),
        value => word(&value.to_string()),
    }
}

fn capitalized(word: &str) -> String {
    let mut characters = word.chars();

    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

fn schedule(args: &Value) -> String {
    let destination = if let Some(destinations) = args["context"].get("destinations") {
        format!("outbound destinations {}", visible(&destinations.to_string()))
    } else {
        args["context"].get("message_destination").filter(|value| !value.is_null()).map_or_else(
            || "no outbound message destination".into(),
            |value| {
                format!(
                    "outbound messages to channel {}, chat {}, private {}, thread {}",
                    word(value["channel"].as_str().unwrap_or("invalid")),
                    word(value["chat"].as_str().unwrap_or("invalid")),
                    value["private"].as_bool().unwrap_or(false),
                    word(value["thread"].as_str().unwrap_or("none")),
                )
            },
        )
    };

    format!(
        "Create a scheduled task with {destination}. Unattended workspace tools: {}, shell: {}, \
            workspace: {}.",
        args["context"]["tools"] == true,
        args["context"]["shell"] == true,
        text(&args["context"]["workspace"]),
    )
}

fn patch(text: &str) -> String {
    let mut files = Vec::new();
    let mut deletes = Vec::new();
    let mut previous = None;
    let mut current: Option<String> = None;

    for line in text.lines() {
        if let Some(header) = line.strip_prefix("diff --git ") {
            current = header.split(' ').next().map(|path| path.trim_start_matches("a/").to_owned());
        } else if line.starts_with("deleted file mode") {
            deletes.extend(current.clone());
            files.extend(current.clone());
        } else if let Some(path) = line.strip_prefix("rename from ") {
            deletes.push(path.into());
            files.push(path.into());
        } else if let Some(path) = line.strip_prefix("rename to ") {
            files.push(path.into());
        } else if let Some(path) = line.strip_prefix("--- ") {
            previous = Some(patch_path(path));
        } else if let Some(path) = line.strip_prefix("+++ ") {
            let path = patch_path(path);
            let old = previous.take().flatten();

            match (old, path) {
                (Some(old), None) => {
                    deletes.push(old.clone());
                    files.push(old);
                }

                (_, Some(new)) => files.push(new),

                (None, None) => {}
            }
        }
    }

    files.dedup();
    deletes.dedup();

    let bytes = format!("{} {}", text.len(), if text.len() == 1 { "byte" } else { "bytes" });

    let kind = if files.is_empty() {
        format!("Apply a Git patch of {bytes}")
    } else {
        format!("Apply a Git patch of {bytes} to {}", list(&files))
    };

    let mut flags = Vec::new();

    if !deletes.is_empty() {
        flags.push(format!("deletes {}", list(&deletes)));
    }

    flags.push("may run repository-configured programs".into());
    sentence(&kind, &flags)
}

fn patch_path(path: &str) -> Option<String> {
    let path = path.split('\t').next().unwrap_or(path).trim();

    if path == "/dev/null" {
        return None;
    }

    Some(path.strip_prefix("a/").or_else(|| path.strip_prefix("b/")).unwrap_or(path).into())
}

/// The fact line for a `git_run` call, read by the same classifier as the shell, so every
/// option and subcommand the shell would flag is flagged here too.
fn git(args: &Value) -> String {
    let words: Vec<&str> =
        args["args"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();

    let command = std::iter::once("git".to_owned())
        .chain(words.iter().map(|word| quoted(word)))
        .collect::<Vec<_>>()
        .join(" ");

    let fact = super::shell::describe(
        &command,
        krabb_core::command::Dialect::Sh,
        krabb_core::command::Os::Linux,
    );

    let fact = match fact.strip_prefix("Run a shell command") {
        Some(rest) => format!("Run a Git command{rest}"),
        None => fact,
    };

    let configured = words.iter().take_while(|word| word.starts_with('-')).any(|word| {
        matches!(*word, "-c" | "--exec-path")
            || word.starts_with("--config")
            || word.starts_with("--exec-path=")
    });

    let warning = "sets its own configuration, which can run any program";

    match configured {
        true if fact.contains("may run repository-configured programs") => {
            fact.replace("may run repository-configured programs", warning)
        }

        true => format!("{fact} It {warning}."),
        false => fact,
    }
}

/// WORD quoted for `sh`, so the classifier reads it as one argument.
fn quoted(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests;
