use std::time::{Duration, Instant};

use ratatui::{Terminal, backend::TestBackend};

use super::{App, draw, layout_transcript};

/// A conversation of MESSAGES questions, each answered with rich text.
fn conversation(messages: usize) -> String {
    let mut transcript = String::new();

    for index in 0..messages {
        transcript.push_str(&format!("You\nQuestion {index} about the build?\n\n"));

        transcript.push_str(&format!(
            "Krabb\n## Answer {index}\n\nThe **build** uses `cargo` and [docs](https://example.com/{index}).\n\n\
             - first step with a longer explanation that wraps\n  - nested detail\n- [x] done\n\n\
             | Name | Value |\n|---|---|\n| jobs | {index} |\n\n\
             ```rust\nfn main() {{ let jobs = {index}; println!(\"{{jobs}}\"); }} // run\n```\n\n"
        ));
    }

    transcript
}

fn frame(terminal: &mut Terminal<TestBackend>, app: &mut App) -> Duration {
    let started = Instant::now();

    terminal.draw(|frame| draw(frame, app)).unwrap();
    started.elapsed()
}

#[test]
fn streaming_rewraps_only_the_reply_it_extends() {
    let transcript = conversation(40);
    let full = layout_transcript(&transcript, "Krabb", 80, None, false);

    assert_eq!(full.wrapped, 80);

    let mut streamed = transcript.clone();

    streamed.push_str("more words");

    let next = layout_transcript(&streamed, "Krabb", 80, Some(full), false);
    let fresh = layout_transcript(&streamed, "Krabb", 80, None, false);

    assert_eq!(next.wrapped, 1);
    assert_eq!(next.rows, fresh.rows);
    assert_eq!(next.marks, fresh.marks);
    assert_eq!(next.links, fresh.links);
    assert_eq!(next.blocks, fresh.blocks);

    let trimmed = &streamed[streamed.find("Question 1 ").unwrap()..];
    let after = layout_transcript(trimmed, "Krabb", 80, Some(next), false);

    assert_eq!(after.wrapped, 2);
    assert_eq!(after.rows, layout_transcript(trimmed, "Krabb", 80, None, false).rows);

    let resized = layout_transcript(trimmed, "Krabb", 60, Some(after), false);

    assert_eq!(resized.wrapped, resized.blocks.len());
}

#[test]
fn long_transcripts_stream_and_type_within_a_frame_budget() {
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();

    let mut app = App {
        name: "Krabb".into(),
        transcript: conversation(600),
        theme_enabled: true,
        ..App::default()
    };

    let first = frame(&mut terminal, &mut app);
    let mut streaming = Duration::ZERO;
    let frames = 40;

    for _ in 0..frames {
        app.append_visible("more streamed words ");
        streaming += frame(&mut terminal, &mut app);

        assert_eq!(app.transcript_layout.as_ref().unwrap().wrapped, 1);
    }

    let mut typing = Duration::ZERO;

    for character in "typing".chars() {
        app.input.push(character);
        app.cursor += 1;
        typing += frame(&mut terminal, &mut app);
    }

    let streaming = streaming / frames;
    let typing = typing / 6;

    eprintln!(
        "{} KiB transcript: first frame {first:?}, streaming frame {streaming:?}, \
         typing frame {typing:?}",
        app.transcript.len() / 1024
    );

    assert!(streaming * 4 < first, "streaming {streaming:?} against first {first:?}");
    assert!(typing * 4 < first, "typing {typing:?} against first {first:?}");
}
