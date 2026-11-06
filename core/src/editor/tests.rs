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

#[test]
fn list_keys_edit_list_items() {
    let mut e = editor("- one|");
    assert!(e.list_key(ListKey::Enter));
    assert_eq!(shown(&e), "- one\n- |");
    e.insert("two");
    assert!(e.list_key(ListKey::Tab));
    assert_eq!(shown(&e), "- one\n  - two|");
    assert!(e.undo());
    assert_eq!(shown(&e), "- one\n- two|");
    // Outside lists, and with a selection, the keys do what they do in other text.
    let mut e = editor("text|");
    assert!(!e.list_key(ListKey::Enter));
    let mut e = editor("- |one|");
    assert!(!e.list_key(ListKey::Enter));
}

#[test]
fn toggles_tasks_with_the_selection_in_place() {
    let mut e = editor("- [ ] task|");
    assert!(e.toggle_task(2));
    assert_eq!(shown(&e), "- [x] task|");
    let mut e = editor("- item|");
    assert!(e.toggle_task(0));
    assert_eq!(shown(&e), "- [ ] item|");
}

#[test]
fn moves_back_into_a_task_box_in_two_steps() {
    let mut e = editor("- [ ] te|xt");
    e.move_by(Unit::Grapheme, Direction::Back, false);
    e.move_by(Unit::Grapheme, Direction::Back, false);
    assert_eq!(shown(&e), "- [ ] |text");
    e.move_by(Unit::Grapheme, Direction::Back, false);
    assert_eq!(shown(&e), "- [ ]| text");
}

/// Note bodies: Labels in display editors.
#[test]
fn link_titles_change_without_edits_or_undo_steps() {
    let target: Id = "01k97krzh1fcx9rk9fhsgt1de7".parse().unwrap();
    let text = format!("[[{target}]]\n\nend");
    let mut e = Editor::new(Body::detached(&text));
    e.move_to(text.len(), false);
    for title in ["**First**", "*Renamed*", ""] {
        e.set_titles(HashMap::from([(target, title.to_owned())]));
        let shown = e.display();
        assert_eq!(shown.labels.len(), 1);
        assert_eq!(e.text(), text);
        assert!(!e.can_undo());
    }
    e.insert("!");
    e.set_titles(HashMap::from([(target, "Later".to_owned())]));
    assert!(e.undo());
    assert_eq!(e.text(), text);
    assert_eq!(e.display().labels[0].text, "Later");
}

/// Note bodies: Labels in display editors.
#[test]
fn link_picker_inserts_only_the_identifier_and_undo_restores_the_brackets() {
    let target: Id = "01k97krzh1fcx9rk9fhsgt1de7".parse().unwrap();
    let mut e = editor("See |");
    assert_eq!(e.type_text("["), None);
    let range = e.type_text("[").unwrap();
    assert_eq!(range, 4..6);
    assert!(e.insert_link(range, "[[", target));
    assert_eq!(e.text(), format!("See [[{target}]]"));
    assert_eq!(e.selection().head, e.text().len());
    assert!(e.undo());
    assert_eq!(e.text(), "See [[");
    assert!(e.redo());
    assert_eq!(e.text(), format!("See [[{target}]]"));
    // An edit while the picker is open must never replace unrelated source.
    assert!(!e.insert_link(0..2, "[[", target));
}

/// Note bodies: Labels in display editors.
#[test]
fn link_picker_preserves_an_explicit_label_with_escapes_and_line_breaks() {
    let target: Id = "01k97krzh1fcx9rk9fhsgt1de7".parse().unwrap();
    let label = "**café** | \\ ]\nnext";
    let mut e = Editor::new(Body::detached(label));
    e.select_all();
    assert_eq!(e.type_text("["), None);
    let range = e.type_text("[").unwrap();
    let expected = e.text().to_owned();
    assert!(e.insert_link(range, &expected, target));
    let links = body::wikilinks(e.text());
    assert_eq!(links[0].label.as_deref(), Some("**café** | \\ ] next"));
    e.set_titles(HashMap::from([(target, label.to_owned())]));
    assert_eq!(e.text(), body::wikilink(target, Some(label)));
    e.insert("\nend");
    assert_eq!(e.display().labels, Vec::<display::Label>::new());
}

#[test]
fn image_pastes_replace_the_original_selection_and_undo_in_one_step() {
    let id: Id = "01k97krzh1fcx9rk9fhsgt1de7".parse().unwrap();
    for text in ["|", "before |after", "- [ ] |task|\n", "é |replace| after"] {
        let mut e = editor(text);
        let selection = e.selection();
        let original = e.text().to_owned();
        let target = e.edit_target();
        e.move_to(0, false);
        assert!(e.insert_image(&target, id), "{text:?}");
        let range = selection.range();
        assert_eq!(
            e.text(),
            format!("{}![[{id}]]{}", &original[..range.start], &original[range.end..])
        );
        assert!(e.undo());
        assert_eq!(e.text(), original);
        assert_eq!(e.selection(), selection);
        assert!(e.redo());
        assert_eq!(body::wikilinks(e.text())[0].target, id);
        assert_eq!(body::wikilinks(e.text())[0].label, None);
    }
}

