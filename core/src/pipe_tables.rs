//! Pipe tables, as GitHub Flavored Markdown defines them: read for an editor to draw, with the
//! source of each cell, and edited with Tab and Shift+Tab, which move between cells and line up the
//! columns, in the manner of Org mode's tables.

use std::fmt::Write as _;
use std::ops::Range;

use pulldown_cmark::{Event, Parser, Tag};
use unicode_width::UnicodeWidthStr;

use crate::body::{self, Alignment, ListTable};
use crate::lists::Edit;

/// The narrowest a column lines up to, which leaves room for a delimiter such as `:-:`.
const MIN_WIDTH: usize = 3;

/// A pipe table's lines in a body: for each, the byte range of its row, past any indentation, list
/// marker, or quote marker before it, and before its line break. The second line is the delimiter
/// row.
struct Table {
    rows: Vec<Range<usize>>,
}

impl Table {
    /// The table that pulldown-cmark found at `range`, which starts at its header row.
    fn found(body: &str, range: &Range<usize>) -> Self {
        let line_start = line_start(body, range.start);
        // Later lines hold the same indentation or quote markers as the first line's container,
        // whose width this is.
        let width = body[line_start..range.start].chars().count();
        let mut rows: Vec<Range<usize>> =
            std::iter::once(range.start..line_end(body, range.start)).collect();
        let mut start = rows[0].end + 1;
        while start < range.end.min(body.len()) {
            rows.push(row_in_line(body, start, width));
            start = rows.last().map_or(body.len(), |row| row.end) + 1;
        }
        Self { rows }
    }

    /// A table that pulldown-cmark doesn't read as one, around byte `at`, because its header row
    /// has more cells than its delimiter row, as after the owner adds a cell to it.
    fn unaligned(body: &str, at: usize) -> Option<Self> {
        let starts: Vec<usize> =
            std::iter::once(0).chain(body.match_indices('\n').map(|(i, _)| i + 1)).collect();
        let line = |i: usize| &body[starts[i]..line_end(body, starts[i])];
        let here = starts.partition_point(|&start| start <= at) - 1;
        let mut header = here;
        loop {
            if !has_pipe(line(header)) {
                return None;
            }
            if header + 1 < starts.len() && is_delimiter(line(header + 1)) {
                break;
            }
            header = header.checked_sub(1)?;
        }
        let delimiter = line(header + 1);
        let width = delimiter.chars().take_while(|c| matches!(c, ' ' | '\t' | '>')).count();
        // The header row starts where the delimiter row does, or at its first pipe, after any list
        // marker before it.
        let text = line(header);
        let skipped = text.char_indices().nth(width).map_or(text.len(), |(i, _)| i);
        let mut start = skipped + (text[skipped..].len() - text[skipped..].trim_start().len());
        if let Some(pipe) = text[..start].find('|') {
            start = pipe;
        }
        let mut rows: Vec<Range<usize>> =
            std::iter::once(starts[header] + start..starts[header] + text.len()).collect();
        for (i, &line_start) in starts.iter().enumerate().skip(header + 1) {
            if i > header + 1 && (line(i).trim().is_empty() || !has_pipe(line(i))) {
                break;
            }
            rows.push(row_in_line(body, line_start, width));
        }
        Some(Self { rows }).filter(|table| table.holds(body, at))
    }

    /// Whether byte `at` lies on the table's lines, past the start of its header row.
    fn holds(&self, body: &str, at: usize) -> bool {
        let last = self.rows.last().map_or(0, |row| row.end);
        self.rows[0].start <= at && at <= last && self.line_of(body, at).is_some()
    }

    /// The line that holds byte `at`.
    fn line_of(&self, body: &str, at: usize) -> Option<usize> {
        self.rows.iter().position(|row| line_start(body, row.start) <= at && at <= row.end)
    }

    fn cells(&self, body: &str, line: usize) -> Vec<Range<usize>> {
        let row = &self.rows[line];
        cells(&body[row.clone()])
            .into_iter()
            .map(|cell| row.start + cell.start..row.start + cell.end)
            .collect()
    }

    fn alignments(&self, body: &str) -> Vec<Alignment> {
        let delimiter = self.rows.get(1).map_or("", |row| &body[row.clone()]);
        cells(delimiter)
            .into_iter()
            .map(|cell| {
                let cell = delimiter[cell].trim();
                match (cell.starts_with(':'), cell.len() > 1 && cell.ends_with(':')) {
                    (true, true) => Alignment::Center,
                    (true, false) => Alignment::Left,
                    (false, true) => Alignment::Right,
                    (false, false) => Alignment::None,
                }
            })
            .collect()
    }

