//! Personalizes a Krabb: a short interview becomes its KRAB.md.
//!
//! Answers are kept as the person wrote them. The model reads them as instructions, so no
//! answer is parsed for keywords and any language works.

use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

use crate::cli::HatchArgs;

type Error = Box<dyn std::error::Error + Send + Sync>;

const NAME_LIMIT: usize = 40;
const ANSWER_LIMIT: usize = 400;
const DEFAULT_NAME: &str = "Krabb";
const BACKUP: &str = "KRAB.previous.md";

/// One interview question, where its answer goes in KRAB.md, and its starting answer.
struct Question {
    heading: &'static str,
    prompt: &'static str,
    example: &'static str,
    starter: Option<&'static str>,
}

const QUESTIONS: [Question; 7] = [
    Question {
        heading: "How To Address The Person You Work With",
        prompt: "What should it call you?",
        example: "Sam",
        starter: None,
    },
    Question {
        heading: "Personality",
        prompt: "Describe its personality.",
        example: "warm, curious, a little playful, never sarcastic",
        starter: Some("Warm, curious, and steady: a capable partner, not a servant."),
    },
    Question {
        heading: "Tone Of Voice",
        prompt: "How should it sound?",
        example: "plain and direct, like a trusted colleague",
        starter: Some("Plain and direct; friendly without filler."),
    },
    Question {
        heading: "Writing",
        prompt: "How should it write replies?",
        example: "lead with the answer, short paragraphs, no emoji",
        starter: Some(
            "Lead with the answer. Keep replies short, and use lists or code blocks only when \
             they help.",
        ),
    },
    Question {
        heading: "Way Of Working",
        prompt: "How should it get things done?",
        example: "act on clear requests, ask before anything irreversible",
        starter: Some(
            "Take requested steps without needless questions; ask when ambiguity changes the \
             outcome.",
        ),
    },
    Question {
        heading: "Language",
        prompt: "Which language should it use?",
        example: "reply in the language I write in",
        starter: Some("Reply in the language the person writes in."),
    },
    Question {
        heading: "Anything Else",
        prompt: "Anything else it should know about itself or you?",
        example: "I build embedded firmware and prefer Rust examples",
        starter: None,
    },
];

/// A Krabb's personality: a name plus one optional answer per question.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Profile {
    pub(crate) name: Option<String>,
    pub(crate) answers: [Option<String>; QUESTIONS.len()],
}

impl Profile {
    pub(crate) fn from_args(args: &HatchArgs) -> Result<Self, String> {
        let given = [
            &args.call,
            &args.personality,
            &args.voice,
            &args.writing,
            &args.work,
            &args.language,
            &args.notes,
        ];

        let mut profile =
            Self { name: clean(args.name.as_deref(), NAME_LIMIT)?, ..Self::default() };

        for (slot, value) in profile.answers.iter_mut().zip(given) {
            *slot = clean(value.as_deref(), ANSWER_LIMIT)?;
        }

        Ok(profile)
    }

    /// Fills every unanswered question with its starting answer.
    pub(crate) fn completed(mut self) -> Self {
        for (question, slot) in QUESTIONS.iter().zip(self.answers.iter_mut()) {
            if slot.is_none() {
                *slot = question.starter.map(str::to_owned);
            }
        }

        self
    }

    pub(crate) fn name(&self) -> &str {
        self.name.as_deref().unwrap_or(DEFAULT_NAME)
    }
}

/// Trims an answer and rejects one that is too long or spans several lines.
fn clean(value: Option<&str>, limit: usize) -> Result<Option<String>, String> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };

    if value.chars().any(char::is_control) {
        return Err("Each answer must be a single line of text.".into());
    }

    if value.chars().count() > limit {
        return Err(format!("Each answer must be at most {limit} characters."));
    }

    Ok(Some(value.to_owned()))
}

/// Writes PROFILE as KRAB.md, the instructions Krabb reads on every turn.
pub(crate) fn render(profile: &Profile) -> String {
    let name = profile.name();

    let mut text = format!(
        "# Personality Instructions\n\n## Name\n\nYou are {name}. Use this name when you \
         introduce yourself.\n"
    );

    for (question, answer) in QUESTIONS.iter().zip(&profile.answers) {
        if let Some(answer) = answer {
            text.push_str(&format!("\n## {}\n\n{answer}\n", question.heading));
        }
    }

    text
}

