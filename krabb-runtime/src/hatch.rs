//! Personalizes a Krabb: the model rewrites KRAB.md and KLAW.md from its owner's wishes.
//!
//! Wishes stay in the owner's own words and language. The model interprets them, so the host
//! never parses them for keywords; it only checks the shape and size of what comes back.

use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;

use krabb_core::types::{Content, Message, Role};

use crate::cli::HatchArgs;

type Error = Box<dyn std::error::Error + Send + Sync>;

/// Each file stays well inside its share of the instruction context.
const FILE_LIMIT: usize = crate::CONTEXT_LIMIT / 2 - 128;
const SUMMARY_LIMIT: usize = 600;
const ANSWER_LIMIT: usize = 400;
pub(crate) const WISH_LIMIT: usize = 2000;
const FILES: [(&str, &str); 2] = [("KRAB.md", "KRAB.previous.md"), ("KLAW.md", "KLAW.previous.md")];
const MARKERS: [&str; 3] = ["=== KRAB.md ===", "=== KLAW.md ===", "=== SUMMARY ==="];

/// Explains what hatching needs when no intelligence provider is ready.
pub(crate) fn no_provider(directory: &Path) -> String {
    format!(
        "Hatching needs an intelligence provider, because your Krabb's model writes its \
         personality from your answers. Set one up first with `krabb plugin install openai` \
         and `krabb plugin setup openai`, or use `ollama` for a local model. Meanwhile, you can \
         edit your Krabb by hand in {}: KRAB.md says who it is, and KLAW.md says how it works.",
        directory.display(),
    )
}

/// One interview question, labelled for the model.
struct Question {
    label: &'static str,
    prompt: &'static str,
    example: &'static str,
}

