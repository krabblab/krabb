//! Paces short updates while long work runs, from model milestones or host tool activity.

use super::*;

pub(crate) const NAME: &str = "progress";
/// Longest milestone line, in bytes.
pub(crate) const LIMIT: usize = 200;
/// Most updates in one turn or task step, the routing acknowledgement included.
pub(crate) const CAP: u32 = 5;
const LANGUAGE_LIMIT: usize = 35;
const THOUGHTFUL_FIRST: Duration = Duration::from_secs(20);
const THOUGHTFUL_NEXT: Duration = Duration::from_secs(45);
const TASK_FIRST: Duration = Duration::from_secs(3 * 60);
const TASK_FLOOR: Duration = Duration::from_secs(2 * 60);
/// The step length assumed for a task when no estimate is known.
const TASK_EXPECTED: Duration = Duration::from_secs(30 * 60);
/// The share of the expected duration between task updates.
const SHARE: u32 = 10;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Updates {
    Off,
    #[default]
    Quiet,
    Frequent,
}

/// The `updates` setting of a channel or client, falling back to the top-level default.
pub(crate) fn setting(config: &Config, channel: &str) -> Updates {
    let own = match sharing::principal(channel) {
        sharing::Principal::Channel(id) => config.channels.get(&id).and_then(|value| value.updates),
        sharing::Principal::Client(id) => config.clients.get(&id).and_then(|value| value.updates),
        sharing::Principal::Agent(_) => None,
    };

    own.unwrap_or(config.updates)
}

/// The silence before the next update, or none when no further update may follow.
///
/// `elapsed` is the work time when the silence starts, so work running past its estimate
/// waits longer between updates.
pub(crate) fn gap(
    kind: routing::Kind,
    updates: Updates,
    sent: u32,
    elapsed: Duration,
    expected: Option<Duration>,
) -> Option<Duration> {
    let scale = match updates {
        _ if sent >= CAP => return None,
        Updates::Off => return None,
        Updates::Quiet => 1,
        Updates::Frequent => 2,
    };

    match kind {
        routing::Kind::Fast => None,
        routing::Kind::Thoughtful if sent == 0 => Some(THOUGHTFUL_FIRST / scale),
        routing::Kind::Thoughtful => Some(THOUGHTFUL_NEXT.max(share(expected, elapsed)) / scale),

        routing::Kind::Background => {
            let gap = share(expected.or(Some(TASK_EXPECTED)), elapsed);
            let gap = if sent == 0 { gap.max(TASK_FIRST) } else { gap };

            Some((gap / scale).max(TASK_FLOOR))
        }
    }
}

fn share(expected: Option<Duration>, elapsed: Duration) -> Duration {
    expected.unwrap_or_default().max(elapsed) / SHARE
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Language {
    English,
    Italian,
    Spanish,
    French,
    German,
    Portuguese,
}

impl Language {
    fn parse(tag: &str) -> Option<Self> {
        let tag = tag.trim();

        if tag.is_empty()
            || tag.len() > LANGUAGE_LIMIT
            || !tag.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return None;
        }

        let primary = tag.split(['-', '_']).next()?.to_ascii_lowercase();

        match primary.as_str() {
            "en" | "eng" => Some(Self::English),
            "it" | "ita" => Some(Self::Italian),
            "es" | "spa" => Some(Self::Spanish),
            "fr" | "fra" | "fre" => Some(Self::French),
            "de" | "deu" | "ger" => Some(Self::German),
            "pt" | "por" => Some(Self::Portuguese),
            _ => None,
        }
    }

    fn words(self) -> &'static Words {
        match self {
            Self::English => &ENGLISH,
            Self::Italian => &ITALIAN,
            Self::Spanish => &SPANISH,
            Self::French => &FRENCH,
            Self::German => &GERMAN,
            Self::Portuguese => &PORTUGUESE,
        }
    }
}

