//! Outliner-style editing of Markdown lists, in the manner of Workflowy.

use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ListKey {
    /// Starts a new item, or moves a blank item out a level, or ends the list. Before the marker of
    /// a list's first item, it inserts a blank line above the list.
    Enter,
    /// Starts a continuation line in the current item.
    ShiftEnter,
    /// Moves the item, with its children, out a level.
    ShiftTab,
    /// Moves the item, with its children, under the previous sibling.
    Tab,
}

/// A change to a note's body: replace `range` with `text`, then put the cursor at the byte offset
/// `cursor` of the changed body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Edit {
    pub cursor: usize,
    pub range: Range<usize>,
    pub text: String,
}

/// Returns the edit that `key` makes with the cursor at byte `cursor`, or `None` if the cursor
/// isn't in a list item, so the key should do what it does in ordinary text. In a list, Tab and
/// Shift+Tab always return an edit, which may change nothing, so they never insert a tab. Lists in
/// code blocks are code, except in a `table` block, whose content is a list table's list. In other
/// code, Enter and Shift+Enter keep the line's indentation, and Tab and Shift+Tab move the list
/// item that holds the block.
pub fn list_edit(body: &str, cursor: usize, key: ListKey) -> Option<Edit> {
    let tab = matches!(key, ListKey::Tab | ListKey::ShiftTab);
    match code_block(body, cursor) {
        Code::Outside => edit(body, cursor, key, None),
        Code::Inside { fence, .. } if tab => edit(body, cursor, key, Some(fence)),
        Code::Inside { indent, .. } => code_newline(body, cursor, indent),
        // A new item or a continuation line can follow a code block in a list item.
        Code::Closed { fence } => edit(body, cursor, key, Some(fence)),
        Code::Table { content, indent } => {
            // The content edits as a note of its own, without the indentation of a table nested
            // in a list item, which would otherwise read as an indented code block. It may hold
            // code blocks of its own, such as a table in a cell.
            let lines = Dedented::new(&body[content.clone()], indent);
            let at = lines.dedented(cursor - content.start);
            let edit = list_edit(&lines.text, at, key)?;
            let shift = |at: usize| content.start + lines.original(at);
            let range = shift(edit.range.start)..shift(edit.range.end);
            let text = edit.text.replace('\n', &format!("\n{}", " ".repeat(indent)));
            let into = edit.cursor - edit.range.start;
            let cursor = if into <= edit.text.len() {
                // In the new text, which its newlines' indentation lengthened.
                range.start + into + edit.text[..into].matches('\n').count() * indent
            } else {
                // After it, where the text was as it is now.
                let unchanged = edit.cursor - edit.text.len() + edit.range.len();
                shift(unchanged) + text.len() - range.len()
            };
            Some(Edit { cursor, range, text })
        }
    }
}

/// Text with up to `indent` spaces taken from the start of each line, and the way back to the
/// original's offsets.
pub(crate) struct Dedented {
    pub(crate) text: String,
    /// For each line, where it starts in the dedented text and in the original, and how many
    /// spaces it lost.
    lines: Vec<(usize, usize, usize)>,
}

impl Dedented {
    pub(crate) fn new(original: &str, indent: usize) -> Self {
        let mut text = String::with_capacity(original.len());
        let mut lines = Vec::new();
        let mut start = 0;
        for line in original.split('\n') {
            if start > 0 {
                text.push('\n');
            }
            let removed = (line.len() - line.trim_start_matches(' ').len()).min(indent);
            lines.push((text.len(), start, removed));
            text.push_str(&line[removed..]);
            start += line.len() + 1;
        }
        Self { text, lines }
    }

    fn line(&self, at: usize, original: bool) -> (usize, usize, usize) {
        let index = self.lines.partition_point(|&(dedented, from, removed)| {
            if original { from + removed <= at } else { dedented <= at }
        });
        self.lines[index.saturating_sub(1)]
    }

    /// The dedented offset of the original's offset `at`, which lies past its line's
    /// indentation.
    fn dedented(&self, at: usize) -> usize {
        let (dedented, from, removed) = self.line(at, true);
        dedented + at.saturating_sub(from + removed)
    }

    /// The original's offset of the dedented offset `at`.
    pub(crate) fn original(&self, at: usize) -> usize {
        let (dedented, from, removed) = self.line(at, false);
        from + removed + (at - dedented)
    }
}

