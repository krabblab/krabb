use super::*;

const SECOND: Duration = Duration::from_secs(1);
const MINUTE: Duration = Duration::from_secs(60);

fn route(kind: routing::Kind, updates: Updates) -> routing::State {
    let route = routing::State::default();

    route.kind.send_replace(kind);
    route.pace.send_replace(Pace::new(updates));
    route
}

fn collect(receiver: &mut tokio::sync::mpsc::Receiver<StreamNotice>) -> Vec<String> {
    let mut lines = Vec::new();

    while let Ok(notice) = receiver.try_recv() {
        match notice {
            StreamNotice::Status(text) | StreamNotice::Progress(text) => lines.push(text),

            _ => {}
        }
    }

    lines
}

async fn run(route: &routing::State, length: Duration) -> Vec<String> {
    let (sender, mut receiver) = tokio::sync::mpsc::channel(32);

    pace(tokio::time::sleep(length), route, &sender).await;
    collect(&mut receiver)
}

#[test]
fn thoughtful_turns_update_after_twenty_seconds_then_every_forty_five() {
    let thoughtful = routing::Kind::Thoughtful;

    assert_eq!(gap(thoughtful, Updates::Quiet, 0, Duration::ZERO, None), Some(20 * SECOND));
    assert_eq!(gap(thoughtful, Updates::Quiet, 1, 20 * SECOND, None), Some(45 * SECOND));
    assert_eq!(gap(thoughtful, Updates::Frequent, 0, Duration::ZERO, None), Some(10 * SECOND));

    assert_eq!(
        gap(thoughtful, Updates::Frequent, 2, MINUTE, None),
        Some(Duration::from_millis(22_500))
    );

    assert_eq!(gap(thoughtful, Updates::Off, 0, Duration::ZERO, None), None);
    assert_eq!(gap(routing::Kind::Fast, Updates::Frequent, 0, Duration::ZERO, None), None);
    assert_eq!(gap(thoughtful, Updates::Quiet, CAP, Duration::ZERO, None), None);
    assert_eq!(gap(thoughtful, Updates::Quiet, 1, 20 * MINUTE, None), Some(2 * MINUTE));
}

#[test]
fn tasks_update_by_a_tenth_of_their_expected_length_and_never_within_two_minutes() {
    let task = routing::Kind::Background;
    let quiet = |sent, elapsed, expected| gap(task, Updates::Quiet, sent, elapsed, expected);

    assert_eq!(quiet(0, Duration::ZERO, None), Some(3 * MINUTE));
    assert_eq!(quiet(1, 3 * MINUTE, None), Some(3 * MINUTE));
    assert_eq!(quiet(0, Duration::ZERO, Some(10 * MINUTE)), Some(3 * MINUTE));
    assert_eq!(quiet(1, 3 * MINUTE, Some(10 * MINUTE)), Some(2 * MINUTE));
    assert_eq!(quiet(0, Duration::ZERO, Some(60 * MINUTE)), Some(6 * MINUTE));
    assert_eq!(quiet(1, 6 * MINUTE, Some(4 * 60 * MINUTE)), Some(24 * MINUTE));

    assert_eq!(quiet(2, 120 * MINUTE, Some(60 * MINUTE)), Some(12 * MINUTE));
    assert!(quiet(2, 240 * MINUTE, Some(60 * MINUTE)) > quiet(2, 120 * MINUTE, Some(60 * MINUTE)));

    assert_eq!(gap(task, Updates::Frequent, 1, MINUTE, Some(10 * MINUTE)), Some(2 * MINUTE));
    assert_eq!(gap(task, Updates::Frequent, 1, MINUTE, Some(60 * MINUTE)), Some(3 * MINUTE));
    assert_eq!(gap(task, Updates::Off, 1, MINUTE, Some(60 * MINUTE)), None);
    assert_eq!(quiet(CAP, MINUTE, None), None);
}

