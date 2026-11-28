use std::ops::Range;

use ratatui::{buffer::Buffer, layout::Rect, style::Modifier, text::Line};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// How a wrapped row continues the copyable text of the row before it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Joint {
    /// A line break the text itself holds.
    Break,
    /// A wrap at a space the layout hid.
    Space,
    /// A wrap inside a word or a code line.
    Glue,
    /// Decoration only, such as a label or a border, which copying skips.
    Hidden,
}

/// The copyable columns of one wrapped row, and where a clickable command starts in it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Mark {
    pub(super) skip: u16,
    pub(super) width: u16,
    pub(super) joint: Joint,
    pub(super) action: Option<u16>,
}

impl Mark {
    pub(super) fn hidden() -> Self {
        Self { skip: 0, width: 0, joint: Joint::Hidden, action: None }
    }

    fn end(self) -> u16 {
        self.skip.saturating_add(self.width)
    }
}

/// Wrapped rows with the marks that tell selection and copying what each row holds.
#[derive(Default)]
pub(super) struct Rows {
    pub(super) lines: Vec<Line<'static>>,
    pub(super) marks: Vec<Mark>,
}

impl Rows {
    pub(super) fn len(&self) -> usize {
        self.lines.len()
    }

    pub(super) fn push(&mut self, line: Line<'static>, mark: Mark) {
        self.lines.push(line);
        self.marks.push(mark);
    }

    /// Adds LINE as text that is copied whole, joined to the row before by JOINT.
    pub(super) fn text(&mut self, line: Line<'static>, joint: Joint) {
        let width = line.width().min(usize::from(u16::MAX)) as u16;

        self.push(line, Mark { skip: 0, width, joint, action: None });
    }

    pub(super) fn hidden(&mut self, line: Line<'static>) {
        self.push(line, Mark::hidden());
    }

    pub(super) fn append(&mut self, mut other: Rows) {
        self.lines.append(&mut other.lines);
        self.marks.append(&mut other.marks);
    }

    /// Marks every row as decoration, so copying skips it.
    pub(super) fn hide(&mut self) {
        self.marks.fill(Mark::hidden());
    }

    /// Records that COLUMNS of decoration now precede each row.
    pub(super) fn indent(&mut self, columns: u16) {
        for mark in &mut self.marks {
            mark.skip = mark.skip.saturating_add(columns);
        }
    }
}

/// A cell in the wrapped transcript: its row from the top and its column.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct Point {
    pub(super) row: usize,
    pub(super) column: u16,
}

/// Text the pointer selected, between two transcript cells or two input positions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Selection {
    Transcript { anchor: Point, head: Point },
    Input { anchor: usize, head: usize },
}

impl Selection {
    /// The selected transcript cells, first to last, both included.
    pub(super) fn cells(self) -> Option<(Point, Point)> {
        match self {
            Self::Transcript { anchor, head } => Some((anchor.min(head), anchor.max(head))),
            Self::Input { .. } => None,
        }
    }

    /// The selected input characters, or none when the selection is empty.
    pub(super) fn characters(self) -> Option<Range<usize>> {
        match self {
            Self::Input { anchor, head } if anchor != head => {
                Some(anchor.min(head)..anchor.max(head))
            }

            _ => None,
        }
    }
}

/// Each grapheme of LINE with the column where it starts.
pub(super) fn cells<'a>(line: &'a Line<'_>) -> impl Iterator<Item = (u16, &'a str)> {
    let mut column = 0_u16;

    line.spans.iter().flat_map(|span| span.content.graphemes(true)).map(move |grapheme| {
        let start = column;
        let width = UnicodeWidthStr::width(grapheme).min(usize::from(u16::MAX)) as u16;

        column = column.saturating_add(width);
        (start, grapheme)
    })
}

/// The copyable columns of ROW that lie between FIRST and LAST, as a half-open range.
fn span(mark: Mark, row: usize, first: Point, last: Point) -> Range<u16> {
    let low = if row == first.row { first.column } else { 0 };

    let high = if row == last.row { last.column.saturating_add(1) } else { u16::MAX };

    low.max(mark.skip)..high.min(mark.end())
}

