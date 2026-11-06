//! The editor that apps show a note's body in, as decision ZIB0017 describes: its text, its
//! selection, and the commands and moves that change them. Each app lays out and draws the text
//! itself, and answers what depends on how lines wrap, such as the place one line down.
//!
//! The editor edits the note's Loro text directly, and keeps the selection as Loro cursors, so it
//! stays in place as edits from other machines arrive. Offsets are bytes of UTF-8 text.

pub mod display;

use std::ops::Range;
use std::sync::{Arc, Mutex};

use loro::cursor::{AbsolutePosition, Cursor, Side};
use loro::undo::CursorWithPos;
use loro::{LoroDoc, LoroText, UndoItemMeta, UndoManager};
use unicode_segmentation::{GraphemeCursor, UnicodeSegmentation};

use crate::space::ORIGIN;
use crate::time::{self, Timestamp};

/// How long a pause in typing ends its undo step, in milliseconds.
const TYPING_PAUSE: i64 = 1000;

/// The selection: where it started, and where the cursor is now. They're equal when nothing is
/// selected.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
}

impl Selection {
    pub fn cursor(at: usize) -> Self {
        Self { anchor: at, head: at }
    }

    /// The selected bytes, in order.
    pub fn range(self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }

    pub fn is_empty(self) -> bool {
        self.anchor == self.head
    }
}

/// A note's body in its space's document, for an editor to open. It shares the document with the
/// space, so the editor's edits are the space's.
pub struct Body {
    pub(crate) doc: LoroDoc,
    pub(crate) text: LoroText,
}

impl Body {
    /// A body of its own, holding `text`, in no space: for tests, and for editing text that isn't a
    /// note's.
    pub fn detached(text: &str) -> Self {
        let doc = LoroDoc::new();
        let body = doc.get_text("body");
        // A new document's text takes any insertion.
        let _ = body.insert(0, text);
        crate::space::commit(&doc);
        Self { doc, text: body }
    }
}

/// Which way a move goes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    Back,
    Forward,
}

/// What a move steps over.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Unit {
    /// A character as the owner sees it, which can be several code points.
    Grapheme,
    /// To the start of a word going back, and to its end going forward.
    Word,
    /// To the start or end of the line, as the text breaks it, not as it wraps.
    Line,
    /// To the start or end of the text.
    Text,
}

/// A note's body open for editing.
pub struct Editor {
    doc: LoroDoc,
    body: LoroText,
    undo: UndoManager,
    /// The body as the editor last read it.
    text: String,
    selection: Selection,
    /// The selection as Loro cursors, which follow the text as others edit it.
    cursors: Option<(Cursor, Cursor)>,
    /// The left edge that moves up and down keep, in the app's units, while they go on.
    goal: Option<f64>,
    /// The selection before the edit in progress, for an undo step to remember, as cursors and
    /// their places in characters.
    before: Arc<Mutex<Vec<CursorWithPos>>>,
    /// The selection that the last undo or redo brought back, in characters.
    restored: Arc<Mutex<Option<Vec<usize>>>>,
    /// When the owner last typed, while their typing makes one undo step.
    typing: Option<Timestamp>,
}