/// The edit that `key` makes outside code. In a code block or after its closing fence, `fence` is
/// the byte offset of its opening fence, and the item is the one that holds the fence.
fn edit(body: &str, cursor: usize, key: ListKey, fence: Option<usize>) -> Option<Edit> {
    let lines = lines(body);
    let line_at = |at: usize| lines.iter().position(|line| line.start <= at && at <= line.end);
    let current = line_at(cursor)?;
    let (from, at) = match fence {
        Some(fence) => (line_at(fence)?, fence),
        None => (current, cursor),
    };
    let found = owner(&lines, from);
    // A line of text or a code block can belong to its item past blank lines, by its indentation.
    // So can a blank line, for the continuation line that Shift+Enter starts on it, and for Tab
    // and Shift+Tab to move its item, and one that holds indentation, for Enter to start the next
    // item after it. Enter on an empty line stays a line break.
    let past_blank_lines =
        !lines[from].is_blank() || key != ListKey::Enter || !lines[from].text.is_empty();
    let (owner, item) =
        if past_blank_lines { found.or_else(|| enclosing(&lines, from, at))? } else { found? };
    // A fence left of the item's content ends the list rather than continuing the item.
    if fence.is_some_and(|fence| fence - lines[from].start < item.content_column) {
        return None;
    }
    match key {
        ListKey::Enter => Some(enter(body, &lines, current, owner, &item, cursor)),
        ListKey::ShiftEnter => {
            let text = format!("\n{}", " ".repeat(item.content_column));
            Some(Edit { cursor: cursor + text.len(), range: cursor..cursor, text })
        }
        ListKey::Tab => Some(indent(body, &lines, owner, &item, cursor)),
        ListKey::ShiftTab => Some(outdent(body, &lines, owner, &item, cursor)),
    }
}

#[derive(Clone, Copy, Debug)]
struct Line<'a> {
    /// Byte offset of the line's first character.
    start: usize,
    /// Byte offset just past the line's last character, before any line break.
    end: usize,
    text: &'a str,
    code: CodeLine,
}

/// Where a line lies relative to code blocks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CodeLine {
    Outside,
    /// Starts a code block, after any list item's marker.
    Opens,
    /// Lies in a code block past its first line, and is code, never a list item.
    Inside,
}

impl Line<'_> {
    fn is_blank(&self) -> bool {
        self.text.trim().is_empty()
    }

    fn indent(&self) -> usize {
        self.text.len() - self.text.trim_start_matches(' ').len()
    }

    /// The list item that the line starts, unless it's code.
    fn item(&self) -> Option<Item> {
        if self.code == CodeLine::Inside { None } else { Item::parse(self.text) }
    }
}

/// The parts of a line that starts a list item.
#[derive(Clone, Debug)]
pub(crate) struct Item {
    pub(crate) checkbox: bool,
    /// Column just past the marker and its space, where nested items and continuation lines start.
    /// A task box is part of the item's text, so a fence or quote indented past the box would read
    /// as text.
    content_column: usize,
    /// Byte offset within the line where the item's text starts, past any checkbox.
    pub(crate) content_start: usize,
    indent: usize,
    marker: Marker,
}

#[derive(Clone, Debug)]
enum Marker {
    Bullet(char),
    Ordered { delimiter: char, number: u64 },
}

impl Item {
    pub(crate) fn parse(line: &str) -> Option<Self> {
        let indent = line.len() - line.trim_start_matches(' ').len();
        let rest = &line[indent..];
        let first = rest.chars().next()?;
        let (marker, marker_length) = if matches!(first, '-' | '*' | '+') {
            (Marker::Bullet(first), 1)
        } else {
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            let delimiter = rest[digits..].chars().next().filter(|c| matches!(c, '.' | ')'))?;
            if !(1..=9).contains(&digits) {
                return None;
            }
            let number = rest[..digits].parse().ok()?;
            (Marker::Ordered { delimiter, number }, digits + 1)
        };
        let after = &rest[marker_length..];
        let spaces = after.len() - after.trim_start_matches(' ').len();
        if spaces == 0 && !after.is_empty() {
            return None;
        }
        let content_column = indent + marker_length + spaces.clamp(1, 4);
        let mut content_start = indent + marker_length + spaces;
        let checkbox = ["[ ]", "[x]", "[X]"].iter().any(|box_| {
            let text = &line[content_start..];
            text.starts_with(box_) && (text.len() == 3 || text[3..].starts_with(' '))
        });
        if checkbox {
            content_start = (content_start + 4).min(line.len());
        }
        Some(Self { checkbox, content_column, content_start, indent, marker })
    }