/// The text between FIRST and LAST as Krabb wrote it: real line breaks kept, wrap-only
/// breaks undone, and labels, rails, and borders left out.
pub(super) fn extract(lines: &[Line<'_>], marks: &[Mark], first: Point, last: Point) -> String {
    let mut text = String::new();
    let mut started = false;
    let end = last.row.min(lines.len().min(marks.len()).saturating_sub(1));

    for row in first.row..=end {
        let mark = marks[row];

        if mark.joint == Joint::Hidden {
            continue;
        }

        if started {
            match mark.joint {
                Joint::Break => text.push('\n'),
                Joint::Space => text.push(' '),

                Joint::Glue | Joint::Hidden => {}
            }
        }

        started = true;

        let columns = span(mark, row, first, last);

        for (column, grapheme) in cells(&lines[row]) {
            let width = UnicodeWidthStr::width(grapheme).max(1).min(usize::from(u16::MAX)) as u16;

            if column < columns.end && column.saturating_add(width) > columns.start {
                text.extend(grapheme.chars().map(pasteable));
            }
        }
    }

    text
}

/// CHARACTER as it may reach a clipboard: control and invisible reordering marks become a
/// visible replacement, so pasted text cannot drive a terminal or hide what it says.
fn pasteable(character: char) -> char {
    let hidden = matches!(
        character,
        '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
    );

    if (character.is_control() && character != '\t') || hidden { '\u{fffd}' } else { character }
}

/// Reverses the selected cells of the rows drawn in AREA from transcript row OFFSET.
pub(super) fn highlight(
    buffer: &mut Buffer,
    area: Rect,
    offset: usize,
    marks: &[Mark],
    first: Point,
    last: Point,
) {
    for y in 0..area.height {
        let row = offset.saturating_add(usize::from(y));

        let Some(mark) = marks.get(row) else {
            break;
        };

        if row < first.row || row > last.row || mark.joint == Joint::Hidden {
            continue;
        }

        let columns = span(*mark, row, first, last);

        for x in columns.start..columns.end.min(area.width) {
            if let Some(cell) = buffer.cell_mut((area.x.saturating_add(x), area.y + y)) {
                cell.modifier.insert(Modifier::REVERSED);
            }
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Class {
    Space,
    Word,
    Other,
}

fn class(text: &str) -> Class {
    if text.chars().all(char::is_whitespace) {
        Class::Space
    } else if text.chars().all(|character| character.is_alphanumeric() || character == '_') {
        Class::Word
    } else {
        Class::Other
    }
}

/// The first and last columns of the word under COLUMN in a row, if a word is there.
pub(super) fn word(line: &Line<'_>, mark: Mark, column: u16) -> Option<(u16, u16)> {
    let cells = cells(line)
        .filter(|(start, _)| (mark.skip..mark.end()).contains(start))
        .map(|(start, grapheme)| {
            let width = UnicodeWidthStr::width(grapheme).max(1) as u16;

            (start, start.saturating_add(width), class(grapheme))
        })
        .collect::<Vec<_>>();

    let index = cells.iter().position(|(start, end, _)| (*start..*end).contains(&column))?;
    let kind = cells[index].2;

    if kind == Class::Space {
        return None;
    }

    let first = cells[..index].iter().rposition(|cell| cell.2 != kind).map_or(0, |at| at + 1);

    let last = cells[index..]
        .iter()
        .position(|cell| cell.2 != kind)
        .map_or(cells.len() - 1, |at| index + at - 1);

    Some((cells[first].0, cells[last].1.saturating_sub(1)))
}

/// The characters of the word around INDEX in INPUT.
pub(super) fn input_word(input: &[char], index: usize) -> Range<usize> {
    let index = index.min(input.len().saturating_sub(1));

    let Some(character) = input.get(index) else {
        return 0..0;
    };

    let kind = class(character.encode_utf8(&mut [0; 4]));

    if kind == Class::Space {
        return index..index;
    }

    let same = |character: &char| class(character.encode_utf8(&mut [0; 4])) == kind;
    let start =
        input[..index].iter().rposition(|character| !same(character)).map_or(0, |at| at + 1);

    let end = input[index..]
        .iter()
        .position(|character| !same(character))
        .map_or(input.len(), |at| index + at);

    start..end
}

#[cfg(test)]
mod tests {
    use ratatui::{
        style::{Modifier, Style},
        text::Span,
    };

    use super::*;

    fn point(row: usize, column: u16) -> Point {
        Point { row, column }
    }

    fn rows() -> Rows {
        let mut rows = Rows::default();

        rows.hidden(Line::raw("▌ Krabb"));
        rows.text(Line::raw("▌ alpha beta"), Joint::Break);
        rows.text(
            Line::from(vec![
                Span::raw("▌ "),
                Span::styled("gam", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw("ma"),
            ]),
            Joint::Space,
        );

        rows.text(Line::raw("▌ 界界x"), Joint::Glue);
        rows.text(Line::raw("▌ "), Joint::Break);
        rows.text(Line::raw("▌ end"), Joint::Break);
        rows.indent(2);

        for mark in &mut rows.marks {
            mark.width = mark.width.saturating_sub(2);
        }

        rows
    }

    #[test]
    fn copied_text_cannot_carry_control_sequences_or_reordering_marks() {
        let mut rows = Rows::default();

        rows.text(Line::raw("red \u{1b}]52;c;ZXZpbA==\u{7} \u{202e}txt\tok"), Joint::Break);

        let text = extract(&rows.lines, &rows.marks, point(0, 0), point(0, u16::MAX));

        assert_eq!(text, "red \u{fffd}]52;c;ZXZpbA==\u{fffd} \u{fffd}txt\tok");
    }

    #[test]
    fn extracts_text_without_rails_labels_or_wrap_breaks() {
        let rows = rows();
        let text = extract(&rows.lines, &rows.marks, point(0, 0), point(5, u16::MAX));

        assert_eq!(text, "alpha beta gamma界界x\n\nend");
        assert_eq!(extract(&rows.lines, &rows.marks, point(1, 8), point(2, 4)), "beta gam");
        assert_eq!(extract(&rows.lines, &rows.marks, point(3, 4), point(3, 4)), "界");
        assert_eq!(extract(&rows.lines, &rows.marks, point(3, 3), point(3, 3)), "界");
        assert_eq!(extract(&rows.lines, &rows.marks, point(3, 5), point(3, 6)), "界x");
        assert_eq!(extract(&rows.lines, &rows.marks, point(0, 0), point(0, 9)), "");
        assert_eq!(extract(&rows.lines, &rows.marks, point(9, 0), point(12, 0)), "");
    }

    #[test]
    fn highlights_only_copyable_cells_of_visible_rows() {
        let rows = rows();
        let area = Rect::new(1, 1, 8, 3);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 10, 5));

        highlight(&mut buffer, area, 1, &rows.marks, point(0, 0), point(2, 3));

        let reversed = |x: u16, y: u16| buffer[(x, y)].modifier.contains(Modifier::REVERSED);

        assert!(!reversed(1, 1) && !reversed(2, 1));
        assert!((3..9).all(|x| reversed(x, 1)));
        assert!(reversed(3, 2) && reversed(4, 2) && !reversed(5, 2));
        assert!((0..10).all(|x| !reversed(x, 3)));
    }

    #[test]
    fn finds_words_by_column_across_styles_and_wide_characters() {
        let rows = rows();

        assert_eq!(word(&rows.lines[2], rows.marks[2], 3), Some((2, 6)));
        assert_eq!(word(&rows.lines[1], rows.marks[1], 7), None);
        assert_eq!(word(&rows.lines[1], rows.marks[1], 0), None);
        assert_eq!(word(&rows.lines[3], rows.marks[3], 5), Some((2, 6)));

        let input = "say hello, world".chars().collect::<Vec<_>>();

        assert_eq!(input_word(&input, 5), 4..9);
        assert_eq!(input_word(&input, 9), 9..10);
        assert_eq!(input_word(&input, 3), 3..3);
        assert_eq!(input_word(&input, 99), 11..16);
        assert_eq!(input_word(&[], 0), 0..0);
    }

    #[test]
    fn orders_selection_ends_whichever_way_the_pointer_moved() {
        let selection = Selection::Transcript { anchor: point(4, 1), head: point(2, 9) };

        assert_eq!(selection.cells(), Some((point(2, 9), point(4, 1))));
        assert_eq!(Selection::Input { anchor: 5, head: 2 }.characters(), Some(2..5));
        assert_eq!(Selection::Input { anchor: 3, head: 3 }.characters(), None);
        assert_eq!(selection.characters(), None);
    }
}
