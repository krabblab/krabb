//! Personalizes a Krabb: the model rewrites KRAB.md and KLAW.md from its owner's wishes.
//!
//! Wishes stay in the owner's own words and language. The model interprets them, so the host
//! never parses them for keywords; it only checks the shape and size of what comes back.

use std::io::{BufRead, Read, Write};
use std::path::Path;
use std::time::Duration;

use krabb_core::types::{Content, Message, Role};

use crate::cli::HatchArgs;

type Error = Box<dyn std::error::Error + Send + Sync>;

/// Each file stays well inside its share of the instruction context.
const FILE_LIMIT: usize = crate::CONTEXT_LIMIT / 2 - 128;
const SUMMARY_LIMIT: usize = 600;
pub(crate) const WISH_LIMIT: usize = 2000;
/// Hand-edited files larger than this are refused rather than sent to the model.
const READ_LIMIT: u64 = 16 * 1024;
const LOCK_WAIT: Duration = Duration::from_secs(5);
const FILES: [(&str, &str); 2] = [("KRAB.md", "KRAB.previous.md"), ("KLAW.md", "KLAW.previous.md")];
const MARKERS: [&str; 3] = ["=== KRAB.md ===", "=== KLAW.md ===", "=== SUMMARY ==="];

/// Explains what hatching needs when no intelligence provider is ready.
pub(crate) fn no_provider(directory: &Path) -> String {
    format!(
        "Hatching needs an intelligence provider, because your Krabb's model writes its \
         personality from your answers.\nSet one up with `krabb plugin install openai` and \
         `krabb plugin setup openai`, or use `ollama` for a local model.\nMeanwhile, edit your \
         Krabb by hand in {}: KRAB.md says who it is, and KLAW.md says how it works.",
        directory.display(),
    )
}

/// One interview question, labelled for the model and grouped under a section.
struct Question {
    section: &'static str,
    label: &'static str,
    prompt: &'static str,
    example: &'static str,
    limit: usize,
}

const QUESTIONS: [Question; 8] = [
    Question {
        section: "Identity",
        label: "Name",
        prompt: "What should your Krabb be called?",
        example: "Pinch",
        limit: 40,
    },
    Question {
        section: "Identity",
        label: "Call the owner",
        prompt: "What should it call you?",
        example: "Sam",
        limit: 40,
    },
    Question {
        section: "Identity",
        label: "Personality",
        prompt: "Describe its personality.",
        example: "warm, curious, a little playful, never sarcastic",
        limit: 200,
    },
    Question {
        section: "Voice",
        label: "Tone of voice",
        prompt: "How should it sound?",
        example: "plain and direct, like a trusted colleague",
        limit: 200,
    },
    Question {
        section: "Voice",
        label: "Writing",
        prompt: "How should it write replies?",
        example: "lead with the answer, short paragraphs, no emoji",
        limit: 200,
    },
    Question {
        section: "Voice",
        label: "Language",
        prompt: "Which language should it use?",
        example: "reply in the language I write in",
        limit: 100,
    },
    Question {
        section: "Work",
        label: "Way of working",
        prompt: "How should it get things done?",
        example: "act on clear requests, ask before anything irreversible",
        limit: 300,
    },
    Question {
        section: "Work",
        label: "Anything else",
        prompt: "Anything else it should know about itself or you?",
        example: "I build embedded firmware and prefer Rust examples",
        limit: 400,
    },
];

/// The two instruction files, as text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Files {
    pub(crate) krab: String,
    pub(crate) klaw: String,
}

impl Files {
    fn texts(&self) -> [&str; 2] {
        [&self.krab, &self.klaw]
    }
}

/// What the model proposed: new files and a summary for the owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Draft {
    pub(crate) files: Files,
    pub(crate) summary: String,
}

