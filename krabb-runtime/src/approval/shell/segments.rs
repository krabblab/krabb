//! Splits a shell command into simple commands and their output redirections.
//!
//! The split follows POSIX shells, cmd, and PowerShell closely enough to name programs and
//! targets. A backslash escapes only a character the shell treats specially, so Windows paths
//! such as `C:\build\out` and shares such as `\\server\share` keep their backslashes.

#[derive(Default)]
pub(super) struct Segment {
    pub words: Vec<String>,
    pub outputs: Vec<String>,
}

#[derive(Clone, Copy, Default, PartialEq)]
enum Next {
    #[default]
    Word,
    Output,
    Skip,
}

#[derive(Default)]
struct Split {
    segments: Vec<Segment>,
    word: String,
    started: bool,
    next: Next,
}

pub(super) fn segments(command: &str) -> Vec<Segment> {
    let mut split = Split { segments: vec![Segment::default()], ..Split::default() };
    let mut quote = None;
    let mut heredoc = false;
    let mut characters = command.chars().peekable();

    while let Some(character) = characters.next() {
        match (quote, character) {
            (Some(open), character) if character == open => quote = None,

            (None, '\\') if !split.started && characters.next_if_eq(&'\\').is_some() => {
                split.word.push_str("\\\\");
                split.started = true;
            }

            (Some('"') | None, '\\') if characters.peek().is_some_and(|next| special(*next)) => {
                split.word.extend(characters.next());
                split.started = true;
            }

            (Some(_), character) => split.word.push(character),

            (None, '\'' | '"') => {
                quote = Some(character);
                split.started = true;
            }

            (None, ' ' | '\t') => {
                if split.started {
                    split.finish();
                    split.next = Next::Word;
                }
            }

            (None, '\n') if heredoc => break,

            (None, ';' | '&' | '|' | '\n' | '\r' | '(' | ')' | '`') => {
                split.finish();
                split.next = Next::Word;

                if split.segments.last().is_some_and(|segment| !segment.words.is_empty()) {
                    split.segments.push(Segment::default());
                }
            }

            (None, '>' | '<') => {
                if split.word.bytes().all(|byte| byte.is_ascii_digit()) {
                    split.word.clear();
                    split.started = false;
                }

                split.finish();

                heredoc |= character == '<' && characters.next_if_eq(&'<').is_some();
                characters.next_if_eq(&'>');
                split.next = if character == '<' { Next::Skip } else { Next::Output };

                if characters.next_if_eq(&'&').is_some() {
                    split.next = Next::Skip;
                }
            }

            (None, character) => {
                split.word.push(character);
                split.started = true;
            }
        }
    }

    split.finish();
    split.segments
}

impl Split {
    fn finish(&mut self) {
        if self.started
            && let Some(segment) = self.segments.last_mut()
        {
            match self.next {
                Next::Word => segment.words.push(std::mem::take(&mut self.word)),
                Next::Output => segment.outputs.push(std::mem::take(&mut self.word)),
                Next::Skip => self.word.clear(),
            }
        }

        self.started = false;
    }
}

fn special(character: char) -> bool {
    " \t\n\"'\\$`;&|<>(){}*?![]#~".contains(character)
}