    /// The start of a new item like this one, at `indent`, numbered after this one.
    fn next_prefix(&self, indent: usize) -> String {
        let marker = match self.marker {
            Marker::Bullet(bullet) => bullet.to_string(),
            Marker::Ordered { delimiter, number } => format!("{}{delimiter}", number + 1),
        };
        let checkbox = if self.checkbox { "[ ] " } else { "" };
        format!("{}{marker} {checkbox}", " ".repeat(indent))
    }

    /// The start of a new item like this one, with the same number, for inserting before it.
    fn same_prefix(&self) -> String {
        let marker = match self.marker {
            Marker::Bullet(bullet) => bullet.to_string(),
            Marker::Ordered { delimiter, number } => format!("{number}{delimiter}"),
        };
        let checkbox = if self.checkbox { "[ ] " } else { "" };
        format!("{}{marker} {checkbox}", " ".repeat(self.indent))
    }
}

fn lines(body: &str) -> Vec<Line<'_>> {
    let mut lines = Vec::new();
    let mut start = 0;
    for text in body.split('\n') {
        lines.push(Line { start, end: start + text.len(), text, code: CodeLine::Outside });
        start += text.len() + 1;
    }
    for (event, range) in Parser::new_ext(body, Options::empty()).into_offset_iter() {
        let Event::Start(Tag::CodeBlock(_)) = event else { continue };
        let block = body[range.clone()].trim_end_matches(['\n', '\r']);
        let end = range.start + block.len();
        for line in lines.iter_mut().filter(|line| line.end >= range.start && line.start <= end) {
            line.code = if line.start <= range.start { CodeLine::Opens } else { CodeLine::Inside };
        }
    }
    lines
}

/// Where the cursor lies relative to code blocks.
enum Code {
    Outside,
    /// In a code block, whose opening fence or first line starts at byte `fence`, `indent` columns
    /// into its line.
    Inside {
        fence: usize,
        indent: usize,
    },
    /// At the end of the closing fence of a code block whose opening fence starts at byte `fence`.
    Closed {
        fence: usize,
    },
    /// In the content of a `table` block, which spans the byte range `content`, between its
    /// fences, whose lines are indented by `indent` spaces, to the column of its opening fence.
    Table {
        content: Range<usize>,
        indent: usize,
    },
}

fn code_block(body: &str, cursor: usize) -> Code {
    for (event, range) in Parser::new_ext(body, Options::empty()).into_offset_iter() {
        let Event::Start(Tag::CodeBlock(kind)) = event else { continue };
        // The end of the closing fence's line is in the block too.
        let block = body[range.clone()].trim_end_matches(['\n', '\r']);
        if !(range.start <= cursor && cursor <= range.start + block.len()) {
            continue;
        }
        let line_start = body[..range.start].rfind('\n').map_or(0, |i| i + 1);
        // A fence on a list item's line, after its marker, sits at the item's content column,
        // where its later lines are indented to.
        let indent = body[line_start..range.start].chars().count();
        let inside = Code::Inside { fence: range.start, indent };
        let CodeBlockKind::Fenced(info) = kind else { return inside };
        let Some(opening) = block.find('\n') else { return inside };
        let start = range.start + opening + 1;
        // The closing fence, when the block has one, isn't content.
        let last_line = block.rfind('\n').map_or(0, |i| i + 1);
        let closing = block[last_line..].trim_start();
        let closed =
            last_line > opening && (closing.starts_with("```") || closing.starts_with("~~~"));
        if closed && cursor == range.start + block.len() {
            return Code::Closed { fence: range.start };
        }
        if info.split_whitespace().next() != Some("table") {
            return inside;
        }
        let end = range.start + if closed { last_line - 1 } else { block.len() };
        if start <= cursor && cursor <= end && start <= end {
            return Code::Table { content: start..end, indent };
        }
        return inside;
    }
    Code::Outside
}