/// Trims an answer and rejects one that is too long or spans several lines.
fn clean(value: &str, limit: usize) -> Result<Option<String>, String> {
    let value = value.trim();

    if value.is_empty() {
        return Ok(None);
    }

    if value.chars().any(char::is_control) {
        return Err("Each answer must be a single line of text.".into());
    }

    if value.chars().count() > limit {
        return Err(format!("Keep this answer to {limit} characters or fewer."));
    }

    Ok(Some(value.to_owned()))
}

/// Joins labelled answers and free text into the wishes the model reads.
fn wishes(answers: &[Option<String>], free: &str) -> Option<String> {
    let mut lines = QUESTIONS
        .iter()
        .zip(answers)
        .filter_map(|(question, answer)| {
            answer.as_ref().map(|answer| format!("- {}: {answer}", question.label))
        })
        .collect::<Vec<_>>();

    let free = free.trim();

    if !free.is_empty() {
        lines.push(free.to_owned());
    }

    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// Reads the current files; a missing file reads as empty. Links, other entries, and files
/// over the read limit are refused.
pub(crate) fn read(directory: &Path) -> std::io::Result<Files> {
    let load = |name: &str| {
        existing(&directory.join(name))
            .map(|bytes| String::from_utf8_lossy(&bytes.unwrap_or_default()).into_owned())
    };

    Ok(Files { krab: load(FILES[0].0)?, klaw: load(FILES[1].0)? })
}

/// Builds the model request that turns CURRENT and WISHES into new files.
pub(crate) fn request(session: &str, current: &Files, wishes: &str) -> Vec<Message> {
    let shown = |text: &str| {
        if text.trim().is_empty() { "(empty)".to_owned() } else { text.trim_end().to_owned() }
    };

    let material = format!(
        "Current KRAB.md:\n<<<\n{}\n>>>\n\nCurrent KLAW.md:\n<<<\n{}\n>>>\n\nThe owner's \
         wishes:\n<<<\n{wishes}\n>>>",
        shown(&current.krab),
        shown(&current.klaw),
    );

    let message = |id: &str, role, text: String| Message {
        id: id.into(),
        session: session.into(),
        role,
        sender: None,
        content: vec![Content::Text { text }],
    };

    vec![
        message("hatch-instructions", Role::System, crate::templates::HATCH.into()),
        message("hatch-wishes", Role::User, material),
    ]
}

/// Checks the model's reply and splits it into files and a summary.
pub(crate) fn parse(reply: &str) -> Result<Draft, String> {
    let mut parts = Vec::with_capacity(MARKERS.len());
    let mut rest = reply;

    for (index, marker) in MARKERS.iter().enumerate() {
        let start = rest.find(marker).ok_or_else(|| format!("The draft is missing {marker}."))?;
        let after = &rest[start + marker.len()..];
        let end = MARKERS.get(index + 1).and_then(|next| after.find(next)).unwrap_or(after.len());

        parts.push(unfence(&after[..end]));
        rest = &after[end..];
    }

    let summary = parts.pop().unwrap_or_default();
    let klaw = parts.pop().unwrap_or_default();
    let krab = parts.pop().unwrap_or_default();

    for (text, heading, name) in [
        (&krab, "# Personality Instructions", "KRAB.md"),
        (&klaw, "# Behavioral Instructions", "KLAW.md"),
    ] {
        if text != heading && !text.starts_with(&format!("{heading}\n")) {
            return Err(format!("The draft's {name} must start with {heading}."));
        }

        if text.len() > FILE_LIMIT {
            return Err(format!("The draft's {name} is longer than {FILE_LIMIT} bytes."));
        }
    }

    let summary = summary.split_whitespace().collect::<Vec<_>>().join(" ");

    if summary.is_empty() || summary.chars().count() > SUMMARY_LIMIT {
        return Err("The draft's summary is missing or too long.".into());
    }

    Ok(Draft { files: Files { krab: krab + "\n", klaw: klaw + "\n" }, summary })
}

/// Trims a part and drops a code fence the model may have wrapped around all of it.
fn unfence(text: &str) -> String {
    let text = text.trim();

    let Some(opened) = text.strip_prefix("```") else {
        return text.to_owned();
    };

    let body = opened.split_once('\n').map_or("", |(_, body)| body).trim_end();

    body.strip_suffix("```").unwrap_or(body).trim().to_owned()
}

/// Saves FILES in DIRECTORY if it still holds EXPECTED, the files the draft was written
/// from. When anything changes, both files are first backed up as they were, so the backups
/// always hold the state before the latest change; a missing file is backed up as empty,
/// which reads as absent. Returns whether anything changed. A lock shared by the CLI and
/// the daemon keeps one change from overwriting another.
pub(crate) fn save(directory: &Path, expected: &Files, files: &Files) -> std::io::Result<bool> {
    let _lock = crate::locks::lock_at_timeout(directory, ".hatch.lock", LOCK_WAIT)?;

    for (_, backup) in FILES {
        existing(&directory.join(backup))?;
    }

    if read(directory)? != *expected {
        return Err(std::io::Error::other(
            "The personality changed while the new one was being written, so nothing was \
             saved.\nRun hatch again to build on the latest version.",
        ));
    }

    if expected == files {
        return Ok(false);
    }

    for ((_, backup), previous) in FILES.iter().zip(expected.texts()) {
        krabb_file::save(directory.join(backup), previous)?;
    }

    for ((name, _), (previous, text)) in
        FILES.iter().zip(expected.texts().into_iter().zip(files.texts()))
    {
        if previous != text {
            krabb_file::save(directory.join(name), text)?;
        }
    }

    Ok(true)
}

/// Swaps both files with their backups, so a second undo restores the newer version.
/// Each file is written before its backup, so an interruption can lose only the newer text.
/// Returns the files whose text changed.
pub(crate) fn undo(directory: &Path) -> std::io::Result<Vec<&'static str>> {
    let _lock = crate::locks::lock_at_timeout(directory, ".hatch.lock", LOCK_WAIT)?;
    let mut pairs = Vec::with_capacity(FILES.len());

    for (name, backup) in FILES {
        let (path, backup) = (directory.join(name), directory.join(backup));
        let current = existing(&path)?.unwrap_or_default();

        let Some(previous) = existing(&backup)? else {
            return Ok(Vec::new());
        };

        pairs.push((name, path, backup, current, previous));
    }

    let mut restored = Vec::new();

    for (name, path, backup, current, previous) in pairs {
        if current == previous {
            continue;
        }

        krabb_file::save(&path, &previous)?;
        krabb_file::save(&backup, &current)?;
        restored.push(name);
    }

    Ok(restored)
}