impl Editor {
    /// Opens a note's body, with the cursor at the start.
    pub fn new(Body { doc, text: body }: Body) -> Self {
        let mut undo = UndoManager::new(&doc);
        undo.add_exclude_origin_prefix(ORIGIN);
        let before: Arc<Mutex<Vec<CursorWithPos>>> = Arc::default();
        let restored: Arc<Mutex<Option<Vec<usize>>>> = Arc::default();
        {
            let before = before.clone();
            undo.set_on_push(Some(Box::new(move |_, _, _| {
                let cursors = before.lock().expect("unpoisoned").clone();
                UndoItemMeta { value: loro::LoroValue::Null, cursors }
            })));
        }
        {
            let restored = restored.clone();
            undo.set_on_pop(Some(Box::new(move |_, _, meta| {
                // Undo moves each position past the edits made since, from other machines too.
                let cursors = meta.cursors.into_iter().map(|c| c.pos.pos).collect();
                *restored.lock().expect("unpoisoned") = Some(cursors);
            })));
        }
        let text = body.to_string();
        let mut editor = Self {
            doc,
            body,
            undo,
            text,
            selection: Selection::default(),
            cursors: None,
            goal: None,
            before,
            restored,
            typing: None,
        };
        editor.place(Selection::default());
        editor
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn selection(&self) -> Selection {
        self.selection
    }

    /// How the text looks with the selection as it is.
    pub fn display(&self) -> display::Display {
        display::display(&self.text, &self.selection.range())
    }

    pub fn selected_text(&self) -> &str {
        &self.text[self.selection.range()]
    }

    /// The left edge that moves up and down keep, which the app set with `move_vertically`.
    pub fn goal(&self) -> Option<f64> {
        self.goal
    }

    /// Reads the body again after edits from elsewhere, and keeps the selection with the text
    /// around it. Returns whether the text changed.
    pub fn refresh(&mut self) -> bool {
        let text = self.body.to_string();
        if text == self.text {
            return false;
        }
        self.text = text;
        let found =
            self.cursors.as_ref().map(|(anchor, head)| (self.find(anchor), self.find(head)));
        let selection = if let Some((Some(anchor), Some(head))) = found {
            Selection { anchor, head }
        } else {
            let at = |at: usize| self.boundary(at.min(self.text.len()));
            Selection { anchor: at(self.selection.anchor), head: at(self.selection.head) }
        };
        self.place(selection);
        true
    }

    /// Selects from `anchor` to `head`, which snap to the characters they fall in.
    pub fn select(&mut self, anchor: usize, head: usize) {
        self.goal = None;
        self.checkpoint();
        let (anchor, head) = (self.boundary(anchor), self.boundary(head));
        self.place(Selection { anchor, head });
    }

    /// Moves the cursor to `to`, or with `extend`, the selection's head.
    pub fn move_to(&mut self, to: usize, extend: bool) {
        let anchor = if extend { self.selection.anchor } else { to };
        self.select(anchor, to);
    }

    /// Moves to `to`, a place the app found one line up or down, keeping `goal`, the left edge the
    /// moves started from, for the next one.
    pub fn move_vertically(&mut self, to: usize, goal: f64, extend: bool) {
        self.move_to(to, extend);
        self.goal = Some(goal);
    }

    /// Moves by one `unit` in `direction`. Without `extend`, a move from a selection goes to the
    /// selection's edge on that side, as a move by a character does.
    pub fn move_by(&mut self, unit: Unit, direction: Direction, extend: bool) {
        let Selection { head, .. } = self.selection;
        if !extend && !self.selection.is_empty() && unit == Unit::Grapheme {
            let range = self.selection.range();
            let to = if direction == Direction::Back { range.start } else { range.end };
            self.move_to(to, false);
            return;
        }
        let to = self.step_shown(head, unit, direction);
        self.move_to(to, extend);
    }

    pub fn select_all(&mut self) {
        self.select(0, self.text.len());
    }

    /// Selects the word at `at`, or the space or symbols there if there's no word.
    pub fn select_word_at(&mut self, at: usize) {
        let word = self.word_at(at);
        self.select(word.start, word.end);
    }

    /// Selects the line at `at`, as the text breaks it, with its line break.
    pub fn select_line_at(&mut self, at: usize) {
        let line = self.line_at(at);
        self.select(line.start, line.end);
    }

    /// Selects from `origin`, the word or line a double or triple click selected, to the whole
    /// word or line at `to`, as a drag after the click goes on: the selection grows and shrinks by
    /// whole words or lines, and always holds `origin`. `unit` is `Word` or `Line`.
    pub fn select_by_unit(&mut self, origin: Range<usize>, to: usize, unit: Unit) {
        let here = if unit == Unit::Line { self.line_at(to) } else { self.word_at(to) };
        if here.start < origin.start {
            self.select(origin.end, here.start);
        } else {
            self.select(origin.start, here.end.max(origin.end));
        }
    }

    /// Moves the selection's head past syntax that hides while `showing` is selected, when the head
    /// lies inside it or at its edge nearer the anchor. A selection dragged with the mouse to an
    /// element whose syntax hides then takes in that syntax too, as the text shows it, and as a
    /// move by the keyboard does. `showing` is the selection that decides what shows on screen,
    /// which stays as it was while a mouse button is down.
    pub fn take_in_syntax(&mut self, showing: &Range<usize>) {
        let Selection { anchor, head } = self.selection;
        if anchor == head {
            return;
        }
        let forward = head > anchor;
        let mut to = head;
        for hidden in display::display(&self.text, showing).hidden {
            let at_edge = if forward {
                hidden.start <= to && to < hidden.end
            } else {
                hidden.start < to && to <= hidden.end
            };
            if at_edge {
                to = if forward { hidden.end } else { hidden.start };
            }
        }
        if to != head {
            self.place(Selection { anchor, head: to });
        }
    }

    /// The word at `at`, or the space or symbols there if there's no word. At the end of the
    /// text, it's the last word or space.
    pub fn word_at(&self, at: usize) -> Range<usize> {
        let at = at.min(self.text.len());
        self.text
            .split_word_bound_indices()
            .map(|(start, word)| start..start + word.len())
            .find(|range| range.start <= at && at < range.end)
            .or_else(|| {
                let last = self.text.split_word_bound_indices().next_back();
                last.map(|(start, word)| start..start + word.len()).filter(|r| r.end == at)
            })
            .unwrap_or(at..at)
    }

    /// The line at `at`, as the text breaks it, with its line break.
    pub fn line_at(&self, at: usize) -> Range<usize> {
        let at = at.min(self.text.len());
        let start = self.text[..at].rfind('\n').map_or(0, |i| i + 1);
        let end = self.text[at..].find('\n').map_or(self.text.len(), |i| at + i + 1);
        start..end
    }

    /// Types `typed` in place of the selection.
    pub fn insert(&mut self, typed: &str) {
        let range = self.selection.range();
        // A line break, or text pasted in one go, is an undo step of its own.
        let separate = typed.contains('\n') || typed.chars().nth(1).is_some();
        self.replace(range, typed, separate);
    }

    /// Deletes the selection, or the character before the cursor.
    pub fn backspace(&mut self) {
        self.delete(Unit::Grapheme, Direction::Back);
    }

    /// Deletes the selection, or from the cursor by one `unit` in `direction`.
    pub fn delete(&mut self, unit: Unit, direction: Direction) {
        let mut range = self.selection.range();
        if range.is_empty() {
            let to = self.step(range.start, unit, direction);
            range = range.start.min(to)..range.start.max(to);
        }
        if !range.is_empty() {
            self.replace(range, "", unit != Unit::Grapheme);
        }
    }

    /// Takes the selected text out, and returns it for the clipboard.
    pub fn cut(&mut self) -> String {
        let cut = self.selected_text().to_owned();
        if !cut.is_empty() {
            self.replace(self.selection.range(), "", true);
        }
        cut
    }

    /// Undoes the last step of the owner's edits, and selects what was selected before it. Returns
    /// whether there was one.
    pub fn undo(&mut self) -> bool {
        self.goal = None;
        self.checkpoint();
        let undone = self.undo.undo().unwrap_or(false);
        self.after_history(undone)
    }

    /// Redoes the last step undone. Returns whether there was one.
    pub fn redo(&mut self) -> bool {
        self.goal = None;
        self.checkpoint();
        let redone = self.undo.redo().unwrap_or(false);
        self.after_history(redone)
    }

    pub fn can_undo(&self) -> bool {
        self.undo.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.undo.can_redo()
    }

    fn after_history(&mut self, changed: bool) -> bool {
        if !changed {
            return false;
        }
        self.refresh();
        let restored = self.restored.lock().expect("unpoisoned").take();
        if let Some(&[anchor, head]) = restored.as_deref() {
            let (anchor, head) = (self.byte(anchor), self.byte(head));
            self.place(Selection { anchor, head });
        }
        true
    }

    /// Replaces `range` with `with`, commits it, and puts the cursor after it. A `separate` edit is
    /// an undo step of its own, and doesn't merge with typing around it.
    fn replace(&mut self, range: Range<usize>, with: &str, separate: bool) {
        self.goal = None;
        if separate {
            self.checkpoint();
        } else {
            self.typed();
        }
        let before: Vec<CursorWithPos> = [self.selection.anchor, self.selection.head]
            .into_iter()
            .filter_map(|at| {
                let pos = self.text[..at].chars().count();
                let cursor = self.body.get_cursor(pos, Side::Middle)?;
                Some(CursorWithPos { cursor, pos: AbsolutePosition { pos, side: Side::Middle } })
            })
            .collect();
        *self.before.lock().expect("unpoisoned") = before;
        // A failed edit leaves the text as it was, which `refresh` then shows.
        if !range.is_empty() {
            let _ = self.body.delete_utf8(range.start, range.len());
        }
        if !with.is_empty() {
            let _ = self.body.insert_utf8(range.start, with);
        }
        self.doc.commit();
        if separate {
            self.checkpoint();
        }
        self.text = self.body.to_string();
        let at = self.boundary((range.start + with.len()).min(self.text.len()));
        self.place(Selection::cursor(at));
    }

    /// Ends the undo step in progress, so the next edit starts a new one.
    fn checkpoint(&mut self) {
        if self.typing.take().is_some() {
            self.undo.group_end();
        }
    }

    /// Adds typing to the undo step of the typing before it, unless it paused for a while.
    fn typed(&mut self) {
        let now = time::now();
        if self.typing.is_some_and(|last| now - last >= TYPING_PAUSE) {
            self.checkpoint();
        }
        if self.typing.is_none() {
            let _ = self.undo.group_start();
        }
        self.typing = Some(now);
    }

    /// Sets the selection, and the cursors that keep it in place.
    fn place(&mut self, selection: Selection) {
        self.selection = selection;
        let cursor = |at: usize| {
            let chars = self.text[..at].chars().count();
            self.body.get_cursor(chars, Side::Middle)
        };
        self.cursors = cursor(selection.anchor).zip(cursor(selection.head));
    }

    /// Where a cursor is now, in bytes.
    fn find(&self, cursor: &Cursor) -> Option<usize> {
        let chars = self.doc.get_cursor_pos(cursor).ok()?.current.pos;
        Some(self.byte(chars))
    }

    /// The byte offset of the character offset `chars`.
    fn byte(&self, chars: usize) -> usize {
        self.text.char_indices().nth(chars).map_or(self.text.len(), |(at, _)| at)
    }

    /// The place one `unit` from `at` in `direction`, as the text shows before the move. A move by
    /// a character or a word never stops inside syntax that hides, and one that lands at the edge
    /// of such syntax goes past it, so the cursor lands inside the element it belongs to from
    /// either side. Syntax that follows syntax, as `*` follows `**` in `***both***`, counts as one.
    fn step_shown(&self, at: usize, unit: Unit, direction: Direction) -> usize {
        let mut to = self.step(at, unit, direction);
        if !matches!(unit, Unit::Grapheme | Unit::Word) {
            return to;
        }
        let forward = direction == Direction::Forward;
        for hidden in &self.display().hidden {
            let inside = hidden.start < to && to < hidden.end;
            let onto = if forward { to == hidden.start } else { to == hidden.end };
            if inside || (onto && to != at) {
                to = if forward { hidden.end } else { hidden.start };
            }
        }
        to
    }

    /// The place one `unit` from `at` in `direction`.
    fn step(&self, at: usize, unit: Unit, direction: Direction) -> usize {
        let text = &self.text;
        match (unit, direction) {
            (Unit::Grapheme, Direction::Back) => {
                { GraphemeCursor::new(at, text.len(), true).prev_boundary(text, 0).ok().flatten() }
                    .unwrap_or(0)
            }
            (Unit::Grapheme, Direction::Forward) => {
                { GraphemeCursor::new(at, text.len(), true).next_boundary(text, 0).ok().flatten() }
                    .unwrap_or(text.len())
            }
            (Unit::Word, Direction::Back) => {
                words(text).filter(|word| word.start < at).last().map_or(0, |word| word.start)
            }
            (Unit::Word, Direction::Forward) => {
                words(text).find(|word| word.end > at).map_or(text.len(), |word| word.end)
            }
            (Unit::Line, Direction::Back) => text[..at].rfind('\n').map_or(0, |i| i + 1),
            (Unit::Line, Direction::Forward) => {
                text[at..].find('\n').map_or(text.len(), |i| at + i)
            }
            (Unit::Text, Direction::Back) => 0,
            (Unit::Text, Direction::Forward) => text.len(),
        }
    }

    /// The start of the character that `at` falls in.
    fn boundary(&self, at: usize) -> usize {
        let at = at.min(self.text.len());
        let mut cursor = GraphemeCursor::new(at, self.text.len(), true);
        match cursor.is_boundary(&self.text, 0) {
            Ok(true) => at,
            _ => cursor.prev_boundary(&self.text, 0).ok().flatten().unwrap_or(0),
        }
    }
}

/// The words of `text`: runs of letters, digits, and the like, without the spaces and symbols
/// between them.
fn words(text: &str) -> impl Iterator<Item = Range<usize>> + '_ {
    text.split_word_bound_indices()
        .filter(|(_, word)| word.chars().any(char::is_alphanumeric))
        .map(|(start, word)| start..start + word.len())
}

#[cfg(test)]
mod tests;
