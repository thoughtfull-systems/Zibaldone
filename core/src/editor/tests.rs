use loro::{CommitOptions, ExportMode, LoroDoc};

use super::*;

/// An editor of `text`, where `|` marks the cursor, or two of them the selection from the first to
/// the second.
fn editor(text: &str) -> Editor {
    let doc = LoroDoc::new();
    let body = doc.get_text("body");
    let plain = text.replace('|', "");
    body.insert(0, &plain).unwrap();
    doc.commit_with(CommitOptions::new().origin(ORIGIN));
    let mut editor = Editor::new(Body { doc, text: body });
    let marks: Vec<usize> = text.match_indices('|').map(|(i, _)| i).collect();
    match marks[..] {
        [at] => editor.select(at, at),
        [anchor, head] => editor.select(anchor, head - 1),
        _ => {}
    }
    editor
}

/// The editor's text with `|` at the cursor, or around the selection.
fn shown(editor: &Editor) -> String {
    let mut text = editor.text().to_owned();
    let range = editor.selection().range();
    if range.is_empty() {
        text.insert(range.start, '|');
    } else {
        text.insert(range.end, '|');
        text.insert(range.start, '|');
    }
    text
}

#[test]
fn types_over_the_selection() {
    let mut e = editor("one |two| three");
    e.insert("2");
    assert_eq!(shown(&e), "one 2| three");
    e.insert("\n");
    assert_eq!(shown(&e), "one 2\n| three");
}

#[test]
fn deletes_characters_as_they_show() {
    // An `e` with a combining accent is one character.
    let mut e = editor("ae\u{301}|!");
    e.backspace();
    assert_eq!(shown(&e), "a|!");
    e.delete(Unit::Grapheme, Direction::Forward);
    assert_eq!(shown(&e), "a|");
    e.delete(Unit::Grapheme, Direction::Forward);
    assert_eq!(shown(&e), "a|");
}

#[test]
fn deletes_words() {
    let mut e = editor("one two, |three");
    e.delete(Unit::Word, Direction::Back);
    assert_eq!(shown(&e), "one |three");
    e.delete(Unit::Word, Direction::Forward);
    assert_eq!(shown(&e), "one |");
}

#[test]
fn moves_by_characters_words_lines_and_the_text() {
    let mut e = editor("one |two\nthree four");
    e.move_by(Unit::Word, Direction::Forward, false);
    assert_eq!(shown(&e), "one two|\nthree four");
    e.move_by(Unit::Word, Direction::Forward, false);
    assert_eq!(shown(&e), "one two\nthree| four");
    e.move_by(Unit::Word, Direction::Back, false);
    assert_eq!(shown(&e), "one two\n|three four");
    e.move_by(Unit::Grapheme, Direction::Back, false);
    assert_eq!(shown(&e), "one two|\nthree four");
    e.move_by(Unit::Line, Direction::Back, false);
    assert_eq!(shown(&e), "|one two\nthree four");
    e.move_by(Unit::Text, Direction::Forward, true);
    assert_eq!(shown(&e), "|one two\nthree four|");
}

#[test]
fn a_move_from_a_selection_goes_to_its_edge() {
    let mut e = editor("one |two| three");
    e.move_by(Unit::Grapheme, Direction::Back, false);
    assert_eq!(shown(&e), "one |two three");
    let mut e = editor("one |two| three");
    e.move_by(Unit::Grapheme, Direction::Forward, false);
    assert_eq!(shown(&e), "one two| three");
}

#[test]
fn selects_words_and_lines() {
    let mut e = editor("one two\nthree");
    e.select_word_at(5);
    assert_eq!(shown(&e), "one |two|\nthree");
    e.select_line_at(1);
    assert_eq!(shown(&e), "|one two\n|three");
    e.select_line_at(9);
    assert_eq!(shown(&e), "one two\n|three|");
}

#[test]
fn cuts_the_selection() {
    let mut e = editor("one |two| three");
    assert_eq!(e.cut(), "two");
    assert_eq!(shown(&e), "one | three");
}

#[test]
fn undoes_steps_and_puts_the_selection_back() {
    let mut e = editor("one |two| three");
    e.insert("2");
    e.insert("\n");
    assert!(e.undo());
    assert_eq!(shown(&e), "one 2| three");
    assert!(e.undo());
    assert_eq!(shown(&e), "one |two| three");
    assert!(!e.undo());
    assert!(e.redo());
    assert_eq!(e.text(), "one 2 three");
}

#[test]
fn undo_leaves_other_changes_alone() {
    let mut e = editor("one|");
    e.insert(" two");
    // A change the core makes for itself, such as to a note's metadata.
    e.doc.get_map("meta").insert("updated", 1).unwrap();
    e.doc.commit_with(CommitOptions::new().origin(ORIGIN));
    assert!(e.undo());
    assert_eq!(e.text(), "one");
    assert!(e.doc.get_map("meta").get("updated").is_some());
}