/// The edit that Enter and Shift+Enter make in a code block whose fence starts `indent` columns
/// into its line: a new line indented like the cursor's, and at least as far as the fence, so a
/// block in a list item stays in it. Before the text of a line, or with no indentation to keep,
/// they make a plain line break.
fn code_newline(body: &str, cursor: usize, indent: usize) -> Option<Edit> {
    let line_start = body[..cursor].rfind('\n').map_or(0, |i| i + 1);
    let line_end = body[cursor..].find('\n').map_or(body.len(), |i| cursor + i);
    let line = &body[line_start..line_end];
    let spaces = line.len() - line.trim_start_matches(' ').len();
    if spaces.max(indent) == 0 || cursor < line_start + spaces && !line.trim().is_empty() {
        return None;
    }
    let text = format!("\n{}", " ".repeat(spaces.max(indent)));
    Some(Edit { cursor: cursor + text.len(), range: cursor..cursor, text })
}

/// Finds the item that the line at `current` belongs to: the item it starts, or the item it
/// continues. A code block between them belongs to the item only when it's indented to the item's
/// content column. Otherwise it ends the list, and the line after it is no item's.
fn owner(lines: &[Line], current: usize) -> Option<(usize, Item)> {
    if let Some(item) = lines[current].item() {
        return Some((current, item));
    }
    if lines[current].is_blank() {
        return None;
    }
    // How far every code line between the item and `current` is indented.
    let mut reach = usize::MAX;
    for index in (0..current).rev() {
        let line = lines[index];
        if line.is_blank() && line.code == CodeLine::Outside {
            return None;
        }
        if let Some(item) = line.item() {
            return (item.content_column <= reach).then_some((index, item));
        }
        if line.code != CodeLine::Outside && !line.is_blank() {
            reach = reach.min(line.indent());
        }
    }
    None
}

/// Finds the item that the line at `current` belongs to past blank lines, as `CommonMark` does by
/// indentation: the nearest item above whose content column every line down to `current` reaches,
/// counting a blank `current` as indented up to the cursor.
fn enclosing(lines: &[Line], current: usize, cursor: usize) -> Option<(usize, Item)> {
    let line = lines[current];
    let mut reach = if line.is_blank() { cursor - line.start } else { line.indent() };
    for index in (0..current).rev() {
        let line = lines[index];
        if let Some(item) = line.item()
            && item.content_column <= reach
        {
            return Some((index, item));
        }
        if !line.is_blank() {
            reach = reach.min(line.indent());
        }
    }
    None
}

/// The index of the last line of the item at `owner`, before its children.
fn last_continuation(lines: &[Line], owner: usize) -> usize {
    let mut last = owner;
    while let Some(line) = lines.get(last + 1) {
        if line.is_blank() || line.item().is_some() {
            break;
        }
        last += 1;
    }
    last
}

/// The index of the last line of the item at `owner`, including its children.
fn subtree_end(lines: &[Line], owner: usize, item: &Item) -> usize {
    let mut last = last_continuation(lines, owner);
    let mut index = last + 1;
    while let Some(line) = lines.get(index) {
        if line.is_blank() {
            index += 1;
            continue;
        }
        if line.indent() <= item.indent {
            break;
        }
        last = index;
        index += 1;
    }
    last
}

/// The indent of the nearest item above `owner` with less indent than `item`.
fn parent_indent(lines: &[Line], owner: usize, item: &Item) -> usize {
    lines[..owner]
        .iter()
        .rev()
        .filter_map(Line::item)
        .find(|parent| parent.indent < item.indent)
        .map_or(0, |parent| parent.indent)
}

