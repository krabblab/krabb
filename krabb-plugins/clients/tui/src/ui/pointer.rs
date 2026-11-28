use std::{
    ops::Range,
    time::{Duration, Instant},
};

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::{buffer::Buffer, layout::Rect, style::Modifier, text::Line};
use unicode_width::UnicodeWidthChar;

use super::{
    App, PROMPT_WIDTH, copy,
    selection::{self, Mark, Point, Selection},
};

/// The longest pause between clicks that still counts them as a double or triple click.
const CLICK_GAP: Duration = Duration::from_millis(500);

/// Labels of transcript lines whose value is a command a click runs, such as approvals.
pub(super) const ACTION_LABELS: [&str; 2] = ["Approve", "Deny"];

/// A clickable action drawn in the last frame and the command a click on it runs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Hit {
    pub(super) area: Rect,
    pub(super) command: String,
}

#[derive(Clone, Copy)]
enum Target {
    Transcript(Point),
    Input(usize),
    Elsewhere,
}

/// The left button held down: where, on what, and whether it moved since.
struct Press {
    position: (u16, u16),
    target: Target,
    moved: bool,
    count: u8,
    command: Option<String>,
}

/// What the mouse is doing: the button it holds, recent clicks, the selection, and copies.
#[derive(Default)]
pub(super) struct Pointer {
    press: Option<Press>,
    click: Option<(Instant, (u16, u16), u8)>,
    pub(super) selection: Option<Selection>,
    pub(super) hits: Vec<Hit>,
    pub(super) hovered: Option<usize>,
    pub(super) copy: Option<String>,
    pub(super) notice: Option<copy::Notice>,
    pub(super) clipboard: Option<arboard::Clipboard>,
}

impl Pointer {
    /// Counts a click at POSITION as the first, second, or third of a quick run.
    fn count(&mut self, position: (u16, u16), now: Instant) -> u8 {
        let count = match self.click {
            Some((at, last, count))
                if last == position && now.saturating_duration_since(at) <= CLICK_GAP =>
            {
                count % 3 + 1
            }

            _ => 1,
        };

        self.click = Some((now, position, count));
        count
    }

    /// The action under POSITION in the last frame.
    pub(super) fn hit(&self, position: Option<(u16, u16)>) -> Option<usize> {
        let (column, row) = position?;

        self.hits.iter().position(|hit| hit.area.contains((column, row).into()))
    }

    /// Whether the button is held, so the frame follows every drag.
    pub(super) fn holding(&self) -> bool {
        self.press.is_some()
    }

    /// Whether hovering should leave messages as they are, because text is being selected.
    pub(super) fn selecting(&self) -> bool {
        self.press.is_some() || self.selection.is_some()
    }

    /// Whether the held button has moved, which turns a click into a selection.
    pub(super) fn dragging(&self) -> bool {
        self.press.as_ref().is_some_and(|press| press.moved)
    }
}

impl App {
    /// Handles a mouse EVENT and returns the command of an action it clicked.
    pub(super) fn point(&mut self, event: MouseEvent, now: Instant) -> Option<String> {
        let position = (event.column, event.row);

        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.press(position, now);
                None
            }

            MouseEventKind::Drag(MouseButton::Left) => {
                self.drag(position);
                None
            }