#[test]
fn host_lines_count_tool_activity_in_the_known_language() {
    let mut pace = Pace::new(Updates::Quiet);

    assert_eq!(pace.line(), "Still working.");

    for name in ["web_fetch", "web_fetch", "web_search", "browser_snapshot", "shell_run"] {
        pace.activity.record(name);
    }

    pace.activity.record("git_run");
    pace.activity.record(routing::NAME);
    pace.activity.record(NAME);

    assert_eq!(pace.line(), "Still working: 4 pages read, 2 commands run.");

    pace.activity.record("file_read");
    pace.activity.record("file_write");
    pace.activity.record("memory_search");

    assert_eq!(
        pace.line(),
        "Still working: 4 pages read, 1 file read, 1 file changed, 2 commands run, 1 other step."
    );

    assert!(!pace.speak("klingon"));
    assert!(!pace.speak("e n"));
    assert!(!pace.speak(&"a".repeat(LANGUAGE_LIMIT + 1)));
    assert!(pace.speak("it-IT"));
    assert!(!pace.speak("ita"));

    assert_eq!(
        pace.line(),
        "Sto ancora lavorando: 4 pagine lette, 1 file letto, 1 file modificato, 2 comandi \
            eseguiti, 1 altro passaggio."
    );

    for (tag, lead) in [
        ("es", "Sigo trabajando: "),
        ("fr", "Toujours en cours : "),
        ("de_DE", "Ich arbeite noch: "),
        ("pt-BR", "Ainda trabalhando: "),
        ("EN", "Still working: "),
    ] {
        assert!(pace.speak(tag));
        assert!(pace.line().starts_with(lead), "{tag}");
    }
}

#[test]
fn milestones_are_one_short_line() {
    let parsed = Milestone::parse(serde_json::json!({"message": "  Found three sources.  "}));

    assert_eq!(parsed.unwrap().message, "Found three sources.");

    let parsed = Milestone::parse(serde_json::json!({"message": "Trovate.", "language": "it"}));

    assert_eq!(parsed.unwrap().language.as_deref(), Some("it"));

    for value in [
        serde_json::json!({"message": " "}),
        serde_json::json!({"message": "a".repeat(LIMIT + 1)}),
        serde_json::json!({"message": "Two\nlines"}),
        serde_json::json!({"message": "Done.", "answer": true}),
        serde_json::json!({}),
    ] {
        assert!(Milestone::parse(value).is_err());
    }

    let spec = tool();

    assert_eq!(spec.name, NAME);
    assert_eq!(spec.schema["required"], serde_json::json!(["message"]));
    assert!(HOST_TOOLS.contains(&NAME));
    assert!(!mutating(NAME, &serde_json::json!({"message": "Found it."})));
}

#[test]
fn the_tool_is_offered_only_on_thoughtful_turns_and_task_steps_with_updates_on() {
    let quiet = Pace::new(Updates::Quiet);

    assert!(offered(&quiet, routing::Kind::Thoughtful));
    assert!(offered(&quiet, routing::Kind::Background));
    assert!(!offered(&quiet, routing::Kind::Fast));
    assert!(!offered(&Pace::new(Updates::Off), routing::Kind::Thoughtful));
    assert!(offered(&Pace::default(), routing::Kind::Background));
}

#[test]
fn channels_and_clients_override_the_top_level_setting() {
    let config: Config = toml::from_str(
        "updates = \"frequent\"\n[channels.telegram]\nupdates = \"off\"\n[clients.tui]\n\
            updates = \"quiet\"\n",
    )
    .unwrap();

    assert_eq!(setting(&config, "telegram"), Updates::Off);
    assert_eq!(setting(&config, "tui"), Updates::Quiet);
    assert_eq!(setting(&config, "discord"), Updates::Frequent);
    assert_eq!(setting(&Config::default(), "telegram"), Updates::Quiet);
    assert!(toml::from_str::<Config>("updates = \"loud\"\n").is_err());

    let saved = toml::to_string(&Config::default()).unwrap();

    assert!(saved.contains("updates = \"quiet\""), "{saved}");
    assert!(toml::from_str::<Config>(&saved).is_ok());
}