fn enter(
    body: &str,
    lines: &[Line],
    current: usize,
    owner: usize,
    item: &Item,
    cursor: usize,
) -> Edit {
    let line = lines[current];
    let list_top = current == 0 || self::owner(lines, current - 1).is_none();
    if current == owner && list_top && cursor <= line.start + item.indent {
        return Edit { cursor: cursor + 1, range: line.start..line.start, text: "\n".into() };
    }
    if current == owner && line.text[item.content_start..].trim().is_empty() {
        if item.indent > 0 {
            let indent = parent_indent(lines, owner, item);
            let range = line.start..line.start + item.indent;
            let cursor = line.end - item.indent + indent;
            return Edit { cursor, range, text: " ".repeat(indent) };
        }
        let after_text = current > 0 && !lines[current - 1].is_blank();
        let text = if after_text { "\n" } else { "" }.to_owned();
        return Edit { cursor: line.start + text.len(), range: line.start..line.end, text };
    }
    if current == owner && cursor <= line.start + item.content_start {
        let text = format!("{}\n", item.same_prefix());
        return Edit { cursor: cursor + text.len(), range: line.start..line.start, text };
    }
    if body[cursor..line.end].trim().is_empty() {
        // From the cursor's line, which a code block's blank lines can separate from the item's.
        let last = last_continuation(lines, current);
        let at = lines[last].end;
        let child = lines.get(last + 1).and_then(Line::item);
        let prefix = match child.filter(|child| child.indent > item.indent) {
            Some(child) => child.same_prefix(),
            None => item.next_prefix(item.indent),
        };
        let text = format!("\n{prefix}");
        return Edit { cursor: at + text.len(), range: at..at, text };
    }
    let spaces = body[cursor..line.end].len() - body[cursor..line.end].trim_start().len();
    let text = format!("\n{}", item.next_prefix(item.indent));
    Edit { cursor: cursor + text.len(), range: cursor..cursor + spaces, text }
}

fn indent(body: &str, lines: &[Line], owner: usize, item: &Item, cursor: usize) -> Edit {
    let sibling = lines[..owner]
        .iter()
        .rev()
        .filter_map(Line::item)
        .find(|other| other.indent <= item.indent)
        .filter(|other| other.indent == item.indent);
    let Some(sibling) = sibling else {
        return Edit { cursor, range: cursor..cursor, text: String::new() };
    };
    let shift = sibling.content_column - item.indent;
    // A blank line keeps any indentation it has in step with the item's, and an empty one stays
    // empty.
    reindent(body, lines, owner, item, cursor, |line| {
        if line.text.is_empty() { (0, String::new()) } else { (0, " ".repeat(shift)) }
    })
}

fn outdent(body: &str, lines: &[Line], owner: usize, item: &Item, cursor: usize) -> Edit {
    if item.indent == 0 {
        return Edit { cursor, range: cursor..cursor, text: String::new() };
    }
    let shift = item.indent - parent_indent(lines, owner, item);
    reindent(body, lines, owner, item, cursor, |line| (line.indent().min(shift), String::new()))
}