/// Asks for every answer PROFILE lacks, on OUTPUT, reading replies from INPUT. An empty
/// reply keeps the starting answer, if the question has one.
pub(crate) fn interview(
    input: &mut impl BufRead,
    output: &mut impl Write,
    mut profile: Profile,
) -> Result<Profile, Error> {
    writeln!(
        output,
        "Let's hatch your Krabb. Answer in your own words and in any language. Press Enter to \
         keep the default or skip a question.\n"
    )?;

    if profile.name.is_none() {
        let question = format!("What should your Krabb be called? (default: {DEFAULT_NAME})");

        profile.name = ask(input, output, &question, NAME_LIMIT)?;
    }

    for (question, slot) in QUESTIONS.iter().zip(profile.answers.iter_mut()) {
        if slot.is_some() {
            continue;
        }

        let hint = question.starter.map_or_else(
            || format!("for example: {}", question.example),
            |starter| format!("default: {starter}"),
        );

        *slot = ask(input, output, &format!("{} ({hint})", question.prompt), ANSWER_LIMIT)?;
    }

    Ok(profile)
}

fn ask(
    input: &mut impl BufRead,
    output: &mut impl Write,
    question: &str,
    limit: usize,
) -> Result<Option<String>, Error> {
    loop {
        write!(output, "{question}\n> ")?;
        output.flush()?;

        let mut line = String::new();

        if input.read_line(&mut line)? == 0 {
            writeln!(output)?;
            return Ok(None);
        }

        match clean(Some(line.trim_end_matches(['\r', '\n'])), limit) {
            Ok(answer) => {
                writeln!(output)?;
                return Ok(answer);
            }

            Err(problem) => writeln!(output, "{problem} Please try again.")?,
        }
    }
}

/// Saves TEXT as the KRAB.md in DIRECTORY and returns the backup of a different previous
/// version, if any. Refuses to follow a symbolic link or to replace anything but a file.
pub(crate) fn save(directory: &Path, text: &str) -> std::io::Result<Option<PathBuf>> {
    let path = directory.join("KRAB.md");

    let previous = match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("KRAB.md must be a regular file: {}.", path.display()),
            ));
        }

        Ok(_) => Some(std::fs::read(&path)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };

    let backup = match previous {
        Some(bytes) if bytes != text.as_bytes() => {
            let backup = directory.join(BACKUP);

            krabb_file::save(&backup, &bytes)?;
            Some(backup)
        }

        _ => None,
    };

    krabb_file::save(&path, text)?;

    Ok(backup)
}

fn interactive() -> bool {
    std::env::var_os("CI").is_none()
        && std::io::stdin().is_terminal()
        && std::io::stderr().is_terminal()
}