#[tokio::test(start_paused = true)]
async fn the_host_speaks_after_silence_with_activity_counts() {
    let route = route(routing::Kind::Thoughtful, Updates::Quiet);

    for name in ["web_fetch", "web_fetch", "web_fetch", "web_fetch", "shell_run", "shell_run"] {
        Pace::record(&route.pace, name);
    }

    assert!(run(&route, 19 * SECOND).await.is_empty());

    let route = self::route(routing::Kind::Thoughtful, Updates::Quiet);

    for name in ["web_fetch", "web_fetch", "web_fetch", "web_fetch", "shell_run", "shell_run"] {
        Pace::record(&route.pace, name);
    }

    assert_eq!(run(&route, 21 * SECOND).await, ["Still working: 4 pages read, 2 commands run."]);
    assert_eq!(run(&route, 46 * SECOND).await.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn quiet_turns_stop_at_the_cap_and_frequent_turns_reach_it_sooner() {
    let quiet = route(routing::Kind::Thoughtful, Updates::Quiet);

    assert_eq!(run(&quiet, 3 * MINUTE).await.len(), 4);
    assert_eq!(run(&quiet, 60 * MINUTE).await.len(), 1);
    assert!(run(&quiet, 60 * MINUTE).await.is_empty());

    let frequent = route(routing::Kind::Thoughtful, Updates::Frequent);

    assert_eq!(run(&frequent, 2 * MINUTE).await.len(), CAP as usize);
}

#[tokio::test(start_paused = true)]
async fn updates_stay_silent_when_off_on_fast_turns_and_while_streaming() {
    assert!(run(&route(routing::Kind::Thoughtful, Updates::Off), 60 * MINUTE).await.is_empty());
    assert!(run(&route(routing::Kind::Background, Updates::Off), 60 * MINUTE).await.is_empty());
    assert!(run(&route(routing::Kind::Fast, Updates::Frequent), 60 * MINUTE).await.is_empty());

    let streaming = route(routing::Kind::Thoughtful, Updates::Quiet);

    Pace::stream(&streaming.pace, "The answer is");

    assert!(run(&streaming, 10 * MINUTE).await.is_empty());

    Pace::stream(&streaming.pace, "");

    assert_eq!(run(&streaming, 30 * SECOND).await.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn the_routing_message_counts_as_the_first_update() {
    let route = route(routing::Kind::Thoughtful, Updates::Quiet);

    assert!(open(&route, Some("it")));
    assert!(!open(&route, None));
    assert!(run(&route, 44 * SECOND).await.is_empty());
    assert_eq!(run(&route, 2 * SECOND).await, ["Sto ancora lavorando."]);
    assert_eq!(run(&route, 10 * MINUTE).await.len(), 3);

    let off = self::route(routing::Kind::Thoughtful, Updates::Off);

    assert!(!open(&off, Some("en")));
}

#[tokio::test(start_paused = true)]
async fn milestones_are_paced_and_restart_the_silence() {
    let route = route(routing::Kind::Thoughtful, Updates::Quiet);
    let found = Milestone { message: "Found three sources.".into(), language: Some("fr".into()) };

    assert!(!milestone(&route, &found));
    assert_eq!(route.pace.borrow().language, Some(Language::French));

    tokio::time::advance(10 * SECOND).await;

    assert!(milestone(&route, &found));
    assert!(!milestone(&route, &found));
    assert!(run(&route, 44 * SECOND).await.is_empty());
    assert_eq!(run(&route, 2 * SECOND).await, ["Toujours en cours."]);

    let off = self::route(routing::Kind::Thoughtful, Updates::Off);

    tokio::time::advance(MINUTE).await;

    assert!(!milestone(&off, &found));
}

#[tokio::test(start_paused = true)]
async fn approvals_hold_updates_and_restart_the_silence() {
    let route = route(routing::Kind::Thoughtful, Updates::Quiet);
    let (sender, mut receiver) = tokio::sync::mpsc::channel(32);

    let work = async {
        let waiting = Waiting::new(&route.pace);

        tokio::time::sleep(5 * MINUTE).await;
        drop(waiting);
        tokio::time::sleep(19 * SECOND).await;
        collect(&mut receiver).len()
    };

    assert_eq!(pace(work, &route, &sender).await, 0);
    assert_eq!(run(&route, 2 * SECOND).await.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn nothing_follows_the_final_answer() {
    let route = route(routing::Kind::Thoughtful, Updates::Quiet);
    let (sender, mut receiver) = tokio::sync::mpsc::channel(32);

    let answer = pace(async { "Done." }, &route, &sender).await;

    tokio::time::advance(10 * MINUTE).await;

    assert_eq!(answer, "Done.");
    assert!(collect(&mut receiver).is_empty());
}

#[tokio::test(start_paused = true)]
async fn task_steps_of_different_lengths_update_rarely() {
    let short = route(routing::Kind::Background, Updates::Quiet);

    assert!(run(&short, 179 * SECOND).await.is_empty());
    assert_eq!(run(&short, 2 * SECOND).await.len(), 1);

    let long = route(routing::Kind::Background, Updates::Quiet);

    long.pace.send_modify(|pace| {
        pace.expect(2 * 60 * MINUTE);
    });

    assert!(run(&long, 11 * MINUTE).await.is_empty());
    assert_eq!(run(&long, 2 * MINUTE).await.len(), 1);
    assert!(run(&long, 10 * MINUTE).await.is_empty());
    assert_eq!(run(&long, 3 * MINUTE).await.len(), 1);
}