#[test]
fn a_pending_image_paste_follows_edits_around_its_selection() {
    let id: Id = "01k97krzh1fcx9rk9fhsgt1de7".parse().unwrap();
    let mut e = editor("before |selected| after");
    let target = e.edit_target();
    e.body.insert_utf8(0, "é ").unwrap();
    e.doc.commit_with(CommitOptions::new().origin(ORIGIN));
    assert!(e.insert_image(&target, id));
    assert_eq!(e.text(), format!("é before ![[{id}]] after"));
    assert!(e.undo());
    assert_eq!(e.text(), "é before selected after");
    let target = e.edit_target();
    let selected = e.selection().range();
    e.body.insert_utf8(selected.start + 1, "changed").unwrap();
    e.doc.commit_with(CommitOptions::new().origin(ORIGIN));
    assert!(!e.insert_image(&target, id));
    assert!(e.text().contains("changed"));
}

#[test]
fn moves_onto_images_at_their_source_edges_and_selects_them_whole() {
    let id: Id = "01k97krzh1fcx9rk9fhsgt1de7".parse().unwrap();
    let text = format!("before ![[{id}]] after");
    let mut e = Editor::new(Body::detached(&text));
    e.set_images(HashMap::from([(id, display::ImageSize { width: 80, height: 40 })]));
    let range = 7..38;
    for unit in [Unit::Grapheme, Unit::Word] {
        e.move_to(range.start - 1, false);
        e.move_by(unit, Direction::Forward, false);
        assert_eq!(e.selection().head, range.start);
        assert!(e.display().images[0].revealed);
        e.move_to(range.end + 1, false);
        e.move_by(unit, Direction::Back, false);
        assert_eq!(e.selection().head, range.end);
    }
    e.select(0, range.start + 2);
    e.take_in_syntax(&(0..0));
    assert_eq!(e.selection().head, range.end);
    e.select(text.len(), range.end - 2);
    e.take_in_syntax(&(text.len()..text.len()));
    assert_eq!(e.selection().head, range.start);
    assert!(!e.can_undo());
}

#[test]
fn moves_onto_an_image_past_surrounding_hidden_formatting() {
    let id: Id = "01k97krzh1fcx9rk9fhsgt1de7".parse().unwrap();
    let text = format!("a **![[{id}]]** b");
    let mut e = Editor::new(Body::detached(&text));
    e.set_images(HashMap::from([(id, display::ImageSize { width: 80, height: 40 })]));
    e.move_to(1, false);
    e.move_by(Unit::Grapheme, Direction::Forward, false);
    assert_eq!(e.selection().head, text.find('!').unwrap());
    e.move_to(text.len() - 1, false);
    e.move_by(Unit::Grapheme, Direction::Back, false);
    assert_eq!(e.selection().head, text.find("]]**").unwrap() + 2);
}

#[test]
fn enter_leaves_quotes_and_shift_enter_continues_them_as_single_undo_steps() {
    for (before, enter, continued) in [
        ("> café|", "> café\n\n|", "> café\n> |"),
        (">> text|", ">> text\n>\n> |", ">> text\n>> |"),
        ("> > te|xt", "> > te\n>\n> |xt", "> > te\n> > |xt"),
        ("> text\n> |", "> text\n\n|", "> text\n> \n> |"),
        ("> > a\n> more|", "> > a\n> more\n>\n> |", "> > a\n> more\n>> |"),
        ("- > text|", "- > text\n  \n  |", "- > text\n  > |"),
        ("- item\n  > text|", "- item\n  > text\n  \n  |", "- item\n  > text\n  > |"),
        ("> |text|", "> \n|", "> \n|"),
    ] {
        for (continues, expected) in [(false, enter), (true, continued)] {
            let mut e = editor(before);
            let selection = e.selection();
            e.newline(continues);
            assert_eq!(shown(&e), expected, "{before:?}, continues {continues}");
            assert!(e.undo());
            assert_eq!(shown(&e), before);
            assert_eq!(e.selection(), selection);
            assert!(!e.can_undo());
            assert!(e.redo());
            assert_eq!(shown(&e), expected);
        }
    }
}