    /// The lines that hold rows of cells, which are every line but the delimiter row.
    fn row_lines(&self) -> impl Iterator<Item = usize> {
        std::iter::once(0).chain(2..self.rows.len())
    }
}

/// The start of the line that holds byte `at`.
fn line_start(body: &str, at: usize) -> usize {
    body[..at].rfind('\n').map_or(0, |i| i + 1)
}

/// The end of the line that holds byte `at`, before its line break.
fn line_end(body: &str, at: usize) -> usize {
    let end = body[at..].find('\n').map_or(body.len(), |i| at + i);
    if end > at && body.as_bytes()[end - 1] == b'\r' { end - 1 } else { end }
}

/// The row on the line starting at byte `start`, past up to `width` characters of indentation or
/// quote markers, and any white space after them.
fn row_in_line(body: &str, start: usize, width: usize) -> Range<usize> {
    let end = line_end(body, start);
    let text = &body[start..end];
    let prefix: usize = text
        .chars()
        .take(width)
        .take_while(|c| matches!(c, ' ' | '\t' | '>'))
        .map(char::len_utf8)
        .sum();
    let rest = &text[prefix..];
    start + prefix + (rest.len() - rest.trim_start().len())..end
}

/// The byte offsets of the pipes in `row` that no backslash escapes.
fn pipes(row: &str) -> Vec<usize> {
    let bytes = row.as_bytes();
    let mut pipes = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 1,
            b'|' => pipes.push(i),
            _ => {}
        }
        i += 1;
    }
    pipes
}

fn has_pipe(line: &str) -> bool {
    !pipes(line).is_empty()
}

/// The byte range of each cell in `row`, between its pipes, with the white space around its text.
/// A pipe at either end of the row only closes a cell.
fn cells(row: &str) -> Vec<Range<usize>> {
    let mut pipes = pipes(row);
    let leading = row.len() - row.trim_start().len();
    let trailing = row.trim_end().len();
    let start = if pipes.first() == Some(&leading) { pipes.remove(0) + 1 } else { 0 };
    let end = match pipes.last() {
        Some(&last) if last + 1 == trailing => {
            pipes.pop();
            last
        }
        _ => row.len(),
    };
    let mut cells = Vec::with_capacity(pipes.len() + 1);
    let mut from = start.min(end);
    for pipe in pipes {
        cells.push(from..pipe);
        from = pipe + 1;
    }
    cells.push(from..end);
    cells
}

/// The text of a cell, as a byte range of the body, without the white space around it.
fn trimmed(body: &str, cell: &Range<usize>) -> Range<usize> {
    let text = &body[cell.clone()];
    let start = cell.start + text.len() - text.trim_start().len();
    start..start + text.trim().len()
}

fn is_delimiter(line: &str) -> bool {
    let row = line.trim_start_matches([' ', '\t', '>']);
    has_pipe(row)
        && cells(row).iter().all(|cell| {
            let cell = row[cell.clone()].trim();
            let dashes = cell.strip_prefix(':').unwrap_or(cell);
            let dashes = dashes.strip_suffix(':').unwrap_or(dashes);
            !dashes.is_empty() && dashes.bytes().all(|b| b == b'-')
        })
}

/// Reads the pipe table at `range` of `body`, which an editor draws, as a table with one header
/// row and its columns' alignments. Each cell's markdown is its text, with each escaped pipe as a
/// pipe. Rows have as many cells as the header row.
pub fn read(body: &str, range: &Range<usize>) -> ListTable {
    let table = Table::found(body, range);
    let width = table.cells(body, 0).len();
    let rows = table.row_lines().map(|line| {
        let cells = table.cells(body, line);
        let text = |cell: &Range<usize>| body[trimmed(body, cell)].replace("\\|", "|");
        let mut row: Vec<String> = cells.iter().take(width).map(text).collect();
        row.resize(width, String::new());
        row
    });
    let alignments = table.alignments(body);
    ListTable { header_rows: 1, header_columns: 0, alignments, rows: rows.collect() }
}

/// The source of a cell's text in the pipe table at `range` of `body`, as a byte range of `body`:
/// the cell in `row`, counting the header row as 0, and `column`. An empty cell gives an empty
/// range after the space that starts it. `None` when the row has no such cell.
pub fn cell_source(
    body: &str,
    range: &Range<usize>,
    row: usize,
    column: usize,
) -> Option<Range<usize>> {
    let table = Table::found(body, range);
    let line = table.row_lines().nth(row)?;
    let cell = table.cells(body, line).into_iter().nth(column)?;
    let text = trimmed(body, &cell);
    if text.is_empty() {
        let at = (cell.start + 1).min(cell.end);
        return Some(at..at);
    }
    Some(text)
}

