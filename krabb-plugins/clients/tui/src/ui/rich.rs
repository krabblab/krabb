use std::ops::Range;

use krabb_text::{Document, Style as TextStyle};

use ratatui::{
    buffer::{Buffer, CellDiffOption},
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use unicode_width::UnicodeWidthStr;

use super::selection::{Joint, Link, Mark, Rows};

/// Bullets for each list depth, so nested lists read apart.
const BULLETS: [&str; 3] = ["•", "◦", "▪"];

/// The narrowest column a table gives a cell before it lays its rows out as plain lines.
const CELL_MIN: usize = 3;

/// Links tagged in one render. Later links still show their address but open no link.
const LINK_TAGS: usize = 256;

type Segments = Vec<(String, Style)>;

pub(super) fn render(source: &str, width: usize) -> Rows {
    let mut rows = Rows::default();

    if source.len() > krabb_text::LIMIT {
        for line in source.lines() {
            rows.append(super::wrap_styled_segments(&[(visible(line), Style::default())], width));
        }

        return rows;
    }

    let document = Document::parse(source, true);

    let links = document
        .spans
        .iter()
        .filter_map(|span| match &span.style {
            TextStyle::Link(url) => Some((span.start..span.end, url.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();

    let runs = styles(&document, &links);

    let items = document
        .spans
        .iter()
        .filter(|span| span.style == TextStyle::Item)
        .map(|span| span.start..span.end)
        .collect::<Vec<_>>();

    let blocks = document
        .spans
        .iter()
        .filter(|span| matches!(span.style, TextStyle::Pre(_) | TextStyle::Table))
        .collect::<Vec<_>>();

    let mut next = 0;
    let mut offset = 0;

    while offset < document.text.len() {
        while blocks.get(next).is_some_and(|span| span.start < offset) {
            next += 1;
        }

        if let Some(span) = blocks.get(next).filter(|span| span.start == offset) {
            let text = &document.text[span.start..span.end];
            let text = text.strip_suffix('\n').unwrap_or(text);

            if let TextStyle::Pre(language) = &span.style {
                let lines = text.split('\n').map(visible).collect::<Vec<_>>();
                let lines = lines.iter().map(String::as_str).collect::<Vec<_>>();
                let language = if language.is_empty() { "code" } else { language };

                rows.append(super::wrap_code_block(&lines, language, width));
            } else {
                let range = span.start..span.start + text.len();

                rows.append(table(&cells(&document, &runs, &links, range), width));
            }

            offset = span.end;
            next += 1;
            continue;
        }

        let line_end =
            document.text[offset..].find('\n').map_or(document.text.len(), |n| offset + n);

        let end = blocks.get(next).map_or(line_end, |span| line_end.min(span.start));
        let segments = segments(&document, &runs, &links, offset..end);

        let item = items
            .iter()
            .filter(|item| item.start <= offset && offset < item.end)
            .max_by_key(|item| item.start);

        rows.append(match item {
            Some(item) => list_line(&document.text[item.start..], item.start == offset, segments, width),
            None => super::wrap_styled_segments(&segments, width),
        });

        let starts_block = blocks.get(next).is_some_and(|span| span.start == end);
        offset = end.saturating_add(usize::from(!starts_block));
    }

    tag_links(&mut rows, &links);
    rows
}

/// The visible pieces of the document text in RANGE with their styles, and each link's
/// address after its label, so it shows wherever the terminal cannot open the link.
fn segments(
    document: &Document,
    runs: &[(Range<usize>, Style)],
    links: &[(Range<usize>, String)],
    range: Range<usize>,
) -> Segments {
    let first = runs.partition_point(|(run, _)| run.end <= range.start);
    let mut segments = Vec::new();

    for (run, style) in runs[first..].iter().take_while(|(run, _)| run.start < range.end) {
        let end = run.end.min(range.end);

        segments.push((visible(&document.text[run.start.max(range.start)..end]), *style));

        let link = links.iter().position(|(span, _)| span.end == end && span.start < end);

        if let Some(index) = link
            && run.end == end
            && document.text[links[index].0.clone()] != links[index].1
        {
            let style = tag(Style::default().fg(Color::DarkGray), index);

            segments.push((format!(" ({})", visible(&links[index].1)), style));
        }
    }

    segments
}

/// The marker that opens a list item in TEXT: its indent, bullet or number, and checkbox.
fn marker(text: &str) -> (usize, &str, Option<bool>) {
    let indent = text.bytes().take_while(|byte| *byte == b' ').count();
    let rest = &text[indent..];

    let bullet = if rest.starts_with("• ") {
        "• ".len()
    } else {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();

        if digits > 0 && rest[digits..].starts_with(". ") { digits + 2 } else { 0 }
    };

    let label = &rest[..bullet];
    let rest = &rest[bullet..];

    let checked = if rest.starts_with("[x] ") {
        Some(true)
    } else if rest.starts_with("[ ] ") {
        Some(false)
    } else {
        None
    };

    (indent, label, checked)
}

/// Rows for one line of a list item whose text starts at ITEM. Its FIRST line shows the
/// marker, and every row after the marker hangs under the item's text.
fn list_line(item: &str, first: bool, segments: Segments, width: usize) -> Rows {
    let (indent, label, checked) = marker(item);
    let depth = indent / 2;
    let checkbox = if checked.is_some() { "[ ] ".len() } else { 0 };
    let hang = indent + UnicodeWidthStr::width(label) + checkbox;

    if label.is_empty() || width < hang + CELL_MIN {
        return super::wrap_styled_segments(&segments, width);
    }

    let accent = Style::default().fg(super::ACCENT_COLOR).add_modifier(Modifier::BOLD);

    let prefix = if first {
        let bullet = if label.starts_with('•') {
            format!("{} ", BULLETS[depth % BULLETS.len()])
        } else {
            label.to_owned()
        };

        let mut prefix = vec![(" ".repeat(indent), Style::default()), (bullet, accent)];

        match checked {
            Some(true) => prefix.push(("[✓] ".to_owned(), accent)),
            Some(false) => prefix.push(("[ ] ".to_owned(), Style::default().fg(Color::DarkGray))),
            None => {}
        }

        prefix
    } else {
        Vec::new()
    };

    let skip = if first { indent + label.len() + checkbox } else { 0 };
    let rest = without(segments, skip);
    let mut rows = super::wrap_styled_segments(&rest, width - hang);
    let columns = hang.min(usize::from(u16::MAX)) as u16;

    for (index, (line, mark)) in rows.lines.iter_mut().zip(&mut rows.marks).enumerate() {
        if index == 0 && first {
            let spans = prefix.iter().map(|(text, style)| Span::styled(text.clone(), *style));

            line.spans.splice(0..0, spans);
            mark.width = mark.width.saturating_add(columns);
        } else {
            line.spans.insert(0, Span::raw(" ".repeat(hang)));
            mark.skip = mark.skip.saturating_add(columns);
        }
    }

    rows
}

/// SEGMENTS without their first BYTES bytes.
fn without(segments: Segments, mut bytes: usize) -> Segments {
    let mut rest = Vec::new();

    for (text, style) in segments {
        if bytes >= text.len() {
            bytes -= text.len();
        } else {
            let mut start = bytes;

            while !text.is_char_boundary(start) {
                start += 1;
            }

            rest.push((text[start..].to_owned(), style));
            bytes = 0;
        }
    }

    rest
}

/// The cells of the table at RANGE: rows split at line breaks and cells at tabs.
fn cells(
    document: &Document,
    runs: &[(Range<usize>, Style)],
    links: &[(Range<usize>, String)],
    range: Range<usize>,
) -> Vec<Vec<Segments>> {
    let mut table = Vec::new();
    let mut row_start = range.start;

    for row in document.text[range.clone()].split('\n') {
        let mut cells = Vec::new();
        let mut cell_start = row_start;

        for cell in row.split('\t') {
            let cell_range = cell_start..cell_start + cell.len();

            cells.push(segments(document, runs, links, cell_range));
            cell_start += cell.len() + 1;
        }

        table.push(cells);
        row_start += row.len() + 1;
    }

    table
}

fn width_of(segments: &Segments) -> usize {
    segments.iter().map(|(text, _)| UnicodeWidthStr::width(text.as_str())).sum()
}

/// Column widths that fit AVAILABLE columns: natural widths when they fit, otherwise the
/// narrow columns keep theirs and the wide ones share what is left.
fn fit(natural: &[usize], available: usize) -> Vec<usize> {
    if natural.iter().sum::<usize>() <= available {
        return natural.to_vec();
    }

    let mut order = (0..natural.len()).collect::<Vec<_>>();
    let mut widths = vec![0; natural.len()];
    let mut remaining = available;

    order.sort_by_key(|column| natural[*column]);

    for (placed, column) in order.into_iter().enumerate() {
        let share = remaining / (natural.len() - placed);
        let width = natural[column].min(share).max(1);

        widths[column] = width;
        remaining = remaining.saturating_sub(width);
    }

    widths
}

/// A bordered table whose cells wrap within their columns, or plain rows when the width
/// cannot hold a column for each cell.
fn table(table: &[Vec<Segments>], width: usize) -> Rows {
    let columns = table.iter().map(Vec::len).max().unwrap_or(0);
    let chrome = 3 * columns + 1;
    let header = Style::default().add_modifier(Modifier::BOLD);
    let mut rows = Rows::default();

    if columns == 0 || width < chrome + CELL_MIN * columns {
        for (index, row) in table.iter().enumerate() {
            let mut segments = Vec::new();

            for (column, cell) in row.iter().enumerate() {
                if column > 0 {
                    segments.push((" | ".to_owned(), Style::default().fg(Color::DarkGray)));
                }

                segments.extend(cell.iter().map(|(text, style)| {
                    (text.clone(), if index == 0 { style.patch(header) } else { *style })
                }));
            }

            rows.append(super::wrap_styled_segments(&segments, width));
        }

        return rows;
    }

    let natural = (0..columns)
        .map(|column| {
            table.iter().map(|row| row.get(column).map_or(0, width_of)).max().unwrap_or(0).max(1)
        })
        .collect::<Vec<_>>();

    let widths = fit(&natural, width - chrome);
    let border = Style::default().fg(Color::DarkGray);

    let rule = |left: &str, middle: &str, right: &str| {
        let parts = widths.iter().map(|width| "─".repeat(width + 2)).collect::<Vec<_>>();

        Line::styled(format!("{left}{}{right}", parts.join(middle)), border)
    };

    rows.hidden(rule("┌", "┬", "┐"));

    for (index, row) in table.iter().enumerate() {
        let wrapped = widths
            .iter()
            .enumerate()
            .map(|(column, width)| {
                let cell = row.get(column).cloned().unwrap_or_default();

                let cell = cell
                    .into_iter()
                    .map(|(text, style)| (text, if index == 0 { style.patch(header) } else { style }))
                    .collect::<Vec<_>>();

                super::wrap_styled_segments(&cell, *width).lines
            })
            .collect::<Vec<_>>();

        let height = wrapped.iter().map(Vec::len).max().unwrap_or(1);

        for line in 0..height {
            let mut spans = vec![Span::styled("│ ", border)];

            for (column, cell) in wrapped.iter().enumerate() {
                let content = cell.get(line).cloned().unwrap_or_default();
                let padding = widths[column].saturating_sub(content.width());

                spans.extend(content.spans);
                spans.push(Span::raw(" ".repeat(padding)));
                spans.push(Span::styled(if column + 1 < columns { " │ " } else { " │" }, border));
            }

            let line = Line::from(spans);
            let copied = line.width().saturating_sub(4).min(usize::from(u16::MAX)) as u16;

            rows.push(line, Mark { skip: 2, width: copied, joint: Joint::Break, action: None });
        }

        if index == 0 && table.len() > 1 {
            rows.hidden(rule("├", "┼", "┤"));
        }
    }

    rows.hidden(rule("└", "┴", "┘"));
    rows
}

/// Marks STYLE as part of link INDEX until `tag_links` reads the mark back.
fn tag(style: Style, index: usize) -> Style {
    if index < LINK_TAGS {
        style.underline_color(Color::Indexed(index as u8))
    } else {
        style
    }
}

/// Turns the link marks left in ROWS by `tag` into links, now that wrapping has placed them.
fn tag_links(rows: &mut Rows, links: &[(Range<usize>, String)]) {
    for (row, line) in rows.lines.iter_mut().enumerate() {
        let mut column = 0_u16;

        for span in &mut line.spans {
            let width = span.width().min(usize::from(u16::MAX)) as u16;

            if let Some(Color::Indexed(index)) = span.style.underline_color
                && let Some((_, target)) = links.get(usize::from(index))
            {
                span.style.underline_color = None;

                match rows.links.last_mut() {
                    Some(last)
                        if last.row == row
                            && last.columns.end == column
                            && last.target == *target =>
                    {
                        last.columns.end = column.saturating_add(width);
                    }

                    _ => rows.links.push(Link {
                        row,
                        columns: column..column.saturating_add(width),
                        target: target.clone(),
                    }),
                }
            }

            column = column.saturating_add(width);
        }
    }
}

fn styles(document: &Document, links: &[(Range<usize>, String)]) -> Vec<(Range<usize>, Style)> {
    let mut events = Vec::new();

    for span in &document.spans {
        let kind = match span.style {
            TextStyle::Bold => 0,
            TextStyle::Italic => 1,
            TextStyle::Strike => 2,
            TextStyle::Code => 3,
            TextStyle::Quote => 4,
            TextStyle::Heading(_) => 5,
            TextStyle::Link(_) => 6,
            _ => continue,
        };

        events.push((span.start, kind, true));
        events.push((span.end, kind, false));
    }

    events.sort_unstable();

    let mut active = [0_usize; 7];
    let mut offset = 0;
    let mut runs = Vec::new();

    let style_at = |active: [usize; 7], offset: usize| {
        let link = (active[6] > 0)
            .then(|| links.iter().position(|(range, _)| range.contains(&offset)))
            .flatten();

        style(active, link)
    };

    for (position, kind, start) in events {
        if position > offset {
            runs.push((offset..position, style_at(active, offset)));
            offset = position;
        }

        if start {
            active[kind] += 1;
        } else {
            active[kind] -= 1;
        }
    }

    if offset < document.text.len() {
        runs.push((offset..document.text.len(), style_at(active, offset)));
    }

    runs
}

fn style(active: [usize; 7], link: Option<usize>) -> Style {
    let mut style = Style::default();

    for (count, modifier) in active.into_iter().zip([
        Modifier::BOLD,
        Modifier::ITALIC,
        Modifier::CROSSED_OUT,
        Modifier::DIM,
        Modifier::ITALIC,
        Modifier::BOLD,
        Modifier::UNDERLINED,
    ]) {
        if count > 0 {
            style = style.add_modifier(modifier);
        }
    }

    if active[3] > 0 || active[5] > 0 {
        style = style.fg(super::ACCENT_COLOR);
    }

    match link {
        Some(index) => tag(style, index),
        None => style,
    }
}

/// Wraps the cells of LINK on row Y of AREA in OSC 8 sequences, so terminals that support
/// them open its target on click and others show the text unchanged.
pub(super) fn hyperlink(buffer: &mut Buffer, area: Rect, y: u16, link: &Link) {
    for column in link.columns.start..link.columns.end.min(area.width) {
        let Some(cell) = buffer.cell_mut((area.x + column, area.y + y)) else {
            break;
        };

        let symbol = cell.symbol().to_owned();
        let width = UnicodeWidthStr::width(symbol.as_str()).min(usize::from(u16::MAX)) as u16;

        let Some(width) = std::num::NonZeroU16::new(width) else {
            continue;
        };

        cell.set_symbol(&format!("\x1b]8;;{}\x1b\\{symbol}\x1b]8;;\x1b\\", link.target));
        cell.set_diff_option(CellDiffOption::ForcedWidth(width));
    }
}

pub(super) fn visible(text: &str) -> String {
    let mut visible = String::new();

    for character in text.chars() {
        match character {
            '\t' => visible.push_str("    "),
            character if character.is_control() => visible.push('�'),
            character => visible.push(character),
        }
    }

    visible
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(rows: &Rows) -> String {
        rows.lines
            .iter()
            .map(|row| row.spans.iter().map(|span| span.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn renders_nested_emphasis_code_headings_quotes_and_links() {
        let rows = render(
            "# Today\n\n**bold *both*** and _italic_ and ~~old~~ and `a_b * c`.\n\n\
             > quoted\n\n[ANSA](https://www.ansa.it/english/news/)",
            100,
        );

        assert_eq!(
            text(&rows),
            "Today\n\nbold both and italic and old and a_b * c.\n\n\
             quoted\n\nANSA (https://www.ansa.it/english/news/)"
        );

        let spans = rows.lines.iter().flat_map(|row| &row.spans).collect::<Vec<_>>();
        let styled = |word: &str, modifiers| {
            spans.iter().any(|span| {
                span.content.contains(word) && span.style.add_modifier.contains(modifiers)
            })
        };

        assert!(styled("Today", Modifier::BOLD));
        assert!(styled("bold", Modifier::BOLD));
        assert!(styled("both", Modifier::BOLD | Modifier::ITALIC));
        assert!(styled("italic", Modifier::ITALIC));
        assert!(styled("old", Modifier::CROSSED_OUT));
        assert!(styled("a_b * c", Modifier::DIM));

        assert!(spans.iter().any(|span| {
            span.content.contains("a_b * c") && span.style.fg == Some(super::super::ACCENT_COLOR)
        }));

        assert!(styled("quoted", Modifier::ITALIC));
        assert!(
            spans
                .iter()
                .any(|span| { span.content.contains(" and ") && span.style == Style::default() })
        );
    }

    #[test]
    fn wraps_unicode_styles_and_keeps_unmatched_or_escaped_markers_literal() {
        let rows = render("**こんにちは e\u{301} 👩‍💻 world** plain", 10);

        assert!(rows.lines.iter().all(|row| row.width() <= 10));
        assert!(rows.lines.iter().flat_map(|row| &row.spans).any(|span| {
            span.content.contains("👩‍💻") && span.style.add_modifier.contains(Modifier::BOLD)
        }));

        assert_eq!(
            text(&render(r"\*literal\* foo_bar **unfinished", 80)),
            "*literal* foo_bar **unfinished"
        );

        assert_eq!(text(&render("**first\nsecond**", 80)), "first\nsecond");
        assert!(render("**first\nsecond**", 80).lines.iter().all(|row| {
            row.spans.iter().all(|span| span.style.add_modifier.contains(Modifier::BOLD))
        }));
    }

    #[test]
    fn preserves_indented_code_and_neutralizes_control_characters() {
        let rows = render("Example:\n\n    **literal**\n    \tcode\n\nEnd.", 40);

        assert!(text(&rows).contains("┌─ code"));
        assert!(text(&rows).contains("**literal**"));
        assert!(text(&rows).contains("    code"));
        assert!(text(&rows).ends_with("End."));
        assert_eq!(text(&render("a\u{1b}[31m\u{7}\tb", 80)), "a�[31m�    b");

        assert!(render("", 80).lines.is_empty());
    }

    #[test]
    fn renders_code_nested_in_a_list_before_later_blocks() {
        let rows = render("-     first\n\n      second\n\nAfter\n\n    third", 40);
        let rendered = text(&rows);

        assert_eq!(rendered.matches("┌─ code").count(), 2);
        assert!(rendered.contains("first"));
        assert!(rendered.contains("second"));
        assert!(rendered.contains("After"));
        assert!(rendered.contains("third"));

        let rows = render("- ```rust\n\n  **literal**\n  ```", 40);
        let rendered = text(&rows);

        assert!(rendered.contains("┌─ rust"));
        assert!(rendered.contains("**literal**"));
    }

    #[test]
    fn oversized_text_is_preserved_with_plain_fallback() {
        let source = format!("{}\nTHE END", "x".repeat(krabb_text::LIMIT + 1));
        let rows = render(&source, 80);

        assert_eq!(text(&rows).replace('\n', ""), source.replace('\n', ""));
        assert!(rows.lines.iter().all(|row| row.width() <= 80));
    }

    #[test]
    fn every_streamed_prefix_remains_renderable() {
        let source = "**bold _nested_** `code` [link](https://example.com)\n\n> 界\n\n\
            - [x] done and a long item\n  1. nested\n\n| a | 界 b |\n|---|---|\n| `1` | two |\n\n\
            ```rust\nfn main() { \"x\" } // c\n```";

        for end in source.char_indices().map(|(offset, _)| offset).chain([source.len()]) {
            for width in [6, 12, 30] {
                let rows = render(&source[..end], width);

                assert!(rows.lines.iter().all(|row| row.width() <= width), "{end} {width}");
                assert_eq!(rows.lines.len(), rows.marks.len());
            }
        }
    }

    fn spans_of(rows: &Rows) -> Vec<&Span<'static>> {
        rows.lines.iter().flat_map(|row| &row.spans).collect()
    }

    fn styled(rows: &Rows, text: &str) -> Style {
        spans_of(rows).into_iter().find(|span| span.content.trim() == text).unwrap().style
    }

    #[test]
    fn highlights_known_code_and_leaves_unknown_languages_plain() {
        let rows = render("```rust\nfn main() { let x = 1; } // hi\n```", 60);

        assert_eq!(styled(&rows, "fn").fg, Some(super::super::ACCENT_COLOR));
        assert!(styled(&rows, "fn").add_modifier.contains(Modifier::BOLD));
        assert_eq!(styled(&rows, "1").fg, Some(Color::Magenta));
        assert_eq!(styled(&rows, "// hi").fg, Some(Color::DarkGray));

        let rows = render("```elixir\ndefmodule Hello do\n```", 60);

        assert!(text(&rows).contains("defmodule Hello do"));
        assert_eq!(styled(&rows, "defmodule Hello do").fg, Some(Color::Reset));

        let rows = render("```diff\n--- a\n+++ b\n@@ -1 +1 @@\n-old\n+new\n```", 60);

        assert_eq!(styled(&rows, "-old").fg, Some(Color::Red));
        assert_eq!(styled(&rows, "+new").fg, Some(Color::Green));
        assert!(styled(&rows, "--- a").add_modifier.contains(Modifier::BOLD));

        let rows = render("```\n--- a\n+++ b\n@@ -1 +1 @@\n+new\n```", 60);

        assert_eq!(styled(&rows, "+new").fg, Some(Color::Green));
    }

    #[test]
    fn draws_tables_aligned_with_borders_and_wraps_cells_to_fit() {
        let source = "| Name | Value |\n|---|---|\n| alpha | `1` |\n| b | a much longer value |";
        let rows = render(source, 60);
        let rendered = text(&rows);

        assert_eq!(
            rendered,
            "┌───────┬─────────────────────┐\n\
             │ Name  │ Value               │\n\
             ├───────┼─────────────────────┤\n\
             │ alpha │ 1                   │\n\
             │ b     │ a much longer value │\n\
             └───────┴─────────────────────┘"
        );

        assert!(styled(&rows, "Name").add_modifier.contains(Modifier::BOLD));
        assert_eq!(styled(&rows, "1").fg, Some(super::super::ACCENT_COLOR));
        assert_eq!(rows.marks[0].joint, Joint::Hidden);
        assert_eq!(rows.marks[1], Mark { skip: 2, width: 27, joint: Joint::Break, action: None });

        let narrow = render(source, 22);
        let widths = narrow.lines.iter().map(Line::width).collect::<Vec<_>>();

        assert!(widths.iter().all(|width| *width == widths[0] && *width <= 22), "{widths:?}");
        assert!(narrow.lines.len() > rows.lines.len());
        assert!(text(&narrow).contains("│ b     │ a much     │"));

        assert_eq!(
            text(&render(source, 12)),
            "Name | Value\nalpha | 1\nb | a much\nlonger value"
        );
    }

    #[test]
    fn nests_lists_with_hanging_indents_and_shows_task_state() {
        let rows = render(
            "- first item wraps here\n  - nested point\n    1. deep\n- [x] done\n- [ ] todo\n\n\
             3. three\n4. four",
            16,
        );

        assert_eq!(
            text(&rows),
            "• first item\n  wraps here\n  ◦ nested point\n    1. deep\n\
             • [✓] done\n• [ ] todo\n\n3. three\n4. four"
        );

        assert_eq!(rows.marks[1].skip, 2);
        assert_eq!(rows.marks[1].joint, Joint::Space);
        assert_eq!(styled(&rows, "[✓]").fg, Some(super::super::ACCENT_COLOR));
        assert_eq!(styled(&rows, "[ ]").fg, Some(Color::DarkGray));
        assert_eq!(styled(&rows, "◦").fg, Some(super::super::ACCENT_COLOR));
    }

    #[test]
    fn records_links_after_wrapping_and_draws_them_as_osc_8() {
        let rows = render("See [ANSA](https://ansa.it) now", 80);

        assert_eq!(text(&rows), "See ANSA (https://ansa.it) now");

        assert_eq!(
            rows.links,
            [Link { row: 0, columns: 4..26, target: "https://ansa.it".into() }]
        );

        assert!(spans_of(&rows).iter().all(|span| span.style.underline_color.is_none()));
        assert!(styled(&rows, "ANSA").add_modifier.contains(Modifier::UNDERLINED));

        let rows = render("[a](https://a.example) and <https://b.example>", 14);

        assert!(rows.links.iter().any(|link| link.target == "https://a.example"));
        assert!(rows.links.iter().any(|link| link.target == "https://b.example"));
        assert!(rows.links.iter().all(|link| link.columns.end <= 14));

        let mut buffer = Buffer::empty(Rect::new(0, 0, 6, 1));

        buffer.set_string(0, 0, "ab界", Style::default());

        let link = Link { row: 0, columns: 1..4, target: "https://x.example".into() };

        hyperlink(&mut buffer, Rect::new(0, 0, 6, 1), 0, &link);

        assert_eq!(buffer[(0, 0)].symbol(), "a");
        assert_eq!(buffer[(1, 0)].symbol(), "\x1b]8;;https://x.example\x1b\\b\x1b]8;;\x1b\\");
        assert_eq!(buffer[(1, 0)].diff_option, CellDiffOption::ForcedWidth(std::num::NonZeroU16::new(1).unwrap()));
        assert_eq!(buffer[(2, 0)].diff_option, CellDiffOption::ForcedWidth(std::num::NonZeroU16::new(2).unwrap()));
    }
}