pub(crate) fn run(args: HatchArgs, root: &Path, json: bool) -> Result<(), Error> {
    let directory = root.join("data/system");

    if !directory.is_dir() {
        return Err("Krabb is not initialized. Run `krabb init` first.".into());
    }

    let mut profile = Profile::from_args(&args)?;

    if !json && interactive() {
        profile = interview(&mut std::io::stdin().lock(), &mut std::io::stderr(), profile)?;
    }

    let profile = profile.completed();
    let text = render(&profile);
    let path = directory.join("KRAB.md");

    if !json && !args.yes {
        eprintln!("{text}");
    }

    let question = format!("Save {}'s personality to {}?", profile.name(), path.display());

    if !crate::agent::confirm_action(args.yes, json, &question)? {
        crate::agent::print_cancelled("Hatching", json);
        return Ok(());
    }

    let backup = save(&directory, &text)?;

    if json {
        let report = serde_json::json!({
            "status": "hatched",
            "name": profile.name(),
            "path": path,
            "backup": backup,
        });

        println!("{report}");
        return Ok(());
    }

    println!("{} has hatched and takes on this personality from its next reply.", profile.name());
    println!("Personality: {}", path.display());

    if let Some(backup) = backup {
        println!("Previous personality: {}", backup.display());
    }

    println!("Edit that file anytime, or run `krabb hatch` again.");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory() -> PathBuf {
        let path = crate::tests::test_root("hatch");

        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn the_starter_personality_is_the_krab_template() {
        assert_eq!(render(&Profile::default().completed()), crate::templates::KRAB);
    }

    #[test]
    fn answers_become_sections_in_the_order_asked() {
        let mut profile = Profile { name: Some("Pinch".into()), ..Profile::default() };

        profile.answers[0] = Some("Sam".into());
        profile.answers[2] = Some("Calma e diretta.".into());

        let text = render(&profile);

        assert!(text.starts_with("# Personality Instructions\n\n## Name\n\nYou are Pinch."));
        assert!(text.contains("## How To Address The Person You Work With\n\nSam\n"));
        assert!(text.contains("## Tone Of Voice\n\nCalma e diretta.\n"));
        assert!(!text.contains("## Personality\n"));
        assert!(text.find("Address").unwrap() < text.find("Tone Of Voice").unwrap());
    }

    #[test]
    fn the_interview_asks_only_what_is_missing_and_retries_invalid_answers() {
        let mut profile = Profile::default();

        profile.answers[5] = Some("English".into());

        let replies = format!("Pinch\n\n{}\nplayful\n\n\n\n\n", "x".repeat(ANSWER_LIMIT + 1));
        let mut output = Vec::new();

        let profile = interview(&mut replies.as_bytes(), &mut output, profile).unwrap();
        let output = String::from_utf8(output).unwrap();

        assert_eq!(profile.name.as_deref(), Some("Pinch"));
        assert_eq!(profile.answers[0], None);
        assert_eq!(profile.answers[1].as_deref(), Some("playful"));
        assert_eq!(profile.answers[5].as_deref(), Some("English"));
        assert!(!output.contains("Which language"));
        assert!(output.contains("at most 400 characters. Please try again."));
        assert!(output.contains("(for example: Sam)"));
        assert!(output.contains("(default: Plain and direct; friendly without filler.)"));

        let completed = profile.completed();

        assert_eq!(completed.answers[2].as_deref(), QUESTIONS[2].starter);
        assert_eq!(completed.answers[0], None);
    }

    #[test]
    fn closed_input_ends_the_interview_with_defaults() {
        let profile = interview(&mut "".as_bytes(), &mut Vec::new(), Profile::default()).unwrap();

        assert_eq!(profile, Profile::default());
        assert_eq!(profile.name(), "Krabb");
    }

    #[test]
    fn answers_are_single_bounded_lines() {
        assert_eq!(clean(Some("  calm  "), 10), Ok(Some("calm".into())));
        assert_eq!(clean(Some("   "), 10), Ok(None));
        assert!(clean(Some("one\ntwo"), 10).is_err());
        assert!(clean(Some("toolongname"), 10).is_err());
        assert_eq!(clean(Some("ééééé"), 5), Ok(Some("ééééé".into())));
    }

    fn parse(flags: &[&str]) -> HatchArgs {
        use clap::Parser;

        let arguments = ["krabb", "hatch"].iter().chain(flags);

        match crate::cli::Cli::try_parse_from(arguments).unwrap().command {
            crate::cli::Command::Hatch(args) => args,
            command => panic!("parsed {command:?}"),
        }
    }

    #[test]
    fn the_command_saves_flags_without_asking_and_backs_up_the_last_personality() {
        let root = crate::tests::test_root("hatch-command");

        assert!(
            run(parse(&["--yes"]), &root, true).unwrap_err().to_string().contains("krabb init")
        );

        crate::init_at(&root).unwrap();

        let flags = ["--name", "Pinch", "--voice", "Calma e diretta.", "--call", "Sam", "--yes"];

        run(parse(&flags), &root, true).unwrap();

        let path = root.join("data/system/KRAB.md");
        let text = std::fs::read_to_string(&path).unwrap();

        assert!(text.contains("You are Pinch."));
        assert!(text.contains("## Tone Of Voice\n\nCalma e diretta.\n"));
        assert!(text.contains("## How To Address The Person You Work With\n\nSam\n"));
        assert!(text.contains(QUESTIONS[1].starter.unwrap()));

        let backup = root.join("data/system").join(BACKUP);

        assert_eq!(std::fs::read_to_string(&backup).unwrap(), crate::templates::KRAB);

        assert!(run(parse(&["--name", "Nipper"]), &root, true).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        assert!(parse(&["--name", "a\nb", "--yes"]).name.is_some());
        assert!(run(parse(&["--name", "a\nb", "--yes"]), &root, true).is_err());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn saving_backs_up_a_different_personality_and_refuses_links() {
        let directory = directory();

        assert_eq!(save(&directory, "first").unwrap(), None);
        assert_eq!(save(&directory, "first").unwrap(), None);

        let backup = save(&directory, "second").unwrap().unwrap();

        assert_eq!(std::fs::read_to_string(&backup).unwrap(), "first");
        assert_eq!(std::fs::read_to_string(directory.join("KRAB.md")).unwrap(), "second");

        #[cfg(unix)]
        {
            let outside = directory.join("outside.md");

            std::fs::write(&outside, "outside").unwrap();
            std::fs::remove_file(directory.join("KRAB.md")).unwrap();
            std::os::unix::fs::symlink(&outside, directory.join("KRAB.md")).unwrap();

            assert!(save(&directory, "third").is_err());
            assert_eq!(std::fs::read_to_string(&outside).unwrap(), "outside");
        }

        let _ = std::fs::remove_dir_all(directory);
    }
}
