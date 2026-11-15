use super::*;

pub(super) const ERROR: &str = "I've encountered an error while answering, please retry.";
pub(super) const FAST_LIMIT: Duration = Duration::from_secs(60);
pub(super) const STEP_LIMIT: Duration = Duration::from_secs(10 * 60);
/// Longest contextual acknowledgement a model may send while work continues.
pub(super) const MESSAGE_LIMIT: usize = 2_000;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Kind {
    #[default]
    Fast,
    Thoughtful,
    Background,
}

impl Kind {
    pub fn limit(self) -> Duration {
        match self {
            Self::Fast => FAST_LIMIT,
            Self::Thoughtful => TURN_LIMIT,
            Self::Background => STEP_LIMIT,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Task {
    pub kind: Kind,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub instruction: String,
}

impl Task {
    pub fn parse(value: Value) -> Result<Self, PluginError> {
        let mut task: Self = serde_json::from_value(value)?;

        task.message = task.message.trim().into();
        task.instruction = task.instruction.trim().into();

        if task.kind != Kind::Fast
            && (task.message.is_empty() || task.message.len() > MESSAGE_LIMIT)
        {
            return Err("A contextual progress message of at most 2,000 bytes is required.".into());
        }

        if task.kind == Kind::Background
            && (task.instruction.is_empty() || task.instruction.len() > TEXT_LIMIT)
        {
            return Err("A complete background task instruction is required.".into());
        }

        Ok(task)
    }
}

#[derive(Debug)]
pub(super) struct State {
    pub handoff: watch::Sender<Option<String>>,
    pub kind: watch::Sender<Kind>,
    pub gate: AsyncMutex<()>,
    pub announced: std::sync::atomic::AtomicBool,
    pub background: std::sync::atomic::AtomicBool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            handoff: watch::channel(None).0,
            kind: watch::channel(Kind::Fast).0,
            gate: AsyncMutex::new(()),
            announced: std::sync::atomic::AtomicBool::new(false),
            background: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

pub(super) fn tool(background: bool) -> ToolSpec {
    ToolSpec {
        name: "task".into(),
        description: Some(format!(
            "Classify work before doing it. Fast is the default: answer greetings and readily \
                available questions directly without calling this tool or announcing work. \
                Thoughtful work needing research, verification, nontrivial calculations or \
                generation needs a contextual acknowledgement: call with kind thoughtful and one \
                brief contextual message in the user's language. For substantial autonomous work \
                requiring multiple stages, choose background with a contextual acknowledgement and \
                a self-contained goal before side effects. The host saves a durable job, continues \
                it in checkpointed steps without an overall lifetime deadline, and delivers \
                important updates and completion automatically. No scheduler plugin is needed. Do \
                not promise a handoff until this tool succeeds. Background execution is {}. Do not \
                use this tool inside a background or scheduled job.",
            if background {
                "available under the configured approval policy"
            } else {
                "unavailable under this conversation's policy; explain limits honestly"
            },
        )),
        schema: serde_json::json!({
            "type": "object",
            "properties": {
                "kind": {"type": "string", "enum": if background {
                    vec!["fast", "thoughtful", "background"]
                } else { vec!["fast", "thoughtful"] }},
                "message": {"type": "string"},
                "instruction": {"type": "string"}
            },
            "required": ["kind"],
            "additionalProperties": false
        }),
    }
}

pub(super) fn context(session: &str) -> Message {
    Message {
        id: "krabb-task".into(),
        session: session.into(),
        role: Role::System,
        sender: None,
        content: vec![Content::Text {
            text: "Classify the current request by the work it needs, not by provider latency. \
                Fast requests such as greetings and simple questions with readily available \
                answers need only the answer, with no waiting message. Thoughtful requests need \
                research, fact checking, calculations, or generation: use the task tool to provide \
                one brief contextual progress message in the user's language, then do the work. \
                Substantial requests requiring multiple stages should use the task tool's \
                background mode before starting work when available; the host delivers the \
                finished result automatically. Do not invent a background job or promise later \
                delivery when a handoff fails or is unavailable. Do not add canned \
                acknowledgements to fast answers."
                .into(),
        }],
    }
}

/// The host clock, appended to the newest request so earlier turns keep a stable prefix.
pub(super) fn clock() -> Content {
    Content::Text {
        text: format!(
            "Current host time: {}. Resolve relative dates and times from it.",
            krabb_date::format_datetime_local(unix_seconds())
        ),
    }
}

/// Adds the host time to a request; a full message carries it in its last text instead, so the
/// request keeps within the content limit, and a full message without text keeps its content.
pub(super) fn stamp(request: &mut Message) {
    let Content::Text { text: clock } = clock() else { return };

    if request.content.len() < CONTENT_LIMIT {
        request.content.push(Content::Text { text: clock });
        return;
    }

    if let Some(text) = request.content.iter_mut().rev().find_map(|item| match item {
        Content::Text { text } => Some(text),
        _ => None,
    }) {
        text.push_str("\n\n");
        text.push_str(&clock);
    }
}

pub(super) fn reply(text: String) -> ModelReply {
    ModelReply {
        timing: None,
        text,
        stop: "stop".into(),
        input: None,
        output: None,
        context_usage: None,
        events: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_request_carries_the_host_time_without_another_item() {
        let mut request = Message {
            id: "request".into(),
            session: "session".into(),
            role: Role::User,
            sender: None,
            content: vec![Content::Text { text: "Hello.".into() }],
        };

        stamp(&mut request);

        assert_eq!(request.content.len(), 2);

        request.content = vec![Content::Text { text: "Hello.".into() }; CONTENT_LIMIT];
        stamp(&mut request);

        assert_eq!(request.content.len(), CONTENT_LIMIT);
        assert!(matches!(request.content.last(),
            Some(Content::Text { text }) if text.starts_with("Hello.\n\nCurrent host time:")));
    }

    use serde_json::json;

    #[test]
    fn task_modes_require_context_without_accepting_destinations() {
        assert_eq!(Task::parse(json!({"kind":"fast"})).unwrap().kind, Kind::Fast);

        let task = Task::parse(json!({
            "kind":"thoughtful", "message":"  I'll verify today's releases.  "
        }))
        .unwrap();

        assert_eq!(task.message, "I'll verify today's releases.");

        for invalid in [
            json!({"kind":"unknown"}),
            json!({"kind":"thoughtful"}),
            json!({"kind":"thoughtful", "message":" "}),
            json!({"kind":"thoughtful", "message":"x".repeat(2_001)}),
            json!({"kind":"background", "message":"I'll prepare the report."}),
            json!({"kind":"background", "message":"I'll prepare it.", "instruction":" "}),
            json!({"kind":"fast", "destinations":[]}),
        ] {
            assert!(Task::parse(invalid).is_err());
        }

        assert!(
            Task::parse(json!({
                "kind":"background", "message":"I'll prepare it.",
                "instruction":"x".repeat(TEXT_LIMIT + 1)
            }))
            .is_err()
        );
    }

    #[test]
    fn background_availability_is_explicit_and_bounded() {
        for available in [true, false] {
            let spec = tool(available);

            validate_tool_spec(&spec).unwrap();

            assert_eq!(
                spec.schema["properties"]["kind"]["enum"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("background")),
                available
            );
        }

        assert_eq!(Kind::Fast.limit(), Duration::from_secs(60));
        assert_eq!(Kind::Thoughtful.limit(), Duration::from_secs(300));
        assert_eq!(Kind::Background.limit(), Duration::from_secs(600));
        assert_eq!(reply("Saved.".into()).text, "Saved.");
        assert!(clock().render().starts_with("Current host time: "));
        assert!(mutating("task", &json!({"kind":"background"})));
        assert!(!mutating("task", &json!({"kind":"thoughtful"})));
        assert!(mutating("job", &json!({"action":"defer"})));
        assert!(mutating("job", &json!({"action":"extend"})));
        assert!(mutating("job", &json!({"action":"pause"})));
        assert!(!mutating("job", &json!({"action":"list"})));
    }

    #[tokio::test(start_paused = true)]
    async fn fast_turns_and_tool_activity_never_create_a_waiting_message() {
        let mut output = StreamOutput::new();

        output.apply(StreamNotice::Text("Hello!".into()));
        output.apply(StreamNotice::Tool("read".into()));
        tokio::time::advance(Duration::from_secs(60)).await;

        assert!(!output.ready());
        assert!(!output.dirty);
        assert!(output.message.is_none());

        output.apply(StreamNotice::Progress("I'll compare the sources.".into()));

        assert!(output.ready());
        assert_eq!(output.message.as_deref(), Some("I'll compare the sources."));
    }
}