/// Returns the edit that Tab, or Shift+Tab when `back` is set, makes with the cursor at byte
/// `cursor` in a pipe table, with the byte range of the changed body to select, or `None` when the
/// cursor isn't in one.
///
/// The edit lines up the table's columns, gives every row as many cells as the longest row, and
/// moves to the next cell or the one before, selecting its text. Tab past the last cell adds a row.
/// A row with a new cell at its end, such as one the owner adds with ` |`, adds a column, and Tab
/// from after that cell's closing pipe moves into it.
pub fn table_edit(body: &str, cursor: usize, back: bool) -> Option<(Edit, Range<usize>)> {
    let table = table_at(body, cursor)?;
    let line = table.line_of(body, cursor)?;
    // Before a row's first pipe or after its last, the cursor is beside the table rather than in
    // it, unless the row has a cell past the table's columns, as after the owner adds ` |`.
    let cells = table.cells(body, line);
    let after = cells.last().is_none_or(|cell| cursor > cell.end);
    let delimited = table.cells(body, 1).len();
    let added = after && line != 1 && cells.len() > delimited;
    if !added && !cells.iter().any(|cell| cell.start <= cursor && cursor <= cell.end) {
        return None;
    }
    let lines: Vec<usize> = table.row_lines().collect();
    let mut rows: Vec<Vec<&str>> = lines
        .iter()
        .map(|&line| {
            table.cells(body, line).iter().map(|cell| &body[trimmed(body, cell)]).collect()
        })
        .collect();
    let mut alignments = table.alignments(body);
    let columns = rows.iter().map(Vec::len).chain([alignments.len(), 1]).max().unwrap_or(1);
    alignments.resize(columns, Alignment::None);

    // The delimiter row counts as the header row.
    let row = lines.iter().position(|&l| l == line).unwrap_or(0);
    let target = if added && !back {
        (row, delimited)
    } else {
        let column = cells.iter().position(|cell| cursor <= cell.end).unwrap_or(cells.len());
        let column = column.min(columns - 1);
        match (back, column + 1 < columns, row + 1 < rows.len()) {
            (false, true, _) => (row, column + 1),
            (false, false, true) => (row + 1, 0),
            (false, false, false) => {
                rows.push(Vec::new());
                (row + 1, 0)
            }
            (true, ..) if column > 0 => (row, column - 1),
            (true, ..) if row > 0 => (row - 1, columns - 1),
            (true, ..) => (0, 0),
        }
    };

    let widths: Vec<usize> = (0..columns)
        .map(|column| {
            let cells = rows.iter().filter_map(|row| row.get(column));
            cells.map(|cell| cell.width()).chain([MIN_WIDTH]).max().unwrap_or(MIN_WIDTH)
        })
        .collect();
    // Every line keeps as much of what comes before its row as the line with the least, which a
    // stray space before a row adds to. On the first line, that can be a list item's marker.
    let before = |line: usize| {
        let row = &table.rows[line];
        &body[line_start(body, row.start)..row.start]
    };
    let indent = (0..table.rows.len()).map(|line| before(line).chars().count()).min().unwrap_or(0);
    let prefix = |line: usize| {
        let before = before(line);
        &before[..before.char_indices().nth(indent).map_or(before.len(), |(i, _)| i)]
    };
    let start = line_start(body, table.rows[0].start) + prefix(0).len();
    let mut text = String::new();
    let mut selected = start..start;
    for (index, cells) in rows.iter().enumerate() {
        if index > 0 {
            // A new row takes the indentation of the line before it.
            let line = lines.get(index).copied().unwrap_or(table.rows.len() - 1);
            text.push('\n');
            text.push_str(prefix(line));
        }
        for (column, (&width, &align)) in widths.iter().zip(&alignments).enumerate() {
            let cell = cells.get(column).copied().unwrap_or("");
            let room = width - cell.width();
            let before = match align {
                Alignment::Right => room,
                Alignment::Center => room / 2,
                Alignment::None | Alignment::Left => 0,
            };
            text.push_str("| ");
            if (index, column) == target {
                let at = start + text.len() + if cell.is_empty() { 0 } else { before };
                selected = at..at + cell.len();
            }
            text.push_str(&" ".repeat(before));
            text.push_str(cell);
            text.push_str(&" ".repeat(room - before));
            text.push(' ');
        }
        text.push('|');
        if index == 0 {
            text.push('\n');
            text.push_str(prefix(1));
            for (&width, &align) in widths.iter().zip(&alignments) {
                let (left, right) = match align {
                    Alignment::None => ("", ""),
                    Alignment::Left => (":", ""),
                    Alignment::Center => (":", ":"),
                    Alignment::Right => ("", ":"),
                };
                let dashes = "-".repeat(width - left.len() - right.len());
                let _ = write!(text, "| {left}{dashes}{right} ");
            }
            text.push('|');
        }
    }
    let end = table.rows.last().map_or(start, |row| row.end);
    let edit = smallest(body, start..end, &text, selected.end);
    Some((edit, selected))
}