/// Reads PATH if it is a regular file, and refuses a link or any other kind of entry.
fn existing(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Instruction file must be a regular file: {}.", path.display()),
            ))
        }

        Ok(_) => {
            let mut bytes = Vec::new();

            std::fs::File::open(path)?.take(READ_LIMIT + 1).read_to_end(&mut bytes)?;

            if bytes.len() as u64 > READ_LIMIT {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "{} is larger than {} KiB, so shorten it by hand first.",
                        path.display(),
                        READ_LIMIT / 1024
                    ),
                ));
            }

            Ok(Some(bytes))
        }

        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Whether CURRENT is a personality other than the one `init` writes.
pub(crate) fn hatched(current: &Files) -> bool {
    current.krab != crate::templates::KRAB || current.klaw != crate::templates::KLAW
}

/// Asks each question on OUTPUT, reading replies from INPUT. An empty reply keeps the current
/// setting, and closed input cancels the interview with an `UnexpectedEof` error.
pub(crate) fn interview(
    input: &mut impl BufRead,
    output: &mut impl Write,
    rerun: bool,
    color: bool,
) -> std::io::Result<Vec<Option<String>>> {
    let intro = if rerun {
        "Your Krabb already has a personality.\nAnswer only what you want to change."
    } else {
        "Let's hatch your Krabb.\nAnswer in your own words and in any language."
    };

    let enter = if rerun { "keep its current setting" } else { "skip it" };

    writeln!(output, "{intro}\nPress Enter on any question to {enter}.")?;

    let mut answers = Vec::with_capacity(QUESTIONS.len());
    let mut section = "";

    for (index, question) in QUESTIONS.iter().enumerate() {
        if question.section != section {
            section = question.section;
            writeln!(output, "\n{}", crate::style::accent(section, color))?;
        }

        answers.push(ask(input, output, index + 1, question, color)?);
    }

    writeln!(output)?;

    Ok(answers)
}