/// Rewrites the item at `owner` and its children, replacing the first `n` bytes of each line with
/// `text`, where `change` returns `(n, text)` for a line.
fn reindent(
    body: &str,
    lines: &[Line],
    owner: usize,
    item: &Item,
    cursor: usize,
    change: impl Fn(&Line) -> (usize, String),
) -> Edit {
    let last = subtree_end(lines, owner, item);
    let range = lines[owner].start..lines[last].end;
    let mut text = String::new();
    let mut new_cursor = cursor;
    for (index, line) in lines[owner..=last].iter().enumerate() {
        if index > 0 {
            text.push('\n');
        }
        let (removed, inserted) = change(line);
        if line.start <= cursor {
            let removed_before = removed.min(cursor.saturating_sub(line.start));
            new_cursor = new_cursor + inserted.len() - removed_before;
        }
        text.push_str(&inserted);
        text.push_str(&body[line.start + removed..line.end]);
    }
    Edit { cursor: new_cursor, range, text }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Applies `key` at the `|` in `before`, and returns the result with `|` at the new cursor.
    fn press(before: &str, key: ListKey) -> Option<String> {
        let cursor = before.find('|').unwrap();
        let body = before.replacen('|', "", 1);
        let edit = list_edit(&body, cursor, key)?;
        let mut after = body;
        after.replace_range(edit.range, &edit.text);
        after.insert(edit.cursor, '|');
        Some(after)
    }

    #[test]
    fn enter_continues_lists() {
        for (before, after) in [
            ("- one|", "- one\n- |"),
            ("- one|\n- two", "- one\n- |\n- two"),
            ("  * one|", "  * one\n  * |"),
            ("1. one|", "1. one\n2. |"),
            ("9) one|", "9) one\n10) |"),
            ("- [x] done|", "- [x] done\n- [ ] |"),
            ("- split| here", "- split\n- |here"),
            ("- |text", "- \n- |text"),
            ("|- text", "\n|- text"),
            ("text\n|- a", "text\n\n|- a"),
            ("- a\n\n|- b", "- a\n\n\n|- b"),
            ("- a\n|- b", "- a\n- \n|- b"),
            ("- a\n|  - b", "- a\n  - \n|  - b"),
            ("- a\n | - b", "- a\n  - \n | - b"),
            ("- a\n  |- b", "- a\n  - \n  |- b"),
            ("|1. a", "\n|1. a"),
            ("|- [ ] a", "\n|- [ ] a"),
            ("|- ", "\n|- "),
            ("- one\n  more|", "- one\n  more\n- |"),
            ("- one|\n  more", "- one\n  more\n- |"),
            ("- parent|\n  - child", "- parent\n  - |\n  - child"),
        ] {
            assert_eq!(press(before, ListKey::Enter).as_deref(), Some(after), "{before:?}");
        }
    }

    #[test]
    fn enter_on_a_blank_item_moves_out_then_ends_the_list() {
        for (before, after) in [
            ("- a\n  - b\n  - |", "- a\n  - b\n- |"),
            ("- a\n  - b\n    - |", "- a\n  - b\n  - |"),
            ("- a\n- |", "- a\n\n|"),
            ("- |", "|"),
            ("- [ ] |", "|"),
            ("1. a\n   - |", "1. a\n- |"),
        ] {
            assert_eq!(press(before, ListKey::Enter).as_deref(), Some(after), "{before:?}");
        }
    }

    #[test]
    fn shift_enter_continues_items() {
        assert_eq!(press("- one|", ListKey::ShiftEnter).as_deref(), Some("- one\n  |"));
        assert_eq!(press("10. one|", ListKey::ShiftEnter).as_deref(), Some("10. one\n    |"));
        assert_eq!(press("- a\n  b|", ListKey::ShiftEnter).as_deref(), Some("- a\n  b\n  |"));
        assert_eq!(press("- [ ] a|", ListKey::ShiftEnter).as_deref(), Some("- [ ] a\n  |"));
        // Past a blank line, and after a code block, indentation still places a line in its item.
        let blank = "- a\n  b\n  |";
        assert_eq!(press(blank, ListKey::ShiftEnter).as_deref(), Some("- a\n  b\n  \n  |"));
        let nested = "- a\n  - b\n\n  c\n  |";
        assert_eq!(
            press(nested, ListKey::ShiftEnter).as_deref(),
            Some("- a\n  - b\n\n  c\n  \n  |")
        );
        let code = "- a\n  ```\n  x\n\n  ```|\n- b";
        assert_eq!(
            press(code, ListKey::ShiftEnter).as_deref(),
            Some("- a\n  ```\n  x\n\n  ```\n  |\n- b")
        );
        assert_eq!(press("- a\n\n|", ListKey::ShiftEnter), None);
    }

    #[test]
    fn tabs_move_items_from_blank_lines_and_code() {
        for (flat, nested) in [
            (
                "- new parent\n- some|thing\n  a continuation\n  \n  more",
                "- new parent\n  - some|thing\n    a continuation\n    \n    more",
            ),
            (
                "- new parent\n- something\n  a continuation\n  |\n  more",
                "- new parent\n  - something\n    a continuation\n    |\n    more",
            ),
        ] {
            assert_eq!(press(flat, ListKey::Tab).as_deref(), Some(nested));
            assert_eq!(press(nested, ListKey::ShiftTab).as_deref(), Some(flat));
        }
        // An empty line stays empty.
        assert_eq!(
            press("- a\n- b|\n\n  c", ListKey::Tab).as_deref(),
            Some("- a\n  - b|\n\n    c")
        );
        let code = "- a\n- b\n  ```\n  x|\n  ```";
        let moved = "- a\n  - b\n    ```\n    x|\n    ```";
        assert_eq!(press(code, ListKey::Tab).as_deref(), Some(moved));
        assert_eq!(press(moved, ListKey::ShiftTab).as_deref(), Some(code));
        assert_eq!(
            press("- a\n- b\n  ```\n  x\n  ```|", ListKey::Tab).as_deref(),
            Some(moved.replace("x|\n    ```", "x\n    ```|").as_str())
        );
        // Code that no item holds takes its tabs.
        assert_eq!(press("```\nx|\n```", ListKey::Tab), None);
    }

    #[test]
    fn keys_find_items_past_blank_lines() {
        let after_blank = "- done\n  \n  a continuation|\n- next";
        assert_eq!(
            press(after_blank, ListKey::Enter).as_deref(),
            Some("- done\n  \n  a continuation\n- |\n- next")
        );
        let nested = "- a\n- b\n\n  c|";
        assert_eq!(press(nested, ListKey::Tab).as_deref(), Some("- a\n  - b\n\n    c|"));
        // Text at the left margin after a blank line ends the list, and so does an empty line.
        assert_eq!(press("- a\n\nb|", ListKey::Enter), None);
        assert_eq!(press("- a\n\n|", ListKey::Enter), None);
        // A blank line indented into an item belongs to it, and Enter starts the next item.
        assert_eq!(press("- a\n\n  |", ListKey::Enter).as_deref(), Some("- a\n\n  \n- |"));
        let quoted = "- parent\n  - child\n    > not\n    \n    |";
        assert_eq!(
            press(quoted, ListKey::Enter).as_deref(),
            Some("- parent\n  - child\n    > not\n    \n    \n  - |")
        );
    }

    #[test]
    fn enter_after_a_code_block_starts_an_item() {
        let code = "- a\n  ```\n  x\n  ```|";
        assert_eq!(press(code, ListKey::Enter).as_deref(), Some("- a\n  ```\n  x\n  ```\n- |"));
        let blank = "- [ ] a\n  ```\n  x\n\n  y\n  ```|\n- [ ] b";
        let after = "- [ ] a\n  ```\n  x\n\n  y\n  ```\n- [ ] |\n- [ ] b";
        assert_eq!(press(blank, ListKey::Enter).as_deref(), Some(after));
        let more = "- a\n  ```\n  x\n  ```|\n  more";
        assert_eq!(
            press(more, ListKey::Enter).as_deref(),
            Some("- a\n  ```\n  x\n  ```\n  more\n- |")
        );
        assert_eq!(press("```\nx\n```|", ListKey::Enter), None);
        assert_eq!(press("- a\n```\nx\n```|", ListKey::Enter), None);
    }

    #[test]
    fn text_after_a_code_block_is_in_no_item_of_its_code() {
        // The list in a list table is code, and the text after the table is in no item.
        let table = "```table\n- - this\n  - that\n```\n";
        for after in ["[[link|blah]]|", "|this", "th|is"] {
            assert_eq!(press(&format!("{table}{after}"), ListKey::Enter), None, "{after}");
        }
        assert_eq!(press(&format!("- a\n{table}text|"), ListKey::Enter), None);
        // Text after a block that's in an item continues the item.
        let nested = "- a\n  ```\n  - x\n\n  - y\n  ```\n  more|";
        let after = "- a\n  ```\n  - x\n\n  - y\n  ```\n  more\n- |";
        assert_eq!(press(nested, ListKey::Enter).as_deref(), Some(after));
    }

    #[test]
    fn newlines_in_code_keep_indentation() {
        for key in [ListKey::Enter, ListKey::ShiftEnter] {
            let code = "- a\n  ```mermaid\n  flowchart LR|\n  ```";
            let after = "- a\n  ```mermaid\n  flowchart LR\n  |\n  ```";
            assert_eq!(press(code, key).as_deref(), Some(after));
            let deeper = "- a\n  ```\n  if x:\n      y|\n  ```";
            assert_eq!(
                press(deeper, key).as_deref(),
                Some("- a\n  ```\n  if x:\n      y\n      |\n  ```")
            );
            let opening = "- a\n  ```|\n  ```";
            assert_eq!(press(opening, key).as_deref(), Some("- a\n  ```\n  |\n  ```"));
            assert_eq!(press("```\nx|\n```", key), None);
            // Before a line's text, the line breaks where the cursor is.
            assert_eq!(press("- a\n  ```\n|  x\n  ```", key), None);
        }
    }

    #[test]
    fn tab_nests_items_with_their_children() {
        for (before, after) in [
            ("- a\n- b|", "- a\n  - b|"),
            ("- a\n- |", "- a\n  - |"),
            ("- a\n- b|\n  more\n  - c\n- d", "- a\n  - b|\n    more\n    - c\n- d"),
            ("1. a\n2. b|", "1. a\n   2. b|"),
            ("- a\n  - x\n- b|", "- a\n  - x\n  - b|"),
            ("- a|", "- a|"),
            ("- a\n  - b|", "- a\n  - b|"),
        ] {
            assert_eq!(press(before, ListKey::Tab).as_deref(), Some(after), "{before:?}");
        }
    }

    #[test]
    fn shift_tab_moves_items_out_with_their_children() {
        for (before, after) in [
            ("- a\n  - b|", "- a\n- b|"),
            ("- a\n  - b|\n    - c", "- a\n- b|\n  - c"),
            ("- a|", "- a|"),
            ("- a\n  - b\n    more|", "- a\n- b\n  more|"),
        ] {
            assert_eq!(press(before, ListKey::ShiftTab).as_deref(), Some(after), "{before:?}");
        }
    }

    #[test]
    fn edits_lists_in_table_blocks() {
        for (before, key, after) in [
            (
                "```table\n- - this\n  - that|\n```",
                ListKey::Enter,
                "```table\n- - this\n  - that\n  - |\n```",
            ),
            (
                "```table\n- - a\n  - b\n  - |\n```",
                ListKey::Enter,
                "```table\n- - a\n  - b\n- |\n```",
            ),
            ("```table\n- - a\n- b|\n```", ListKey::Tab, "```table\n- - a\n  - b|\n```"),
            ("```table\n- row\n  - a|", ListKey::Enter, "```table\n- row\n  - a\n  - |"),
            (
                "x\n\n```table\n- a\n  - b|\n```\n\n- c",
                ListKey::ShiftTab,
                "x\n\n```table\n- a\n- b|\n```\n\n- c",
            ),
        ] {
            assert_eq!(press(before, key).as_deref(), Some(after), "{before:?} {key:?}");
        }
        // A table in a cell edits within its own fences.
        let nested =
            |lines: &str| format!("```table\n- r\n  - 4\n\n    ```table\n{lines}\n    ```\n```");
        for (before, key, after) in [
            (
                "    - a\n      - 1\n      - 2|",
                ListKey::Enter,
                "    - a\n      - 1\n      - 2\n      - |",
            ),
            ("    - a\n      - 1\n    - 2|", ListKey::Tab, "    - a\n      - 1\n      - 2|"),
            ("    - a\n      - 1|\n        - x", ListKey::ShiftTab, "    - a\n    - 1|\n      - x"),
            ("    - a\n      - 1\n      - |", ListKey::Enter, "    - a\n      - 1\n    - |"),
        ] {
            let pressed = press(&nested(before), key);
            assert_eq!(pressed, Some(nested(after)), "{before:?} {key:?}");
        }
        // A table that starts on a cell's line.
        let cell = concat!(
            "```table\n- row1\n  - ```table\n",
            "    - heading row\n      - a\n      - b|\n    ```\n```",
        );
        let expected = cell.replace("- b|", "- b\n      - |");
        assert_eq!(press(cell, ListKey::Enter), Some(expected));
        // The note that showed the problem.
        let note = concat!(
            "```table\n- row1\n  - this\n- row3\n  - 3\n  - 4\n    \n    and five\n    \n",
            "    ```table\n    - code\n      - blah\n    - another\n",
            "      - 1\n      - 2|\n    ```\n```",
        );
        let expected = note.replace("- 2|", "- 2\n      - |");
        assert_eq!(press(note, ListKey::Enter), Some(expected));
        // The fences aren't content.
        for before in ["```table|\n- a\n```", "```table\n- a\n```|"] {
            assert_eq!(press(before, ListKey::Enter), None, "{before:?}");
        }
    }

    #[test]
    fn leaves_other_text_alone() {
        for before in [
            "plain|",
            "- a\n\nafter|",
            "```\n- code|\n```",
            "```math\n- code|\n```",
            "---|",
            "-not|",
        ] {
            for key in [ListKey::Enter, ListKey::ShiftEnter, ListKey::Tab, ListKey::ShiftTab] {
                assert_eq!(press(before, key), None, "{before:?} {key:?}");
            }
        }
    }
}
