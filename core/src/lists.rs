//! Outliner-style editing of Markdown lists, in the manner of Workflowy.

use std::ops::Range;

use pulldown_cmark::{Event, Options, Parser, Tag};

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
/// Shift+Tab always return an edit, which may change nothing, so they never insert a tab.
pub fn list_edit(body: &str, cursor: usize, key: ListKey) -> Option<Edit> {
    if in_code_block(body, cursor) {
        return None;
    }
    let lines = lines(body);
    let current = lines.iter().position(|line| line.start <= cursor && cursor <= line.end)?;
    let (owner, item) = owner(&lines, current)?;
    match key {
        ListKey::Enter => Some(enter(body, &lines, current, owner, &item, cursor)),
        ListKey::ShiftEnter => {
            let text = format!("\n{}", " ".repeat(item.content_start));
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
}

impl Line<'_> {
    fn is_blank(&self) -> bool {
        self.text.trim().is_empty()
    }

    fn indent(&self) -> usize {
        self.text.len() - self.text.trim_start_matches(' ').len()
    }
}

/// The parts of a line that starts a list item.
#[derive(Clone, Debug)]
pub(crate) struct Item {
    pub(crate) checkbox: bool,
    /// Column just past the marker and its space, where nested items start.
    content_column: usize,
    /// Byte offset within the line where the item's text starts, past any checkbox. Continuation
    /// lines start here too, to line up with the text.
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
        lines.push(Line { start, end: start + text.len(), text });
        start += text.len() + 1;
    }
    lines
}

fn in_code_block(body: &str, cursor: usize) -> bool {
    Parser::new_ext(body, Options::empty()).into_offset_iter().any(|(event, range)| {
        matches!(event, Event::Start(Tag::CodeBlock(_)))
            && range.start <= cursor
            && cursor < range.end
    })
}

/// Finds the item that the line at `current` belongs to: the item it starts, or the item it
/// continues.
fn owner(lines: &[Line], current: usize) -> Option<(usize, Item)> {
    if let Some(item) = Item::parse(lines[current].text) {
        return Some((current, item));
    }
    if lines[current].is_blank() {
        return None;
    }
    for index in (0..current).rev() {
        if lines[index].is_blank() {
            return None;
        }
        if let Some(item) = Item::parse(lines[index].text) {
            return Some((index, item));
        }
    }
    None
}

/// The index of the last line of the item at `owner`, before its children.
fn last_continuation(lines: &[Line], owner: usize) -> usize {
    let mut last = owner;
    while let Some(line) = lines.get(last + 1) {
        if line.is_blank() || Item::parse(line.text).is_some() {
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
        .filter_map(|line| Item::parse(line.text))
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
        let last = last_continuation(lines, owner);
        let at = lines[last].end;
        let child = lines.get(last + 1).and_then(|next| Item::parse(next.text));
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
        .filter_map(|line| Item::parse(line.text))
        .find(|other| other.indent <= item.indent)
        .filter(|other| other.indent == item.indent);
    let Some(sibling) = sibling else {
        return Edit { cursor, range: cursor..cursor, text: String::new() };
    };
    let shift = sibling.content_column - item.indent;
    reindent(body, lines, owner, item, cursor, |line| {
        if line.is_blank() { (0, String::new()) } else { (0, " ".repeat(shift)) }
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
        assert_eq!(press("- [ ] a|", ListKey::ShiftEnter).as_deref(), Some("- [ ] a\n      |"));
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
    fn leaves_other_text_alone() {
        for before in ["plain|", "- a\n\nafter|", "```\n- code|\n```", "---|", "-not|"] {
            for key in [ListKey::Enter, ListKey::ShiftEnter, ListKey::Tab, ListKey::ShiftTab] {
                assert_eq!(press(before, key), None, "{before:?} {key:?}");
            }
        }
    }
}