            MouseEventKind::Up(MouseButton::Left) => self.release(position),
            _ => None,
        }
    }

    fn press(&mut self, position: (u16, u16), now: Instant) {
        let count = self.pointer.count(position, now);
        let command =
            self.pointer.hit(Some(position)).map(|at| self.pointer.hits[at].command.clone());
        let inside = |area: Option<Rect>| area.is_some_and(|area| area.contains(position.into()));

        let target = if inside(self.input_area) {
            Target::Input(self.input_position(position))
        } else if inside(self.transcript_area) {
            self.transcript_point(position, false).map_or(Target::Elsewhere, Target::Transcript)
        } else {
            Target::Elsewhere
        };

        self.pointer.selection = None;

        match (target, count) {
            (Target::Input(index), 1) => self.cursor = index,

            (Target::Input(index), 2) => {
                let word = selection::input_word(&self.input, index);

                self.cursor = word.end;
                self.pointer.selection =
                    Some(Selection::Input { anchor: word.start, head: word.end });
            }

            (Target::Input(_), _) => {
                self.cursor = self.input.len();
                self.pointer.selection =
                    Some(Selection::Input { anchor: 0, head: self.input.len() });
            }

            (Target::Transcript(point), 2) => self.select_word(point),
            (Target::Transcript(point), 3) => self.select_message(point),

            _ => {}
        }

        self.pointer.press = Some(Press { position, target, moved: false, count, command });
    }

    fn drag(&mut self, position: (u16, u16)) {
        let Some(press) = self.pointer.press.as_mut() else {
            return;
        };

        press.moved |= position != press.position;

        if !press.moved {
            return;
        }

        match press.target {
            Target::Transcript(anchor) => {
                if let Some(head) = self.transcript_point(position, true) {
                    self.pointer.selection = Some(Selection::Transcript { anchor, head });
                }
            }

            Target::Input(anchor) => {
                let head = self.input_position(position);

                self.cursor = head;
                self.pointer.selection = Some(Selection::Input { anchor, head });
            }

            Target::Elsewhere => {}
        }
    }

    fn release(&mut self, position: (u16, u16)) -> Option<String> {
        let press = self.pointer.press.take()?;

        if press.moved || press.count > 1 {
            self.copy_selection();
            return None;
        }

        let command = press.command?;
        let current = self.pointer.hit(Some(position)).map(|at| &self.pointer.hits[at].command);

        (current == Some(&command)).then_some(command)
    }

    /// Clears the selection, and says whether there was one.
    pub(super) fn clear_selection(&mut self) -> bool {
        self.pointer.press = None;
        self.pointer.selection.take().is_some()
    }

    /// Clears a transcript selection whose rows no longer hold the text it covered.
    pub(super) fn clear_transcript_selection(&mut self) {
        if matches!(self.pointer.selection, Some(Selection::Transcript { .. })) {
            self.pointer.selection = None;
        }

        if self
            .pointer
            .press
            .as_ref()
            .is_some_and(|press| matches!(press.target, Target::Transcript(_)))
        {
            self.pointer.press = None;
        }
    }

    fn copy_selection(&mut self) {
        let text = match self.pointer.selection {
            Some(selection @ Selection::Transcript { .. }) => {
                let Some((first, last)) = selection.cells() else {
                    return;
                };

                let Some(layout) = self.current_layout() else {
                    return;
                };

                selection::extract(&layout.rows, &layout.marks, first, last)
            }

            Some(selection @ Selection::Input { .. }) => {
                let Some(range) = selection.characters() else {
                    return;
                };

                let end = range.end.min(self.input.len());

                self.input[range.start.min(end)..end].iter().collect()
            }

            None => return,
        };

        if !text.is_empty() {
            self.pointer.copy = Some(text);
        }
    }

    fn current_layout(&mut self) -> Option<&super::TranscriptLayout> {
        let width = self.transcript_area?.width;

        self.refresh_transcript_layout(width);
        self.transcript_layout.as_ref()
    }

    fn select_word(&mut self, point: Point) {
        let Some(layout) = self.current_layout() else {
            return;
        };

        let (Some(line), Some(mark)) = (layout.rows.get(point.row), layout.marks.get(point.row))
        else {
            return;
        };

        if let Some((first, last)) = selection::word(line, *mark, point.column) {
            self.pointer.selection = Some(Selection::Transcript {
                anchor: Point { row: point.row, column: first },
                head: Point { row: point.row, column: last },
            });
        }
    }

    fn select_message(&mut self, point: Point) {
        let Some(layout) = self.current_layout() else {
            return;
        };

        let Some(rows) = layout
            .messages
            .iter()
            .map(|message| &message.rows)
            .find(|rows| rows.contains(&point.row))
        else {
            return;
        };

        self.pointer.selection = Some(Selection::Transcript {
            anchor: Point { row: rows.start, column: 0 },
            head: Point { row: rows.end.saturating_sub(1), column: u16::MAX },
        });
    }

    /// The transcript cell under POSITION. When SCROLL is set, a pointer above or below the
    /// transcript scrolls it one row, so a selection can grow past the visible rows.
    fn transcript_point(&mut self, (column, row): (u16, u16), scroll: bool) -> Option<Point> {
        let area = self.transcript_area.filter(|area| !area.is_empty())?;

        if scroll && row < area.y {
            self.transcript_scroll.scroll_up(1);
        } else if scroll && row >= area.bottom() {
            self.transcript_scroll.scroll_down(1);
        }

        let y = row.clamp(area.y, area.bottom() - 1) - area.y;

        Some(Point {
            row: self.transcript_scroll.offset.saturating_add(usize::from(y)),
            column: column.saturating_sub(area.x),
        })
    }

    /// The input position under POSITION, clamped to the rows the input box shows.
    fn input_position(&self, (column, row): (u16, u16)) -> usize {
        let Some(area) = self.input_area else {
            return self.cursor;
        };

        let top = area.y.saturating_add(1);
        let bottom = area.bottom().saturating_sub(2).max(top);
        let row = row.clamp(top, bottom) - top;

        super::input_index(
            &self.input,
            self.cursor,
            area.width,
            column.saturating_sub(area.x.saturating_add(PROMPT_WIDTH)),
            self.input_scroll.offset.saturating_add(usize::from(row)),
        )
    }
}