const QUESTIONS: [Question; 8] = [
    Question { label: "Name", prompt: "What should your Krabb be called?", example: "Pinch" },
    Question { label: "Call the owner", prompt: "What should it call you?", example: "Sam" },
    Question {
        label: "Personality",
        prompt: "Describe its personality.",
        example: "warm, curious, a little playful, never sarcastic",
    },
    Question {
        label: "Tone of voice",
        prompt: "How should it sound?",
        example: "plain and direct, like a trusted colleague",
    },
    Question {
        label: "Writing",
        prompt: "How should it write replies?",
        example: "lead with the answer, short paragraphs, no emoji",
    },
    Question {
        label: "Way of working",
        prompt: "How should it get things done?",
        example: "act on clear requests, ask before anything irreversible",
    },
    Question {
        label: "Language",
        prompt: "Which language should it use?",
        example: "reply in the language I write in",
    },
    Question {
        label: "Anything else",
        prompt: "Anything else it should know about itself or you?",
        example: "I build embedded firmware and prefer Rust examples",
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
        return Err(format!("Each answer must be at most {limit} characters."));
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

/// Reads the current files; a missing, linked, or unreadable file reads as empty.
pub(crate) fn read(directory: &Path) -> Files {
    let load = |name: &str| match existing(&directory.join(name)) {
        Ok(Some(bytes)) => String::from_utf8_lossy(&bytes).into_owned(),
        _ => String::new(),
    };

    Files { krab: load(FILES[0].0), klaw: load(FILES[1].0) }
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
        if !text.starts_with(heading) {
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

/// Trims a part and drops a code fence the model may have wrapped around it.
fn unfence(text: &str) -> String {
    let text = text.trim();
    let text =
        text.strip_prefix("```markdown").or_else(|| text.strip_prefix("```")).unwrap_or(text);
    let text = text.strip_suffix("```").unwrap_or(text);

    text.trim().to_owned()
}

/// Saves FILES in DIRECTORY. When anything changes, both files are first backed up as they
/// were, so the backups always hold the state before the latest change; a missing file is
/// backed up as empty, which reads as absent. Returns whether anything changed. Every path
/// is checked before anything is written, and links or other non-files are refused.
pub(crate) fn save(directory: &Path, files: &Files) -> std::io::Result<bool> {
    let mut current = Vec::with_capacity(FILES.len());

    for (name, backup) in FILES {
        existing(&directory.join(backup))?;
        current.push(existing(&directory.join(name))?.unwrap_or_default());
    }

    let texts = files.texts();

    if current.iter().zip(texts).all(|(previous, text)| previous == text.as_bytes()) {
        return Ok(false);
    }

    for ((_, backup), previous) in FILES.iter().zip(&current) {
        krabb_file::save(directory.join(backup), previous)?;
    }

    for ((name, _), (previous, text)) in FILES.iter().zip(current.iter().zip(texts)) {
        if previous != text.as_bytes() {
            krabb_file::save(directory.join(name), text)?;
        }
    }

    Ok(true)
}

/// Swaps both files with their backups, so a second undo restores the newer version.
/// Returns the files whose text changed.
pub(crate) fn undo(directory: &Path) -> std::io::Result<Vec<&'static str>> {
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

        krabb_file::save(&backup, &current)?;
        krabb_file::save(&path, &previous)?;
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

        Ok(_) => std::fs::read(path).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Whether DIRECTORY holds a personality other than the one `init` writes.
pub(crate) fn hatched(directory: &Path) -> bool {
    let current = read(directory);

    current.krab != crate::templates::KRAB || current.klaw != crate::templates::KLAW
}

/// Asks each question on OUTPUT, reading replies from INPUT. An empty reply skips it.
pub(crate) fn interview(
    input: &mut impl BufRead,
    output: &mut impl Write,
    rerun: bool,
) -> Result<Vec<Option<String>>, Error> {
    let intro = if rerun {
        "Your Krabb already has a personality. Answer only what you want to change; press \
         Enter to keep the rest as it is."
    } else {
        "Let's hatch your Krabb. Answer in your own words and in any language; press Enter to \
         skip a question."
    };

    writeln!(output, "{intro}\n")?;

    let mut answers = Vec::with_capacity(QUESTIONS.len());

    for question in &QUESTIONS {
        answers.push(ask(input, output, question)?);
    }

    Ok(answers)
}

fn ask(
    input: &mut impl BufRead,
    output: &mut impl Write,
    question: &Question,
) -> Result<Option<String>, Error> {
    loop {
        write!(output, "{} (for example: {})\n> ", question.prompt, question.example)?;
        output.flush()?;

        let mut line = String::new();

        if input.read_line(&mut line)? == 0 {
            writeln!(output)?;
            return Ok(None);
        }

        match clean(&line, ANSWER_LIMIT) {
            Ok(answer) => {
                writeln!(output)?;
                return Ok(answer);
            }

            Err(problem) => writeln!(output, "{problem} Please try again.")?,
        }
    }
}

fn interactive() -> bool {
    std::env::var_os("CI").is_none()
        && std::io::stdin().is_terminal()
        && std::io::stderr().is_terminal()
}

pub(crate) async fn run(args: HatchArgs, root: &Path, json: bool) -> Result<(), Error> {
    let directory = root.join("data/system");

    if !directory.is_dir() {
        return Err("Krabb is not initialized. Run `krabb init` first.".into());
    }

    if args.undo {
        return restore(&directory, args.yes, json);
    }

    let config = crate::load_config_at(root)?;

    config.validate()?;

    let (provider, model) = crate::selection::client(&config, "cli", (None, None));

    let Ok(plugin) = crate::model_plugin_at(root, &provider) else {
        return Err(no_provider(&directory).into());
    };

    let given = [
        &args.name,
        &args.call,
        &args.personality,
        &args.voice,
        &args.writing,
        &args.work,
        &args.language,
        &args.notes,
    ];

    let mut answers = given
        .iter()
        .map(|value| value.as_deref().map_or(Ok(None), |value| clean(value, ANSWER_LIMIT)))
        .collect::<Result<Vec<_>, _>>()?;

    let free = args.wishes.join(" ");

    if free.chars().count() > WISH_LIMIT {
        return Err(format!("Wishes must be at most {WISH_LIMIT} characters.").into());
    }

    if !json && interactive() && answers.iter().all(Option::is_none) && free.trim().is_empty() {
        answers =
            interview(&mut std::io::stdin().lock(), &mut std::io::stderr(), hatched(&directory))?;
    }

    let Some(wishes) = wishes(&answers, &free) else {
        if json {
            println!("{}", serde_json::json!({"status": "unchanged"}));
        } else {
            println!(
                "Nothing to change. Answer at least one question, or describe the change: \
                 `krabb hatch be more concise`."
            );
        }

        return Ok(());
    };

    let current = read(&directory);
    let spinner = crate::terminal::progress("Your Krabb is writing its personality", json);
    let reply = crate::model_at(plugin, model, root, request("cli", &current, &wishes)).await;

    drop(spinner);

    let draft = parse(&reply?.text).map_err(|problem| {
        format!(
            "{problem} Nothing was changed. Run `krabb hatch` again, or edit the files in {} \
             by hand.",
            directory.display()
        )
    })?;

    if draft.files == current {
        report(json, "unchanged", &draft.summary);
        return Ok(());
    }

    if !json && !args.yes {
        eprintln!("{}\n{}\n{}\n", draft.files.krab, draft.files.klaw, draft.summary);
    }

    if !crate::agent::confirm_action(args.yes, json, "Save this personality?")? {
        crate::agent::print_cancelled("Hatching", json);
        return Ok(());
    }

    save(&directory, &draft.files)?;
    report(json, "hatched", &draft.summary);

    Ok(())
}

fn report(json: bool, status: &str, summary: &str) {
    if json {
        let report = serde_json::json!({"status": status, "summary": summary});

        println!("{report}");
        return;
    }

    println!("{summary}");

    if status == "hatched" {
        println!("It applies from your Krabb's next reply. Undo with `krabb hatch --undo`.");
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
        println!("Restored the previous {}. Undo again to switch back.", restored.join(" and "));
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
                "```markdown\n# Personality Instructions\n\nYou are Pinch.\n```",
                "# Behavioral Instructions\n\nAsk first.",
                "Ora ti chiami\nPinch.\n",
            )
        );

        let draft = parse(&reply).unwrap();

        assert_eq!(draft.files.krab, "# Personality Instructions\n\nYou are Pinch.\n");
        assert_eq!(draft.files.klaw, "# Behavioral Instructions\n\nAsk first.\n");
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

        let long = format!("{krab}\n{}", "x".repeat(FILE_LIMIT));

        assert!(parse(&draft(&long, klaw, "Done.")).unwrap_err().contains("longer"));
    }

    #[test]
    fn the_request_carries_the_hatching_prompt_current_files_and_wishes() {
        let current = Files {
            krab: "# Personality Instructions\nYou are Pinch.\n".into(),
            ..Files::default()
        };

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
        answers[6] = Some("italiano".into());

        assert_eq!(
            wishes(&answers, "be brief").unwrap(),
            "- Name: Pinch\n- Language: italiano\nbe brief"
        );
    }

    #[test]
    fn the_interview_keeps_answers_in_order_and_retries_invalid_ones() {
        let replies = format!("Pinch\n\n{}\nplayful\n", "x".repeat(ANSWER_LIMIT + 1));
        let mut output = Vec::new();

        let answers = interview(&mut replies.as_bytes(), &mut output, true).unwrap();
        let output = String::from_utf8(output).unwrap();

        assert_eq!(answers.len(), QUESTIONS.len());
        assert_eq!(answers[0].as_deref(), Some("Pinch"));
        assert_eq!(answers[1], None);
        assert_eq!(answers[2].as_deref(), Some("playful"));
        assert!(answers[3..].iter().all(Option::is_none));
        assert!(output.starts_with("Your Krabb already has a personality."));
        assert!(output.contains("at most 400 characters. Please try again."));
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
    fn saving_backs_up_changed_files_and_undo_swaps_them_back() {
        let directory = directory();
        let first = Files { krab: "krab one".into(), klaw: "klaw one".into() };
        let second = Files { krab: "krab two".into(), klaw: "klaw one".into() };

        assert!(undo(&directory).unwrap().is_empty());
        assert!(save(&directory, &first).unwrap());
        assert!(!save(&directory, &first).unwrap());
        assert!(save(&directory, &second).unwrap());
        assert_eq!(
            std::fs::read_to_string(directory.join("KLAW.previous.md")).unwrap(),
            "klaw one"
        );

        assert_eq!(read(&directory), second);

        assert_eq!(undo(&directory).unwrap(), ["KRAB.md"]);
        assert_eq!(read(&directory), first);
        assert_eq!(undo(&directory).unwrap(), ["KRAB.md"]);
        assert_eq!(read(&directory), second);

        let _ = std::fs::remove_dir_all(directory);
    }

    #[cfg(unix)]
    #[test]
    fn saving_refuses_linked_files_before_writing_anything() {
        let directory = directory();
        let outside = directory.join("outside.md");

        std::fs::write(&outside, "outside").unwrap();
        std::os::unix::fs::symlink(&outside, directory.join("KLAW.md")).unwrap();

        let files = Files { krab: "krab".into(), klaw: "klaw".into() };

        assert!(save(&directory, &files).is_err());
        assert!(undo(&directory).unwrap().is_empty());
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "outside");
        assert!(!directory.join("KRAB.md").exists());
        assert_eq!(read(&directory).klaw, "");

        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn without_a_provider_hatching_explains_setup_and_hand_editing() {
        let root = crate::tests::test_root("hatch-command");
        let error = run(parse_args(&["be", "brief"]), &root, true).await.unwrap_err();

        assert!(error.to_string().contains("krabb init"));

        crate::init_at(&root).unwrap();

        let error = run(parse_args(&["--name", "Pinch", "--yes"]), &root, true).await.unwrap_err();
        let message = error.to_string();
        let directory = root.join("data/system");

        assert!(message.contains("needs an intelligence provider"));
        assert!(message.contains("krabb plugin setup openai"));
        assert!(message.contains(&directory.display().to_string()));
        assert!(message.ends_with("KLAW.md says how it works."));
        assert!(!hatched(&directory));

        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn undo_restores_the_previous_personality_without_a_provider() {
        let root = crate::tests::test_root("hatch-undo");

        crate::init_at(&root).unwrap();

        let directory = root.join("data/system");
        let changed = Files { krab: "krab hatched".into(), klaw: crate::templates::KLAW.into() };

        save(&directory, &changed).unwrap();

        assert!(hatched(&directory));
        assert!(run(parse_args(&["--undo"]), &root, true).await.is_err());

        run(parse_args(&["--undo", "--yes"]), &root, true).await.unwrap();

        assert!(!hatched(&directory));

        let _ = std::fs::remove_dir_all(root);
    }
}