fn ask(
    input: &mut impl BufRead,
    output: &mut impl Write,
    number: usize,
    question: &Question,
    color: bool,
) -> std::io::Result<Option<String>> {
    let limit = format!("Up to {} characters, for example:", question.limit);

    writeln!(
        output,
        "\n{}",
        crate::style::accent(&format!("{number}. {}", question.prompt), color)
    )?;
    writeln!(
        output,
        "   {} {}",
        crate::style::dim(&limit, color),
        crate::style::example(question.example, color)
    )?;

    loop {
        write!(output, "   > ")?;
        output.flush()?;

        let mut line = String::new();

        if input.read_line(&mut line)? == 0 {
            writeln!(output)?;
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }

        match clean(&line, question.limit) {
            Ok(answer) => return Ok(answer),
            Err(problem) => writeln!(output, "   {problem}")?,
        }
    }
}

pub(crate) async fn run(args: HatchArgs, root: &Path, json: bool) -> Result<(), Error> {
    let directory = root.join("data/system");

    if !directory.is_dir() {
        return Err("Krabb is not initialized.\nRun `krabb init` first.".into());
    }

    if args.undo {
        return restore(&directory, args.yes, json);
    }

    let given = [
        &args.name,
        &args.call,
        &args.personality,
        &args.voice,
        &args.writing,
        &args.language,
        &args.work,
        &args.notes,
    ];

    let mut answers = given
        .iter()
        .zip(&QUESTIONS)
        .map(|(value, question)| {
            value.as_deref().map_or(Ok(None), |value| {
                clean(value, question.limit)
                    .map_err(|problem| format!("{}: {problem}", question.label))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let free = args.wishes.join(" ");

    if free.chars().count() > WISH_LIMIT {
        return Err(format!("Wishes must be at most {WISH_LIMIT} characters.").into());
    }

    let config = crate::load_config_at(root)?;

    config.validate()?;

    let (provider, model) = crate::selection::client(&config, "cli", (None, None));

    let Ok(plugin) = crate::model_plugin_at(root, &provider) else {
        return Err(no_provider(&directory).into());
    };

    let interactive = !json && crate::agent::interactive();

    if !args.yes && !interactive {
        return Err("Hatching asks before saving.\nUse --yes to save without a prompt.".into());
    }

    let current = read(&directory)?;

    if interactive && answers.iter().all(Option::is_none) && free.trim().is_empty() {
        let mut input = std::io::stdin().lock();

        answers = match interview(
            &mut input,
            &mut std::io::stderr(),
            hatched(&current),
            crate::style::colors(),
        ) {
            Ok(answers) => answers,

            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                crate::agent::print_cancelled("Hatching", json);
                return Ok(());
            }

            Err(error) => return Err(error.into()),
        };
    }

    let Some(wishes) = wishes(&answers, &free) else {
        if json {
            println!("{}", serde_json::json!({"status": "unchanged"}));
        } else {
            println!(
                "Nothing to change.\nAnswer at least one question, or describe the change: \
                 `krabb hatch be more concise`."
            );
        }

        return Ok(());
    };

    let spinner = crate::terminal::progress("Your Krabb is writing its personality", json);
    let reply = crate::model_at(plugin, model, root, request("cli", &current, &wishes)).await;

    drop(spinner);

    let draft = parse(&reply?.text).map_err(|problem| {
        format!(
            "{problem}\nNothing was changed.\nRun `krabb hatch` again, or edit the files in {} \
             by hand.",
            directory.display()
        )
    })?;

    if draft.files == current {
        report(json, "unchanged", &draft.summary, &directory);
        return Ok(());
    }

    if !args.yes {
        eprintln!("{}\n", draft.summary);

        loop {
            eprintln!("Save this personality?\n");

            let choice = crate::choice::pick(&["Save it", "Show the files", "Discard it"], 0)?;

            eprintln!();

            match choice {
                Some(0) => break,
                Some(1) => eprintln!("{}\n{}", draft.files.krab, draft.files.klaw),

                _ => {
                    crate::agent::print_cancelled("Hatching", json);
                    return Ok(());
                }
            }
        }
    }

    save(&directory, &current, &draft.files)?;

    let summary = if args.yes { draft.summary.as_str() } else { "" };

    report(json, "hatched", summary, &directory);

    Ok(())
}

/// Prints the outcome; an empty summary was already shown before the confirmation.
fn report(json: bool, status: &str, summary: &str, directory: &Path) {
    if json {
        let report = serde_json::json!({"status": status, "summary": summary});

        println!("{report}");
        return;
    }

    if !summary.is_empty() {
        println!("{summary}");
    }

    if status == "hatched" {
        let show = if cfg!(windows) { "type" } else { "cat" };

        let [krab, klaw] = FILES.map(|(name, _)| directory.join(name).display().to_string());

        println!("Saved your Krabb's personality.");
        println!("Read it with `{show} \"{krab}\" \"{klaw}\"`");
        println!("It applies from your Krabb's next reply.");
        println!("Undo with `krabb hatch --undo`");
    }
}

fn restore(directory: &Path, yes: bool, json: bool) -> Result<(), Error> {
    if !crate::agent::confirm_action(yes, json, "Restore the previous personality?")? {
        crate::agent::print_cancelled("Restoring", json);
        return Ok(());
    }

    let restored = undo(directory)?;

    if json {
        println!("{}", serde_json::json!({"status": "restored", "files": restored}));
    } else if restored.is_empty() {
        println!("There is no previous personality to restore.");
    } else {
        println!("Restored the previous {}.", restored.join(" and "));
        println!("Undo again to switch back.");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn directory() -> PathBuf {
        let path = crate::tests::test_root("hatch");

        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn draft(krab: &str, klaw: &str, summary: &str) -> String {
        format!("=== KRAB.md ===\n{krab}\n=== KLAW.md ===\n{klaw}\n=== SUMMARY ===\n{summary}\n")
    }

    fn files(krab: &str, klaw: &str) -> Files {
        Files { krab: krab.into(), klaw: klaw.into() }
    }

    fn parse_args(flags: &[&str]) -> HatchArgs {
        use clap::Parser;

        let arguments = ["krabb", "hatch"].iter().chain(flags);

        match crate::cli::Cli::try_parse_from(arguments).unwrap().command {
            crate::cli::Command::Hatch(args) => args,
            command => panic!("parsed {command:?}"),
        }
    }

    #[test]
    fn a_well_formed_draft_becomes_files_and_a_summary() {
        let reply = format!(
            "Here you go.\n{}",
            draft(
                "```md\n# Personality Instructions\n\nYou are Pinch.\n```",
                "# Behavioral Instructions\n\nRun checks:\n\n```sh\nmake verify\n```",
                "Ora ti chiami\nPinch.\n",
            )
        );

        let draft = parse(&reply).unwrap();

        assert_eq!(draft.files.krab, "# Personality Instructions\n\nYou are Pinch.\n");
        assert!(draft.files.klaw.ends_with("```sh\nmake verify\n```\n"));
        assert_eq!(draft.summary, "Ora ti chiami Pinch.");
    }

    #[test]
    fn malformed_drafts_are_rejected_with_the_reason() {
        let krab = "# Personality Instructions\n\nYou are Pinch.";
        let klaw = "# Behavioral Instructions\n\nAsk first.";

        assert!(parse("I could not do that.").unwrap_err().contains("missing === KRAB.md ==="));
        assert!(parse(&draft("You are Pinch.", klaw, "Done.")).unwrap_err().contains("KRAB.md"));
        assert!(parse(&draft(krab, "Ask first.", "Done.")).unwrap_err().contains("KLAW.md"));
        assert!(parse(&draft(krab, klaw, " ")).unwrap_err().contains("summary"));

        let glued = draft("# Personality Instructionsfoo", klaw, "Done.");

        assert!(parse(&glued).unwrap_err().contains("must start with"));

        let long = format!("{krab}\n{}", "x".repeat(FILE_LIMIT));

        assert!(parse(&draft(&long, klaw, "Done.")).unwrap_err().contains("longer"));
    }

    #[test]
    fn the_request_carries_the_hatching_prompt_current_files_and_wishes() {
        let current = files("# Personality Instructions\nYou are Pinch.\n", "");
        let messages = request("cli", &current, "- Name: Nipper");

        assert_eq!(messages[0].role, Role::System);
        assert_eq!(messages[0].content[0].render(), crate::templates::HATCH);

        let material = messages[1].content[0].render();

        assert!(material.contains("<<<\n# Personality Instructions\nYou are Pinch.\n>>>"));
        assert!(material.contains("Current KLAW.md:\n<<<\n(empty)\n>>>"));
        assert!(material.ends_with("wishes:\n<<<\n- Name: Nipper\n>>>"));
    }

    #[test]
    fn wishes_join_labelled_answers_and_free_text() {
        let mut answers = vec![None; QUESTIONS.len()];

        assert_eq!(wishes(&answers, "  "), None);

        answers[0] = Some("Pinch".into());
        answers[5] = Some("italiano".into());

        assert_eq!(
            wishes(&answers, "be brief").unwrap(),
            "- Name: Pinch\n- Language: italiano\nbe brief"
        );
    }

    #[test]
    fn the_interview_keeps_answers_in_order_and_retries_invalid_ones() {
        let skipped = "\n".repeat(QUESTIONS.len() - 3);
        let long = "x".repeat(QUESTIONS[1].limit + 1);
        let replies = format!("Pinch\n{long}\n\nplayful\n{skipped}");
        let mut output = Vec::new();

        let answers = interview(&mut replies.as_bytes(), &mut output, true, false).unwrap();
        let output = String::from_utf8(output).unwrap();

        assert_eq!(answers.len(), QUESTIONS.len());
        assert_eq!(answers[0].as_deref(), Some("Pinch"));
        assert_eq!(answers[1], None);
        assert_eq!(answers[2].as_deref(), Some("playful"));
        assert!(answers[3..].iter().all(Option::is_none));

        assert!(output.starts_with("Your Krabb already has a personality.\n"));
        assert!(output.contains("Press Enter on any question to keep its current setting."));
        assert!(output.contains("\nIdentity\n\n1. What should your Krabb be called?\n"));
        assert!(output.contains("   Up to 40 characters, for example: Pinch\n"));
        assert!(output.contains("   Keep this answer to 40 characters or fewer.\n"));
        assert!(output.contains("\nWork\n\n7. How should it get things done?"));
        assert_eq!(output.matches("\nVoice\n").count(), 1);
    }

    #[test]
    fn closing_the_input_cancels_the_interview() {
        let error =
            interview(&mut "Pinch\n".as_bytes(), &mut Vec::new(), false, false).unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn answers_are_single_bounded_lines() {
        assert_eq!(clean("  calm  ", 10), Ok(Some("calm".into())));
        assert_eq!(clean("   ", 10), Ok(None));
        assert!(clean("one\ntwo", 10).is_err());
        assert!(clean("toolongname", 10).is_err());
        assert_eq!(clean("ééééé", 5), Ok(Some("ééééé".into())));
    }

    #[test]
    fn saving_backs_up_both_files_and_undo_swaps_them_back() {
        let directory = directory();
        let empty = Files::default();
        let first = files("krab one", "klaw one");
        let second = files("krab two", "klaw one");

        assert!(undo(&directory).unwrap().is_empty());
        assert!(save(&directory, &empty, &first).unwrap());
        assert!(!save(&directory, &first, &first).unwrap());
        assert!(save(&directory, &first, &second).unwrap());

        let klaw = std::fs::read_to_string(directory.join("KLAW.previous.md")).unwrap();

        assert_eq!(klaw, "klaw one");
        assert_eq!(read(&directory).unwrap(), second);

        assert_eq!(undo(&directory).unwrap(), ["KRAB.md"]);
        assert_eq!(read(&directory).unwrap(), first);
        assert_eq!(undo(&directory).unwrap(), ["KRAB.md"]);
        assert_eq!(read(&directory).unwrap(), second);

        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn saving_refuses_a_personality_that_changed_meanwhile() {
        let directory = directory();
        let first = files("krab one", "klaw one");

        save(&directory, &Files::default(), &first).unwrap();

        let error = save(&directory, &Files::default(), &files("krab two", "")).unwrap_err();

        assert!(error.to_string().contains("changed while"));
        assert_eq!(read(&directory).unwrap(), first);

        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn oversized_files_are_refused_instead_of_sent_to_the_model() {
        let directory = directory();

        std::fs::write(directory.join("KLAW.md"), "x".repeat(READ_LIMIT as usize + 1)).unwrap();

        assert!(read(&directory).unwrap_err().to_string().contains("shorten it by hand"));

        let _ = std::fs::remove_dir_all(directory);
    }

    #[cfg(unix)]
    #[test]
    fn saving_refuses_linked_files_before_writing_anything() {
        let directory = directory();
        let outside = directory.join("outside.md");

        std::fs::write(&outside, "outside").unwrap();
        std::os::unix::fs::symlink(&outside, directory.join("KLAW.md")).unwrap();

        let empty = Files::default();

        assert!(save(&directory, &empty, &files("krab", "klaw")).is_err());
        assert!(undo(&directory).unwrap().is_empty());
        assert!(read(&directory).is_err());
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "outside");
        assert!(!directory.join("KRAB.md").exists());

        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn without_a_provider_hatching_explains_setup_and_hand_editing() {
        let root = crate::tests::test_root("hatch-command");
        let error = run(parse_args(&["be", "brief"]), &root, true).await.unwrap_err();

        assert!(error.to_string().contains("krabb init"));

        crate::init_at(&root).unwrap();

        let long = "x".repeat(QUESTIONS[0].limit + 1);
        let error = run(parse_args(&["--name", &long, "--yes"]), &root, true).await.unwrap_err();

        assert_eq!(error.to_string(), "Name: Keep this answer to 40 characters or fewer.");

        let error = run(parse_args(&["--name", "Pinch", "--yes"]), &root, true).await.unwrap_err();
        let message = error.to_string();
        let directory = root.join("data/system");

        assert!(message.contains("needs an intelligence provider"));
        assert!(message.contains("krabb plugin setup openai"));
        assert!(message.contains(&directory.display().to_string()));
        assert!(message.ends_with("KLAW.md says how it works."));
        assert!(!hatched(&read(&directory).unwrap()));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn undo_cannot_be_combined_with_wishes() {
        use clap::Parser;

        for flags in [["--undo", "be brief"], ["--undo", "--name=Pinch"]] {
            let arguments = ["krabb", "hatch"].iter().chain(&flags);

            assert!(crate::cli::Cli::try_parse_from(arguments).is_err());
        }
    }

    #[tokio::test]
    async fn undo_restores_the_previous_personality_without_a_provider() {
        let root = crate::tests::test_root("hatch-undo");

        crate::init_at(&root).unwrap();

        let directory = root.join("data/system");
        let current = read(&directory).unwrap();
        let changed = files("krab hatched", crate::templates::KLAW);

        save(&directory, &current, &changed).unwrap();

        assert!(hatched(&read(&directory).unwrap()));
        assert!(run(parse_args(&["--undo"]), &root, true).await.is_err());

        run(parse_args(&["--undo", "--yes"]), &root, true).await.unwrap();

        assert!(!hatched(&read(&directory).unwrap()));

        let _ = std::fs::remove_dir_all(root);
    }
}