/// Clickable commands among ROWS drawn in AREA: each row marked as an action whose command
/// starts at its marked column.
pub(super) fn transcript_hits(rows: &[&Line<'_>], marks: &[Mark], area: Rect) -> Vec<Hit> {
    let mut hits = Vec::new();

    for (y, (line, mark)) in rows.iter().zip(marks).enumerate().take(usize::from(area.height)) {
        let Some(start) = mark.action else {
            continue;
        };

        let start = mark.skip.saturating_add(start);
        let end = mark.skip.saturating_add(mark.width).min(area.width);

        let command = selection::cells(line)
            .filter(|(column, _)| (start..end).contains(column))
            .map(|(_, grapheme)| grapheme)
            .collect::<String>();

        if end > start && command.starts_with('/') {
            hits.push(Hit {
                area: Rect::new(area.x + start, area.y + y as u16, end - start, 1),
                command: command.trim().to_owned(),
            });
        }
    }

    hits
}

/// Hits for the spans of LINE, drawn at X and Y, whose text names one of LABELS.
pub(super) fn label_hits(line: &Line<'_>, x: u16, y: u16, labels: &[(&str, String)]) -> Vec<Hit> {
    let mut hits = Vec::new();
    let mut column = x;

    for span in &line.spans {
        let width = span.width().min(usize::from(u16::MAX)) as u16;
        let name = span.content.trim().trim_matches(['[', ']']);

        if let Some((_, command)) = labels.iter().find(|(label, _)| *label == name) {
            hits.push(Hit { area: Rect::new(column, y, width, 1), command: command.clone() });
        }

        column = column.saturating_add(width);
    }

    hits
}

/// Underlines the cells of HIT, so the pointer shows what a click would run.
pub(super) fn emphasize(buffer: &mut Buffer, hit: &Hit) {
    let area = hit.area.intersection(buffer.area);

    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            buffer[(x, y)].modifier.insert(Modifier::UNDERLINED | Modifier::BOLD);
        }
    }
}