/// The table that holds byte `cursor`, unless it's in code.
fn table_at(body: &str, cursor: usize) -> Option<Table> {
    for (event, range) in Parser::new_ext(body, body::options()).into_offset_iter() {
        match event {
            Event::Start(Tag::Table(_)) => {
                let table = Table::found(body, &range);
                if table.holds(body, cursor) {
                    return Some(table);
                }
            }
            Event::Start(Tag::CodeBlock(_) | Tag::HtmlBlock) if range.contains(&cursor) => {
                return None;
            }
            _ => {}
        }
    }
    Table::unaligned(body, cursor)
}

/// The edit that replaces `range` of `body` with `text`, trimmed to the part that changes, so that
/// an edit that only moves the cursor changes nothing.
fn smallest(body: &str, range: Range<usize>, text: &str, cursor: usize) -> Edit {
    let old = &body[range.clone()];
    let same_start: usize =
        old.chars().zip(text.chars()).take_while(|(a, b)| a == b).map(|(c, _)| c.len_utf8()).sum();
    let same_end: usize = old[same_start..]
        .chars()
        .rev()
        .zip(text[same_start..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum();
    let changed = range.start + same_start..range.end - same_end;
    let text = text[same_start..text.len() - same_end].to_owned();
    Edit { cursor, range: changed, text }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lists::{ListKey, list_edit};

    /// Presses Tab, or Shift+Tab when `back` is set, in `before`, where `‸` marks the cursor, and
    /// returns the result, with `«` and `»` around the selection, or `‸` where the cursor is.
    fn tab(before: &str, back: bool) -> Option<String> {
        let cursor = before.find('‸').unwrap();
        let body = before.replacen('‸', "", 1);
        let (edit, selected) = table_edit(&body, cursor, back)?;
        let mut after = body;
        after.replace_range(edit.range, &edit.text);
        assert_eq!(edit.cursor, selected.end);
        if selected.is_empty() {
            after.insert(selected.start, '‸');
        } else {
            after.insert(selected.end, '»');
            after.insert(selected.start, '«');
        }
        Some(after)
    }

    #[test]
    fn tab_lines_up_and_moves_on() {
        for (before, back, after) in [
            (
                "|a|b‸|\n|-|-|\n|ccccc|d|",
                false,
                "| a     | b   |\n| ----- | --- |\n| «ccccc» | d   |",
            ),
            (
                "| a | b |\n|---|---|\n| c |‸ d |",
                true,
                "| a   | b   |\n| --- | --- |\n| «c»   | d   |",
            ),
            // Shift+Tab from the first cell of a row goes to the last cell of the row above.
            (
                "| a | b |\n|---|---|\n|‸ c | d |",
                true,
                "| a   | «b»   |\n| --- | --- |\n| c   | d   |",
            ),
            // Tab past the last cell adds a row.
            (
                "| a | b |\n|---|---|\n| c | d‸ |",
                false,
                "| a   | b   |\n| --- | --- |\n| c   | d   |\n| ‸    |     |",
            ),
            // Alignment survives, and places the text.
            (
                "| a | b | c |\n|:-|:-:|-:|\n| x‸ | y | z |",
                false,
                "| a   |  b  |   c |\n| :-- | :-: | --: |\n| x   |  «y»  |   z |",
            ),
            // Escaped pipes stay in their cell, and wide characters take two columns.
            (
                "| a\\|b | 表 |\n|---|---|\n| ‸c | d |",
                false,
                "| a\\|b | 表  |\n| ---- | --- |\n| c    | «d»   |",
            ),
        ] {
            assert_eq!(tab(before, back).as_deref(), Some(after), "{before:?}");
        }
    }

    #[test]
    fn a_new_cell_adds_a_column() {
        // In a body row, the table stays a table.
        assert_eq!(
            tab("| a | b |\n|---|---|\n| c | d | |‸", false).as_deref(),
            Some("| a   | b   |     |\n| --- | --- | --- |\n| c   | d   | ‸    |"),
        );
        // In the header row, it stops being one until the delimiter row catches up.
        assert_eq!(
            tab("Intro\n\n| a | b | |‸\n|---|---|\n| c | d |", false).as_deref(),
            Some("Intro\n\n| a   | b   | ‸    |\n| --- | --- | --- |\n| c   | d   |     |"),
        );
    }

    #[test]
    fn tab_lines_up_rows_that_stray() {
        let aligned = concat!(
            "| a    | b    | c   | d    |\n",
            "| ---- | ---- | --- | ---- |\n",
            "| what | yes? | no  | blah |",
        );
        let strays = [
            aligned.replace("\n| -", "\n | -"),
            aligned.replace("\n| w", "\n   | w"),
            format!(" {aligned}"),
        ];
        for stray in strays {
            let before = stray.replacen("blah", "blah‸", 1);
            let after = format!("{aligned}\n| ‸     |      |     |      |");
            assert_eq!(tab(&before, false), Some(after), "{before:?}");
        }
        // In a list item, rows line up with the item's text.
        assert_eq!(
            tab("- | a‸ | b |\n   |---|---|\n  | c | d |", false).as_deref(),
            Some("- | a   | «b»   |\n  | --- | --- |\n  | c   | d   |"),
        );
    }

    #[test]
    fn tables_in_list_items_keep_their_indentation() {
        assert_eq!(
            tab("- item\n\n  | a | b |\n  |---|---|\n  | c | d‸ |", false).as_deref(),
            Some("- item\n\n  | a   | b   |\n  | --- | --- |\n  | c   | d   |\n  | ‸    |     |"),
        );
        assert_eq!(
            tab("- | a‸ | b |\n  |---|---|", false).as_deref(),
            Some("- | a   | «b»   |\n  | --- | --- |"),
        );
    }

    #[test]
    fn tab_outside_tables_does_nothing() {
        for before in [
            "- item‸\n\n  | a | b |\n  |---|---|",
            "‸- | a | b |\n  |---|---|",
            "```\n| a | b |\n|---|---|‸\n```",
            "Just | a pipe‸",
        ] {
            assert_eq!(tab(before, false), None, "{before:?}");
        }
    }

    #[test]
    fn tab_beside_a_table_in_a_list_item_moves_the_item() {
        let item = |row: &str| {
            let rows = format!("    {row}\n    | --- | --- |\n    | c   | d   |\n");
            format!("- a parent\n  - a list item\n{rows}\n    something")
        };
        // Before the first pipe or after the last, Tab and Shift+Tab move the item.
        for row in ["‸| a   | b   |", "| a   | b   |‸"] {
            let before = item(row);
            let cursor = before.find('‸').unwrap();
            let body = before.replacen('‸', "", 1);
            for back in [false, true] {
                assert_eq!(table_edit(&body, cursor, back), None, "{before:?}");
                let key = if back { ListKey::ShiftTab } else { ListKey::Tab };
                assert!(list_edit(&body, cursor, key).is_some(), "{before:?}");
            }
        }
        // In a cell, Tab moves to the next cell.
        assert_eq!(tab(&item("|‸ a   | b   |"), false), Some(item("| a   | «b»   |")));
    }

    #[test]
    fn an_aligned_table_changes_nothing() {
        let body = "| a   | b   |\n| --- | --- |";
        let (edit, selected) = table_edit(body, 2, false).unwrap();
        assert!(edit.range.is_empty() && edit.text.is_empty());
        assert_eq!(&body[selected], "b");
    }

    #[test]
    fn reads_tables_and_their_cells() {
        let body = "Intro\n\n- | a | b \\| c |\n  |---|---|\n  | d |\n";
        let range = drawn_range(body);
        let table = read(body, &range);
        assert_eq!(table.rows, [vec!["a", "b | c"], vec!["d", ""]]);
        assert_eq!(table.alignments, [Alignment::None; 2]);
        let aligned = "| a | b | c | d |\n|:-|:-:|-:|-|\n";
        assert_eq!(
            read(aligned, &drawn_range(aligned)).alignments,
            [Alignment::Left, Alignment::Center, Alignment::Right, Alignment::None],
        );
        assert_eq!(&body[cell_source(body, &range, 0, 1).unwrap()], "b \\| c");
        assert_eq!(&body[cell_source(body, &range, 1, 0).unwrap()], "d");
        assert_eq!(cell_source(body, &range, 1, 1), None);
    }

    fn drawn_range(body: &str) -> Range<usize> {
        Parser::new_ext(body, body::options())
            .into_offset_iter()
            .find_map(|(event, range)| {
                matches!(event, Event::Start(Tag::Table(_))).then_some(range)
            })
            .unwrap()
    }
}