struct Words {
    idle: &'static str,
    lead: &'static str,
    pages: [&'static str; 2],
    files: [&'static str; 2],
    changes: [&'static str; 2],
    commands: [&'static str; 2],
    steps: [&'static str; 2],
}

const ENGLISH: Words = Words {
    idle: "Still working.",
    lead: "Still working: ",
    pages: ["page read", "pages read"],
    files: ["file read", "files read"],
    changes: ["file changed", "files changed"],
    commands: ["command run", "commands run"],
    steps: ["other step", "other steps"],
};

const ITALIAN: Words = Words {
    idle: "Sto ancora lavorando.",
    lead: "Sto ancora lavorando: ",
    pages: ["pagina letta", "pagine lette"],
    files: ["file letto", "file letti"],
    changes: ["file modificato", "file modificati"],
    commands: ["comando eseguito", "comandi eseguiti"],
    steps: ["altro passaggio", "altri passaggi"],
};

const SPANISH: Words = Words {
    idle: "Sigo trabajando.",
    lead: "Sigo trabajando: ",
    pages: ["página leída", "páginas leídas"],
    files: ["archivo leído", "archivos leídos"],
    changes: ["archivo modificado", "archivos modificados"],
    commands: ["comando ejecutado", "comandos ejecutados"],
    steps: ["paso más", "pasos más"],
};

const FRENCH: Words = Words {
    idle: "Toujours en cours.",
    lead: "Toujours en cours : ",
    pages: ["page lue", "pages lues"],
    files: ["fichier lu", "fichiers lus"],
    changes: ["fichier modifié", "fichiers modifiés"],
    commands: ["commande exécutée", "commandes exécutées"],
    steps: ["autre étape", "autres étapes"],
};

const GERMAN: Words = Words {
    idle: "Ich arbeite noch.",
    lead: "Ich arbeite noch: ",
    pages: ["Seite gelesen", "Seiten gelesen"],
    files: ["Datei gelesen", "Dateien gelesen"],
    changes: ["Datei geändert", "Dateien geändert"],
    commands: ["Befehl ausgeführt", "Befehle ausgeführt"],
    steps: ["weiterer Schritt", "weitere Schritte"],
};

const PORTUGUESE: Words = Words {
    idle: "Ainda trabalhando.",
    lead: "Ainda trabalhando: ",
    pages: ["página lida", "páginas lidas"],
    files: ["arquivo lido", "arquivos lidos"],
    changes: ["arquivo alterado", "arquivos alterados"],
    commands: ["comando executado", "comandos executados"],
    steps: ["outra etapa", "outras etapas"],
};

/// Completed tool calls of the current turn or step, by what they did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Activity {
    pages: u32,
    files: u32,
    changes: u32,
    commands: u32,
    steps: u32,
}

impl Activity {
    fn record(&mut self, name: &str) {
        let count = match name {
            "web_fetch" | "web_search" | "browser_open" | "browser_snapshot" => &mut self.pages,
            "file_read" | "file_list" | "file_search" => &mut self.files,
            "file_write" | "file_patch" => &mut self.changes,
            "shell_run" | "git_run" => &mut self.commands,
            name if HOST_TOOLS.contains(&name) => return,
            _ => &mut self.steps,
        };

        *count = count.saturating_add(1);
    }
}

/// When the current turn or task step last sent an update, and what it has done since starting.
#[derive(Clone, Debug)]
pub(crate) struct Pace {
    updates: Updates,
    sent: u32,
    started: tokio::time::Instant,
    last: tokio::time::Instant,
    expected: Option<Duration>,
    waiting: u32,
    streaming: bool,
    language: Option<Language>,
    activity: Activity,
}

impl Default for Pace {
    fn default() -> Self {
        Self::new(Updates::default())
    }
}

impl Pace {
    pub(crate) fn new(updates: Updates) -> Self {
        let now = tokio::time::Instant::now();

        Self {
            updates,
            sent: 0,
            started: now,
            last: now,
            expected: None,
            waiting: 0,
            streaming: false,
            language: None,
            activity: Activity::default(),
        }
    }

    fn gap(&self, kind: routing::Kind) -> Option<Duration> {
        gap(kind, self.updates, self.sent, self.last - self.started, self.expected)
    }

    /// When the host should speak if nothing else is said, or none while waiting on a person
    /// or while the model streams text that may be the answer.
    pub(crate) fn due(&self, kind: routing::Kind) -> Option<tokio::time::Instant> {
        if self.waiting > 0 || self.streaming {
            return None;
        }

        self.gap(kind).map(|gap| self.last + gap)
    }

    /// Whether a model milestone may go out now: at least half the current gap after the last.
    pub(crate) fn allows(&self, kind: routing::Kind, now: tokio::time::Instant) -> bool {
        self.waiting == 0
            && !self.streaming
            && self.gap(kind).is_some_and(|gap| now >= self.last + gap / 2)
    }

    /// Whether the routing acknowledgement may go out as the first update.
    pub(crate) fn opens(&self) -> bool {
        self.updates != Updates::Off && self.sent == 0
    }

    pub(crate) fn mark(&mut self, now: tokio::time::Instant) {
        self.sent = self.sent.saturating_add(1);
        self.last = now;
    }

    /// Counts a completed tool call without changing when the next update is due.
    pub(crate) fn record(pace: &watch::Sender<Self>, name: &str) {
        pace.send_if_modified(|pace| {
            pace.activity.record(name);
            std::mem::take(&mut pace.streaming)
        });
    }

    /// Holds host lines while the current generation streams text.
    pub(crate) fn stream(pace: &watch::Sender<Self>, text: &str) {
        pace.send_if_modified(|pace| {
            let streaming = !text.is_empty();
            let changed = pace.streaming != streaming;

            pace.streaming = streaming;
            changed
        });
    }

    /// Keeps a known conversation language for host lines; reports whether it changed.
    pub(crate) fn speak(&mut self, tag: &str) -> bool {
        let Some(language) = Language::parse(tag) else { return false };

        let changed = self.language != Some(language);

        self.language = Some(language);
        changed
    }

    pub(crate) fn expect(&mut self, expected: Duration) -> bool {
        let changed = self.expected != Some(expected);

        self.expected = Some(expected);
        changed
    }

    fn hold(&mut self) {
        self.waiting = self.waiting.saturating_add(1);
    }

    fn resume(&mut self) {
        self.waiting = self.waiting.saturating_sub(1);
        self.last = self.last.max(tokio::time::Instant::now());
    }

    /// A short factual line from tool activity, in the conversation's language when known.
    pub(crate) fn line(&self) -> String {
        let words = self.language.unwrap_or(Language::English).words();

        let parts = [
            (self.activity.pages, words.pages),
            (self.activity.files, words.files),
            (self.activity.changes, words.changes),
            (self.activity.commands, words.commands),
            (self.activity.steps, words.steps),
        ]
        .into_iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, [one, many])| format!("{count} {}", if count == 1 { one } else { many }))
        .collect::<Vec<_>>();

        if parts.is_empty() {
            words.idle.into()
        } else {
            format!("{}{}.", words.lead, parts.join(", "))
        }
    }
}

/// Holds updates while the turn waits on a person, then restarts the silence.
pub(crate) struct Waiting<'a>(&'a watch::Sender<Pace>);

impl<'a> Waiting<'a> {
    pub(crate) fn new(pace: &'a watch::Sender<Pace>) -> Self {
        pace.send_modify(Pace::hold);
        Self(pace)
    }
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.send_modify(Pace::resume);
    }
}