#[test]
fn typing_quotes_preserves_selected_lines_and_their_list_indentation() {
    for (before, once, twice) in [
        ("|café\n\nmore|", "|> café\n> \n> more|", "|> > café\n> > \n> > more|"),
        ("- |a\n  b|", "- |> a\n  > b|", "- |> > a\n  > > b|"),
        ("\n|\na\n|\n", "\n\n|> a|\n\n", "\n\n|> > a|\n\n"),
    ] {
        let mut e = editor(before);
        assert_eq!(e.type_text(">"), None);
        assert_eq!(shown(&e), once);
        assert_eq!(e.type_text(">"), None);
        assert_eq!(shown(&e), twice);
        assert!(e.undo());
        assert_eq!(shown(&e), once);
        assert!(e.undo());
        assert_eq!(shown(&e), before);
        assert!(e.redo());
        assert_eq!(shown(&e), once);
        assert!(e.redo());
        assert_eq!(shown(&e), twice);
    }
    for (before, expected) in [("x |a|", "x >|"), ("|a| x", ">| x")] {
        let mut e = editor(before);
        e.type_text(">");
        assert_eq!(shown(&e), expected);
    }
}

#[test]
fn typed_delimiters_wrap_content_and_keep_boundary_line_breaks_outside() {
    for delimiter in ["*", "_", "`", "~", "=", "$", "["] {
        let close = if delimiter == "[" { "]" } else { delimiter };
        let mut e = editor("\n|\ncafé\n|\n");
        e.type_text(delimiter);
        assert_eq!(shown(&e), format!("\n\n{delimiter}|café|{close}\n\n"));
        assert!(e.undo());
        assert_eq!(shown(&e), "\n|\ncafé\n|\n");
    }
    // Pasting a delimiter replaces the selection without applying typing shortcuts.
    let mut e = editor("|text|");
    e.insert("`");
    assert_eq!(shown(&e), "`|");
}

#[test]
fn typing_fences_completes_blocks_and_preserves_the_language_cursor() {
    for (before, typed, expected) in [
        ("``|", "`", "```|\n```"),
        ("- [ ] ``|", "`", "- [ ] ```|\n      ```"),
        ("- item\n  ``|", "`", "- item\n  ```|\n  ```"),
        ("$|", "$", "$$\n|\n$$"),
        ("- item\n  $|", "$", "- item\n  $$\n  |\n  $$"),
    ] {
        let mut e = editor(before);
        e.type_text(typed);
        assert_eq!(shown(&e), expected);
        assert!(e.undo());
        assert_eq!(shown(&e), before);
        assert!(e.redo());
        assert_eq!(shown(&e), expected);
    }
    let mut e = editor("``|");
    e.type_text("`");
    e.type_text("rust");
    e.newline(false);
    assert_eq!(shown(&e), "```rust\n|\n```");
    for before in ["a ``|", "``| a", "```\n``|"] {
        let mut e = editor(before);
        e.type_text("`");
        assert_eq!(shown(&e), before.replace('|', "`|"));
    }
}

#[test]
fn a_third_backtick_and_second_dollar_fence_selected_content() {
    for (before, typed, expected) in [
        ("``|café|``", "`", "```\n|café|\n```"),
        ("- ``|text|``", "`", "- ```\n  |text|\n  ```"),
        ("$|x|$", "$", "$$\n|x|\n$$"),
    ] {
        let mut e = editor(before);
        e.type_text(typed);
        assert_eq!(shown(&e), expected);
        assert!(e.undo());
        assert_eq!(shown(&e), before);
        assert!(e.redo());
        assert_eq!(shown(&e), expected);
    }
}

#[test]
fn lists_and_code_inside_quotes_keep_their_quote_prefixes() {
    for (before, continues, expected) in [
        ("> - one|", false, "> - one\n> - |"),
        ("> - one|", true, "> - one\n>   |"),
        ("> - one\n>   more|", false, "> - one\n>   more\n> - |"),
        ("> - one\n>   more|", true, "> - one\n>   more\n>   |"),
        ("> > - one|", false, "> > - one\n> > - |"),
        ("> - |", false, "> |"),
        ("> ```\n>   code|\n> ```", false, "> ```\n>   code\n>   |\n> ```"),
        ("- > - one|", false, "- > - one\n  > - |"),
        ("```\n  code|\n```", false, "```\n  code\n  |\n```"),
    ] {
        let mut e = editor(before);
        e.newline(continues);
        assert_eq!(shown(&e), expected, "{before:?}");
        assert!(e.undo());
        assert_eq!(shown(&e), before);
    }
    let before = "> - one\n> - two|\n>   more";
    let nested = "> - one\n>   - two|\n>     more";
    let mut e = editor(before);
    assert!(e.list_key(ListKey::Tab));
    assert_eq!(shown(&e), nested);
    assert!(e.undo());
    assert_eq!(shown(&e), before);
    e.list_key(ListKey::Tab);
    assert!(e.list_key(ListKey::ShiftTab));
    assert_eq!(shown(&e), before);
}