#[test]
fn keeps_the_selection_with_the_text_as_others_edit() {
    let mut e = editor("one |two| three");
    let other = LoroDoc::new();
    other.import(&e.doc.export(ExportMode::Snapshot).unwrap()).unwrap();
    other.get_text("body").insert(0, "zero ").unwrap();
    other.commit();
    e.doc.import(&other.export(ExportMode::Snapshot).unwrap()).unwrap();
    assert!(e.refresh());
    assert_eq!(shown(&e), "zero one |two| three");
    // Undo reverses only this machine's edits.
    e.insert("2");
    assert!(e.undo());
    assert_eq!(e.text(), "zero one two three");
}

#[test]
fn typing_undoes_in_one_step_until_the_cursor_moves() {
    let mut e = editor("one|");
    for c in [" ", "t", "w", "o"] {
        e.insert(c);
    }
    e.move_by(Unit::Grapheme, Direction::Back, false);
    e.insert("!");
    assert!(e.undo());
    assert_eq!(shown(&e), "one tw|o");
    assert!(e.undo());
    assert_eq!(shown(&e), "one|");
}

#[test]
fn drags_after_a_double_click_select_whole_words() {
    let mut e = editor("one two three four");
    let origin = e.word_at(5);
    e.select_word_at(5);
    // Forward into a word takes all of it, and back past the start keeps the first word.
    e.select_by_unit(origin.clone(), 9, Unit::Word);
    assert_eq!(shown(&e), "one |two three| four");
    e.select_by_unit(origin.clone(), 1, Unit::Word);
    assert_eq!(e.selection(), Selection { anchor: 7, head: 0 });
    // Back over the first word shrinks to it.
    e.select_by_unit(origin, 4, Unit::Word);
    assert_eq!(shown(&e), "one |two| three four");
}

#[test]
fn drags_after_a_triple_click_select_whole_lines() {
    let mut e = editor("one\ntwo\nthree");
    let origin = e.line_at(5);
    e.select_by_unit(origin.clone(), 10, Unit::Line);
    assert_eq!(shown(&e), "one\n|two\nthree|");
    e.select_by_unit(origin, 1, Unit::Line);
    assert_eq!(e.selection(), Selection { anchor: 8, head: 0 });
}

#[test]
fn moves_onto_hidden_syntax_land_inside_its_element() {
    let mut e = editor("a| **bold** c");
    e.move_by(Unit::Grapheme, Direction::Forward, false);
    assert_eq!(shown(&e), "a **|bold** c");
    let mut e = editor("a **bold** |c");
    e.move_by(Unit::Grapheme, Direction::Back, false);
    assert_eq!(shown(&e), "a **bold|** c");
}

#[test]
fn moves_past_syntax_that_follows_syntax() {
    let mut e = editor("a| ***both*** c");
    e.move_by(Unit::Grapheme, Direction::Forward, false);
    assert_eq!(shown(&e), "a ***|both*** c");
}

#[test]
fn moves_through_shown_syntax_a_character_at_a_time() {
    let mut e = editor("a **|bold** c");
    e.move_by(Unit::Grapheme, Direction::Back, false);
    assert_eq!(shown(&e), "a *|*bold** c");
}

#[test]
fn word_moves_never_stop_inside_hidden_syntax() {
    let mut e = editor("see [the site](http://example.com/page) now|");
    e.move_by(Unit::Word, Direction::Back, false);
    assert_eq!(shown(&e), "see [the site](http://example.com/page) |now");
    // The address hides, so the move goes past it into the link.
    e.move_by(Unit::Word, Direction::Back, false);
    assert_eq!(shown(&e), "see [the site|](http://example.com/page) now");
}

#[test]
fn mouse_selections_take_in_hidden_syntax_at_their_ends() {
    let text = "this is some ==task==";
    let task = text.find("task").unwrap();
    // To the end of the word, before its hidden `==`.
    let mut e = editor(text);
    e.select(0, task + 4);
    e.take_in_syntax(&(0..0));
    assert_eq!(e.selection(), Selection { anchor: 0, head: text.len() });
    // To the middle of the `==`, as a whole-word drag past the word reaches.
    e.select(0, task + 5);
    e.take_in_syntax(&(0..0));
    assert_eq!(e.selection().head, text.len());
    // Back from the end, past the `==` before the word.
    e.select(text.len(), task);
    e.take_in_syntax(&(0..0));
    assert_eq!(e.selection().head, task - 2);
    // Syntax that shows stays as it is.
    e.select(0, task + 4);
    e.take_in_syntax(&(task..task));
    assert_eq!(e.selection().head, task + 4);
}