/// A model milestone, bounded to one short line.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Milestone {
    pub message: String,
    #[serde(default)]
    pub language: Option<String>,
}

impl Milestone {
    pub(crate) fn parse(value: Value) -> Result<Self, PluginError> {
        let mut milestone: Self = serde_json::from_value(value)?;

        milestone.message = milestone.message.trim().into();

        if milestone.message.is_empty()
            || milestone.message.len() > LIMIT
            || milestone.message.chars().any(char::is_control)
        {
            return Err("An update must be one short line of at most 200 bytes.".into());
        }

        Ok(milestone)
    }
}

/// Paces a model milestone and reports whether it goes out now.
pub(crate) fn milestone(route: &routing::State, milestone: &Milestone) -> bool {
    let kind = *route.kind.borrow();
    let now = tokio::time::Instant::now();
    let mut sent = false;

    route.pace.send_if_modified(|pace| {
        let spoke = milestone.language.as_deref().is_some_and(|tag| pace.speak(tag));

        if !pace.allows(kind, now) {
            return spoke;
        }

        pace.mark(now);
        sent = true;
        true
    });

    sent
}

/// Counts the routing acknowledgement as the first update; false when updates are off.
pub(crate) fn open(route: &routing::State, language: Option<&str>) -> bool {
    let now = tokio::time::Instant::now();
    let mut opened = false;

    route.pace.send_if_modified(|pace| {
        let spoke = language.is_some_and(|tag| pace.speak(tag));

        if !pace.opens() {
            return spoke;
        }

        pace.mark(now);
        opened = true;
        true
    });

    opened
}

pub(crate) fn tool() -> ToolSpec {
    ToolSpec {
        name: NAME.into(),
        description: Some(
            "Tell the person waiting about a real milestone while long work continues, such as \
                \"Found three sources, now comparing prices\". Write one short line in the \
                conversation's language, only for real progress, never for routine steps or the \
                answer itself. Optionally name the language as a tag such as en or it. The host \
                paces updates and may skip one; continue the work either way."
                .into(),
        ),
        schema: serde_json::json!({
            "type": "object",
            "properties": {
                "message": {"type": "string"},
                "language": {"type": "string"}
            },
            "required": ["message"],
            "additionalProperties": false
        }),
    }
}

/// Whether the `progress` tool is offered: on thoughtful turns and task steps with updates on.
pub(crate) fn offered(pace: &Pace, kind: routing::Kind) -> bool {
    pace.updates != Updates::Off && kind != routing::Kind::Fast
}

/// Runs the work and sends a host line from tool activity after each silence, until it ends.
pub(crate) async fn pace<T>(
    work: impl std::future::Future<Output = T>,
    route: &routing::State,
    notices: &tokio::sync::mpsc::Sender<StreamNotice>,
) -> T {
    tokio::pin!(work);

    let mut pace = route.pace.subscribe();
    let mut kind = route.kind.subscribe();

    loop {
        let current = *kind.borrow_and_update();
        let due = pace.borrow_and_update().due(current);

        tokio::select! {
            biased;
            result = &mut work => return result,

            _ = pace.changed() => {}

            _ = kind.changed() => {}

            _ = tokio::time::sleep_until(due.unwrap_or_else(tokio::time::Instant::now)),
                if due.is_some() =>
            {
                let now = tokio::time::Instant::now();
                let mut line = None;

                route.pace.send_if_modified(|pace| {
                    if pace.due(current).is_none_or(|due| due > now) {
                        return false;
                    }

                    pace.mark(now);
                    line = Some(pace.line());
                    true
                });

                if let Some(line) = line {
                    let _ = notices.try_send(StreamNotice::Status(line));
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "progress/tests.rs"]
mod tests;