/// Reverses the selected input characters in AREA, whose rows start at STARTS and scroll by
/// OFFSET rows.
pub(super) fn highlight_input(
    buffer: &mut Buffer,
    area: Rect,
    input: &[char],
    starts: &[usize],
    offset: usize,
    range: Range<usize>,
) {
    let top = area.y.saturating_add(1);
    let shown = usize::from(area.height.saturating_sub(2));

    for (row, start) in starts.iter().enumerate().skip(offset).take(shown) {
        let end = starts.get(row + 1).copied().unwrap_or(input.len()).min(input.len());
        let y = top + (row - offset) as u16;
        let mut column = area.x.saturating_add(PROMPT_WIDTH);

        for (index, &character) in input.iter().enumerate().take(end).skip(*start) {
            if character == '\n' {
                break;
            }

            let width = UnicodeWidthChar::width(character).unwrap_or(0) as u16;

            if range.contains(&index) {
                for x in column..column.saturating_add(width).min(area.right()) {
                    buffer[(x, y)].modifier.insert(Modifier::REVERSED);
                }
            }

            column = column.saturating_add(width);
        }
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use ratatui::{Terminal, backend::TestBackend, style::Style, text::Span};
    use tokio::io::{AsyncReadExt, duplex};

    use super::{
        super::{draw, handle_event, input_index},
        *,
    };

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> Event {
        Event::Mouse(MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE })
    }

    fn approval() -> App {
        App {
            generation_active: true,
            input: "draft".chars().collect(),
            cursor: 2,
            transcript: "System | now\nApproval needed.\nTool: shell\nCommand: touch foo\n\
                Approve: /approve approval-1\nDeny: /deny approval-1\n"
                .into(),
            ..App::default()
        }
    }

    #[test]
    fn places_the_cursor_across_wrapped_rows_and_wide_characters() {
        let input = "ab界cd\nxy".chars().collect::<Vec<_>>();

        assert_eq!(input_index(&input, 0, 8, 0, 0), 0);
        assert_eq!(input_index(&input, 0, 8, 2, 0), 2);
        assert_eq!(input_index(&input, 0, 8, 3, 0), 2);
        assert_eq!(input_index(&input, 0, 8, 4, 0), 3);
        assert_eq!(input_index(&input, 0, 8, 9, 0), 4);
        assert_eq!(input_index(&input, 0, 8, 0, 1), 4);
        assert_eq!(input_index(&input, 0, 8, 9, 1), 5);
        assert_eq!(input_index(&input, 0, 8, 1, 2), 7);
        assert_eq!(input_index(&input, 0, 8, 9, 9), 8);
        assert_eq!(input_index(&[], 0, 8, 4, 0), 0);
    }

    #[tokio::test]
    async fn clicking_the_input_moves_the_cursor_and_dragging_selects_text() {
        let (mut command_tx, _command_rx) = duplex(1024);
        let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
        let mut app = App { input: "hello wide 界 world".chars().collect(), ..App::default() };

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();

        let area = app.input_area.unwrap();
        let row = area.y + 1;
        let left = MouseEventKind::Down(MouseButton::Left);
        let up = MouseEventKind::Up(MouseButton::Left);

        handle_event(mouse(left, area.x + 9, row), &mut app, &mut command_tx, false).await.unwrap();

        handle_event(mouse(up, area.x + 9, row), &mut app, &mut command_tx, false).await.unwrap();

        assert_eq!(app.cursor, 6);
        assert_eq!(app.pointer.copy, None);

        handle_event(mouse(left, area.x + 3, row), &mut app, &mut command_tx, false).await.unwrap();

        let drag = MouseEventKind::Drag(MouseButton::Left);

        handle_event(mouse(drag, area.x + 8, row), &mut app, &mut command_tx, false).await.unwrap();

        handle_event(mouse(up, area.x + 8, row), &mut app, &mut command_tx, false).await.unwrap();

        assert_eq!(app.pointer.copy.take().as_deref(), Some("hello"));

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();

        let buffer = terminal.backend().buffer();

        assert!(buffer[(area.x + 3, row)].modifier.contains(Modifier::REVERSED));
        assert!(!buffer[(area.x + 8, row)].modifier.contains(Modifier::REVERSED));

        let key = |code| Event::Key(KeyEvent::new(code, KeyModifiers::NONE));

        handle_event(key(KeyCode::Char('J')), &mut app, &mut command_tx, false).await.unwrap();

        assert_eq!(app.input.iter().collect::<String>(), "J wide 界 world");
        assert_eq!(app.pointer.selection, None);
    }

    #[tokio::test]
    async fn dragging_and_multiple_clicks_select_and_copy_transcript_text() {
        let (mut command_tx, _command_rx) = duplex(1024);
        let mut terminal = Terminal::new(TestBackend::new(40, 14)).unwrap();

        let mut app = App {
            name: "Krabb".into(),
            transcript: "You\nfirst question\n\nKrabb\nalpha beta gamma delta epsilon zeta \
                eta theta\nsecond line\n"
                .into(),
            ..App::default()
        };

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();

        let layout = app.transcript_layout.as_ref().unwrap();
        let row = layout.messages[1].rows.start as u16 + 1;
        let left = MouseEventKind::Down(MouseButton::Left);
        let drag = MouseEventKind::Drag(MouseButton::Left);
        let up = MouseEventKind::Up(MouseButton::Left);

        for event in [mouse(left, 0, 0), mouse(drag, 39, row + 2), mouse(up, 39, row + 2)] {
            handle_event(event, &mut app, &mut command_tx, false).await.unwrap();
        }

        assert_eq!(
            app.pointer.copy.take().as_deref(),
            Some("first question\n\nalpha beta gamma delta epsilon zeta eta theta\nsecond line")
        );

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();

        assert!(terminal.backend().buffer()[(2, 1)].modifier.contains(Modifier::REVERSED));
        assert!(!terminal.backend().buffer()[(0, 1)].modifier.contains(Modifier::REVERSED));

        let escape = Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert!(!handle_event(escape.clone(), &mut app, &mut command_tx, false).await.unwrap());
        assert_eq!(app.pointer.selection, None);

        for event in
            [mouse(left, 9, row), mouse(up, 9, row), mouse(left, 9, row), mouse(up, 9, row)]
        {
            handle_event(event, &mut app, &mut command_tx, false).await.unwrap();
        }

        assert_eq!(app.pointer.copy.take().as_deref(), Some("beta"));

        for event in [mouse(left, 9, row), mouse(up, 9, row)] {
            handle_event(event, &mut app, &mut command_tx, false).await.unwrap();
        }

        assert_eq!(
            app.pointer.copy.take().as_deref(),
            Some("alpha beta gamma delta epsilon zeta eta theta\nsecond line")
        );
    }

    #[tokio::test]
    async fn clicking_an_action_runs_the_typed_command_and_keeps_the_draft() {
        let (mut command_tx, mut command_rx) = duplex(1024);
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let mut app = approval();

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();

        let commands = app.pointer.hits.iter().map(|hit| hit.command.as_str()).collect::<Vec<_>>();

        assert_eq!(
            commands,
            ["/approve approval-1", "/deny approval-1", "/approve approval-1", "/deny approval-1"]
        );

        let deny = app.pointer.hits[1].area;
        let (x, y) = (deny.x + 1, deny.y);

        app.mouse_position = Some((x, y));

        assert!(app.hover_changed());
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();

        assert!(terminal.backend().buffer()[(x, y)].modifier.contains(Modifier::UNDERLINED));
        assert!(!app.hover_changed());

        let left = MouseEventKind::Down(MouseButton::Left);
        let drag = MouseEventKind::Drag(MouseButton::Left);
        let up = MouseEventKind::Up(MouseButton::Left);

        for event in [mouse(left, x, y), mouse(drag, x + 2, y), mouse(up, x + 2, y)] {
            handle_event(event, &mut app, &mut command_tx, false).await.unwrap();
        }

        assert!(app.dismissed_approvals.is_empty());
        assert!(app.pointer.copy.take().is_some());

        for event in [mouse(left, x, y), mouse(up, x, y + 5)] {
            handle_event(event, &mut app, &mut command_tx, false).await.unwrap();
        }

        assert!(app.dismissed_approvals.is_empty());

        app.pointer.click = None;

        for event in [mouse(left, x, y), mouse(up, x, y)] {
            handle_event(event, &mut app, &mut command_tx, false).await.unwrap();
        }

        let mut bytes = [0; 128];
        let count = command_rx.read(&mut bytes).await.unwrap();

        assert_eq!(&bytes[..count], b"\"/deny approval-1\"\n");
        assert_eq!(app.input.iter().collect::<String>(), "draft");
        assert_eq!(app.cursor, 2);
        assert!(app.dismissed_approvals.contains("approval-1"));

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();

        assert!(app.pointer.hits.is_empty());
    }

    #[test]
    fn shows_the_copy_notice_at_the_top_right_of_the_input() {
        let mut terminal = Terminal::new(TestBackend::new(50, 10)).unwrap();
        let mut app = App::default();
        let mut written = Vec::new();

        app.pointer.notice = Some(copy::copy("hello", &mut written, |_| false, Instant::now()));
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();

        let area = app.input_area.unwrap();
        let buffer = terminal.backend().buffer();

        let top = (area.x..area.right())
            .map(|x| buffer[(x, area.y)].symbol().to_owned())
            .collect::<String>();

        let notice = " ✓ Copied 5 characters ";
        let end = top.find(notice).unwrap() + notice.len();

        assert_eq!(top[end..].chars().count(), 1);
        assert!(app.pointer.notice.is_some());
    }

    #[test]
    fn counts_quick_clicks_in_place_up_to_three() {
        let mut pointer = Pointer::default();
        let now = Instant::now();

        assert_eq!(pointer.count((3, 4), now), 1);
        assert_eq!(pointer.count((3, 4), now + Duration::from_millis(200)), 2);
        assert_eq!(pointer.count((3, 4), now + Duration::from_millis(400)), 3);
        assert_eq!(pointer.count((3, 4), now + Duration::from_millis(600)), 1);
        assert_eq!(pointer.count((4, 4), now + Duration::from_millis(700)), 1);
        assert_eq!(pointer.count((4, 4), now + Duration::from_secs(2)), 1);
    }

    #[test]
    fn finds_commands_on_marked_rows_and_labelled_spans() {
        let mut rows = selection::Rows::default();

        rows.text(Line::raw("Approve: /approve 7"), selection::Joint::Break);
        rows.text(Line::raw("Approve: /approve 8"), selection::Joint::Break);
        rows.text(Line::raw("Approve: /approve 9"), selection::Joint::Break);
        rows.marks[0].action = Some(9);
        rows.marks[1].action = Some(9);
        rows.indent(2);

        for line in &mut rows.lines {
            line.spans.insert(0, Span::raw("▌ "));
        }

        let lines = rows.lines.iter().collect::<Vec<_>>();
        let hits = transcript_hits(&lines, &rows.marks, Rect::new(4, 2, 40, 2));

        assert_eq!(
            hits,
            vec![
                Hit { area: Rect::new(15, 2, 10, 1), command: "/approve 7".into() },
                Hit { area: Rect::new(15, 3, 10, 1), command: "/approve 8".into() },
            ]
        );

        let line = Line::from(vec![
            Span::raw("  "),
            Span::styled("[Approve]", Style::default()),
            Span::raw("   "),
            Span::raw(" Deny "),
        ]);

        let labels = [("Approve", "/approve 7".to_owned()), ("Deny", "/deny 7".to_owned())];
        let hits = label_hits(&line, 2, 5, &labels);

        assert_eq!(hits[0], Hit { area: Rect::new(4, 5, 9, 1), command: "/approve 7".into() });
        assert_eq!(hits[1], Hit { area: Rect::new(16, 5, 6, 1), command: "/deny 7".into() });
    }
}
