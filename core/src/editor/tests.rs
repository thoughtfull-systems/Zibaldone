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
fn delayed_text_pastes_follow_the_original_selection_and_leave_changed_source_alone() {
    let mut e = editor("before |café| after");
    let target = e.edit_target();
    e.move_to(e.text().len(), false);
    e.insert("!");
    assert!(e.paste(&target, "tea"));
    assert_eq!(e.text(), "before tea after!");
    assert!(e.undo());
    assert_eq!(e.text(), "before café after!");
    assert_eq!(e.selected_text(), "café");
    assert!(e.undo());
    assert_eq!(e.text(), "before café after");

    let start = e.text().find("café").unwrap();
    e.select(start, start + "café".len());
    let target = e.edit_target();
    e.insert("changed");
    assert!(!e.paste(&target, "stale clipboard"));
    assert_eq!(e.text(), "before changed after");
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

#[test]
fn rendered_math_moves_through_source_edges_and_render_updates_never_edit_the_body() {
    let mut e = editor("before $x^2$ after\n|end");
    let source = Drawing::InlineMath("x^2".into());
    assert_eq!(e.drawing_targets().as_slice(), std::slice::from_ref(&source));
    e.set_drawing(
        source.clone(),
        display::Rendered::Image(display::ImageSize { width: 80, height: 40 }),
    );
    for (from, direction, expected) in [(6, Direction::Forward, 7), (13, Direction::Back, 12)] {
        e.move_to(from, false);
        e.move_by(Unit::Grapheme, direction, false);
        assert_eq!(e.selection().head, expected);
        assert!(e.display().drawings[0].revealed);
    }
    e.select(0, 9);
    e.take_in_syntax(&(0..0));
    assert_eq!(e.selection().head, 12);
    e.set_drawing(source, display::Rendered::Error("bad formula".into()));
    assert_eq!(e.text(), "before $x^2$ after\nend");
    assert!(!e.can_undo());
    let e = editor("$x$ and $x$\n\n```mermaid\nflowchart LR\na-->b\n```\n|end");
    assert_eq!(e.drawing_targets().len(), 2);
}

#[test]
fn diagram_and_math_edits_keep_the_previous_result_until_the_current_source_finishes() {
    for text in [
        "```mermaid\na-->b|\n```\nend",
        "$$\nx+y|\n$$\nend",
        "$x+y|$\nend",
        "```mermaid\na-->b|\n```",
        "$$\nx+y|\n$$",
        "$x+y|$",
    ] {
        let mut e = editor(text);
        let original = e.drawing_targets()[0].clone();
        let size = display::ImageSize { width: 120, height: 80 };
        e.set_drawing(original.clone(), display::Rendered::Image(size));
        e.insert("z");
        let intermediate = e.drawing_targets()[0].clone();
        e.insert("w");
        let latest = e.drawing_targets()[0].clone();
        let shown = &e.display().drawings[0];
        assert_eq!(shown.source, latest);
        assert_eq!(shown.preview.as_ref(), Some(&original));
        assert_eq!(shown.rendered, Some(display::Rendered::Image(size)));
        // An older request finishing during this edit cannot replace the preview with a stale edit.
        e.set_drawing(
            intermediate,
            display::Rendered::Image(display::ImageSize { width: 20, height: 20 }),
        );
        assert_eq!(e.display().drawings[0].preview.as_ref(), Some(&original));
        let replacement = display::ImageSize { width: 140, height: 90 };
        e.set_drawing(latest.clone(), display::Rendered::Image(replacement));
        assert_eq!(e.display().drawings[0].preview.as_ref(), Some(&latest));
        assert_eq!(e.display().drawings[0].rendered, Some(display::Rendered::Image(replacement)));
        assert_eq!(e.text(), text.replace('|', "zw"));
    }
}

#[test]
fn previews_follow_each_occurrence_across_other_edits_and_do_not_transfer_to_new_blocks() {
    let mut e = editor("```mermaid\na-->b|\n```\n\n```mermaid\nx-->y\n```\nend");
    let original = e.drawing_targets();
    for (index, source) in original.iter().enumerate() {
        let size = display::ImageSize { width: 100 + u32::try_from(index).unwrap(), height: 50 };
        e.set_drawing(source.clone(), display::Rendered::Image(size));
    }
    e.insert("c");
    e.move_to(0, false);
    e.insert("heading\n\n");
    let source = e.text().find("x-->y").unwrap() + 5;
    e.move_to(source, false);
    e.insert("z");
    let shown = e.display();
    assert_eq!(shown.drawings[0].preview.as_ref(), Some(&original[0]));
    assert_eq!(shown.drawings[1].preview.as_ref(), Some(&original[1]));
    let first = shown.drawings[0].range.clone();
    e.select(first.start, first.end);
    e.insert("plain text");
    e.move_to(0, false);
    e.insert("```mermaid\nnew-->block\n```\n\n");
    let shown = e.display();
    assert_eq!(shown.drawings[0].preview, None);
    assert_eq!(shown.drawings[1].preview.as_ref(), Some(&original[1]));
}

#[test]
fn a_removed_formula_cannot_lend_its_preview_to_a_formula_appended_at_the_note_end() {
    let mut e = editor("$x|$");
    let original = e.drawing_targets()[0].clone();
    e.set_drawing(
        original,
        display::Rendered::Image(display::ImageSize { width: 120, height: 80 }),
    );
    e.move_to(e.text().len(), false);
    e.insert("\n\n$y$");
    e.select(0, 1);
    e.insert("");
    let shown = e.display();
    assert_eq!(shown.drawings.len(), 1);
    assert_eq!(shown.drawings[0].source, Drawing::InlineMath("y".into()));
    assert_eq!(shown.drawings[0].preview, None);
}

#[test]
fn render_errors_replace_previews_when_ready_and_remain_until_the_next_result() {
    let mut e = editor("```mermaid\na-->b|\n```\nend");
    let original = e.drawing_targets()[0].clone();
    e.set_drawing(
        original,
        display::Rendered::Image(display::ImageSize { width: 120, height: 80 }),
    );
    e.insert("bad");
    let invalid = e.drawing_targets()[0].clone();
    let error = display::Rendered::Error("cannot render".into());
    e.set_drawing(invalid.clone(), error.clone());
    assert_eq!(e.display().drawings[0].rendered, Some(error.clone()));
    e.insert("fixed");
    assert_eq!(e.display().drawings[0].preview.as_ref(), Some(&invalid));
    assert_eq!(e.display().drawings[0].rendered, Some(error));
    let latest = e.drawing_targets()[0].clone();
    e.set_drawing(
        latest.clone(),
        display::Rendered::Image(display::ImageSize { width: 140, height: 90 }),
    );
    assert_eq!(e.display().drawings[0].preview.as_ref(), Some(&latest));
}

#[test]
fn table_tabs_align_pipe_cells_and_undo_restores_the_original_source() {
    let before = "| h | other |\n|---|---|\n| a | b |\n\nend";
    let mut e = Editor::new(Body::detached(before));
    e.move_to(before.find(" a ").unwrap() + 1, false);
    let selected = e.selection();
    assert!(e.table_key(false));
    assert_eq!(e.selected_text(), "b");
    let aligned = e.text().to_owned();
    assert!(e.table_key(true));
    assert_eq!(e.selected_text(), "a");
    assert_eq!(e.text(), aligned);
    assert!(e.undo());
    assert_eq!(e.text(), before);
    assert_eq!(e.selection(), selected);
    assert!(!e.can_undo());
    assert!(e.redo());
    assert_eq!(e.text(), aligned);
}

#[test]
fn list_table_tabs_select_cells_without_changing_their_markdown() {
    let text = "```table\n- - alpha\n  - beta\n- - gamma\n  - delta\n```\nend";
    let mut e = Editor::new(Body::detached(text));
    e.move_to(text.find("alpha").unwrap(), false);
    for next in ["beta", "gamma", "delta", "delta"] {
        assert!(e.table_key(false));
        assert_eq!(e.selected_text(), next);
    }
    for next in ["gamma", "beta", "alpha", "alpha"] {
        assert!(e.table_key(true));
        assert_eq!(e.selected_text(), next);
    }
    assert_eq!(e.text(), text);
    assert!(!e.can_undo());
}

#[test]
fn clicking_a_table_cell_selects_its_source_for_replacement_and_undo() {
    for text in [
        "| left | **café** |\n|---|---|\n\nend",
        "```table\n- - left\n  - **café**\n```\nend",
        "> ```table\n> - - left\n>   - **café**\n> ```\nend",
        "- ```table\n  - - left\n    - **café**\n  ```\nend",
    ] {
        let mut e = Editor::new(Body::detached(text));
        let range = e.display().drawings[0].range.clone();
        assert!(e.select_table_cell(range, 0, 1, &[]), "{text:?}");
        assert_eq!(e.selected_text(), "**café**");
        e.insert("edited");
        assert_eq!(e.text(), text.replace("**café**", "edited"));
        assert!(e.undo());
        assert_eq!(e.text(), text);
        assert_eq!(e.selected_text(), "**café**");
    }
}

#[test]
fn nested_table_cells_select_and_navigate_their_innermost_source() {
    for inner in ["```table\n- - inner\n  - value\n```", "| inner | value |\n|---|---|"] {
        let content = inner.replace('\n', "\n    ");
        let text = format!("````table\n- - outer\n  - {content}\n````\nend");
        let mut e = Editor::new(Body::detached(&text));
        let range = e.display().drawings[0].range.clone();
        assert!(e.select_table_cell(range, 0, 1, &[(0, 0, 1)]), "{text:?}");
        assert_eq!(e.selected_text(), "value");
        assert!(e.table_key(true));
        assert_eq!(e.selected_text(), "inner");
        assert!(e.table_key(false));
        assert_eq!(e.selected_text(), "value");
        assert_eq!(e.text(), text);
    }
}

#[test]
fn table_availability_collapses_source_and_survives_pending_cell_edits() {
    let mut e = editor("```table\n- - a|\n  - b\n```\nend");
    let table = e.drawing_targets()[0].clone();
    e.set_drawing(table.clone(), display::Rendered::Table);
    e.insert("new");
    assert_eq!(e.display().drawings[0].preview.as_ref(), Some(&table));
    e.move_to(e.text().len(), false);
    assert!(e.display().drawings[0].collapsed());
    assert_eq!(e.display().paragraphs(e.text()).len(), 2);
}

#[test]
fn superseded_render_metadata_is_released_without_losing_the_pending_preview() {
    let mut e = editor("$x|$\nend");
    let size = display::ImageSize { width: 64, height: 64 };
    let mut previous = e.drawing_targets()[0].clone();
    e.set_drawing(previous.clone(), display::Rendered::Image(size));
    for version in 0..8 {
        let end = e.text()[1..].find('$').unwrap() + 1;
        e.select(1, end);
        e.insert(&format!("x_{version}"));
        assert_eq!(e.display().drawings[0].preview.as_ref(), Some(&previous));
        let current = e.drawing_targets()[0].clone();
        e.set_drawing(current.clone(), display::Rendered::Image(size));
        assert_eq!(e.drawings.len(), 1);
        assert_eq!(e.display().drawings[0].preview.as_ref(), Some(&current));
        previous = current;
    }
}

#[test]
fn encoded_live_selections_keep_direction_and_use_the_body_not_display_text() {
    let mut e = editor("é **bold**\nend");
    e.select(11, 3);
    let encoded = e.encoded_selection().unwrap();
    let position =
        |bytes: &[u8]| e.doc.get_cursor_pos(&Cursor::decode(bytes).unwrap()).unwrap().current.pos;
    assert_eq!(position(&encoded.anchor), 10);
    assert_eq!(position(&encoded.head), 2);
    let mut empty = editor("");
    let empty_selection = empty.encoded_selection().unwrap();
    let machine = Id::generate();
    empty.presence().started(machine);
    empty.presence().received(machine, vec![empty_selection]);
    assert_eq!(empty.remote_selections()[0].selection, Selection::cursor(0));
}

#[test]
fn remote_selections_follow_local_and_imported_edits_without_affecting_undo_or_syntax() {
    let mut e = editor("é **bold**\n|end");
    let other = LoroDoc::new();
    other.import(&e.doc.export(ExportMode::Snapshot).unwrap()).unwrap();
    let mut peer = Editor::new(Body { text: other.get_text("body"), doc: other });
    peer.select(9, 5);
    let machine = Id::generate();
    e.presence().started(machine);
    e.presence().received(machine, vec![peer.encoded_selection().unwrap()]);
    e.presence().set_name(machine, "Peer <&>".to_owned());
    let hidden = e.display().hidden.clone();
    let remote = &e.remote_selections()[0];
    assert_eq!(remote.name, "Peer <&>");
    assert_eq!(&e.text()[remote.selection.range()], "bold");
    assert_eq!(remote.selection, Selection { anchor: 9, head: 5 });
    assert_eq!(e.display().hidden, hidden);
    assert!(!e.can_undo());
    e.move_to(0, false);
    e.insert("local ");
    assert_eq!(e.remote_selections()[0].selection, Selection { anchor: 15, head: 11 });
    peer.move_to(0, false);
    peer.insert("remote ");
    e.doc.import(&peer.doc.export(ExportMode::Snapshot).unwrap()).unwrap();
    assert!(e.refresh());
    let selected = e.remote_selections()[0].selection.range();
    assert_eq!(&e.text()[selected], "bold");
    assert!(e.undo());
    assert_eq!(e.text(), "remote é **bold**\nend");
    assert_eq!(e.remote_selections()[0].selection, Selection { anchor: 16, head: 12 });
    assert!(!e.can_undo());
    assert!(e.redo());
    assert!(e.text().contains("local "));
    assert!(e.text().contains("remote "));
}

#[test]
fn live_cursors_wait_for_their_operations_and_ignore_other_bodies_and_invalid_bytes() {
    let mut e = editor("one\n|end");
    let other = LoroDoc::new();
    other.import(&e.doc.export(ExportMode::Snapshot).unwrap()).unwrap();
    let mut peer = Editor::new(Body { text: other.get_text("body"), doc: other });
    peer.move_to(0, false);
    peer.insert("café ");
    peer.select(0, 5);
    let remote = peer.encoded_selection().unwrap();
    let machine = Id::generate();
    let foreign = e.doc.get_text("another-body");
    foreign.insert(0, "another note").unwrap();
    e.doc.commit_with(CommitOptions::new().origin(ORIGIN));
    let cursor = foreign.get_cursor(2, Side::Middle).unwrap().encode();
    let wrong_body = crate::sync::Selection { anchor: cursor.clone(), head: cursor };
    let invalid = crate::sync::Selection { anchor: vec![255], head: vec![] };
    e.presence().started(machine);
    e.presence().received(machine, vec![remote, wrong_body, invalid]);
    assert_eq!(e.remote_selections(), vec![]);
    e.doc.import(&peer.doc.export(ExportMode::Snapshot).unwrap()).unwrap();
    e.refresh();
    assert_eq!(e.remote_selections().len(), 1);
    assert_eq!(e.remote_selections()[0].selection, Selection { anchor: 0, head: 5 });
    e.presence().received(machine, vec![]);
    assert_eq!(e.remote_selections(), vec![]);
}

#[test]
fn concurrent_edits_keep_a_backward_selection_and_undo_only_local_typing() {
    let mut e = editor("start café fin");
    e.select(11, 6);
    let other = LoroDoc::new();
    other.import(&e.doc.export(ExportMode::Snapshot).unwrap()).unwrap();
    let mut peer = Editor::new(Body { text: other.get_text("body"), doc: other });
    peer.move_to(0, false);
    peer.insert("before ");
    e.doc.import(&peer.doc.export(ExportMode::Snapshot).unwrap()).unwrap();
    e.refresh();
    assert_eq!(e.selection(), Selection { anchor: 18, head: 13 });
    assert_eq!(e.selected_text(), "café");
    e.insert("tea");
    peer.move_to(peer.text().len(), false);
    peer.insert(" after");
    e.doc.import(&peer.doc.export(ExportMode::Snapshot).unwrap()).unwrap();
    e.refresh();
    assert_eq!(e.text(), "before start tea fin after");
    assert!(e.undo());
    assert_eq!(e.text(), "before start café fin after");
    assert_eq!(e.selected_text(), "café");
    assert!(!e.can_undo());
    assert!(e.redo());
    assert_eq!(e.text(), "before start tea fin after");
}

#[test]
fn an_import_that_replaces_the_same_text_still_resolves_the_owners_cursors() {
    let mut e = editor("a|bc|d");
    let before = e.selection();
    let cursors = e.cursors.clone().unwrap();
    let other = LoroDoc::new();
    other.import(&e.doc.export(ExportMode::Snapshot).unwrap()).unwrap();
    let body = other.get_text("body");
    body.delete(1, 2).unwrap();
    body.insert(1, "bc").unwrap();
    other.commit();
    e.doc.import(&other.export(ExportMode::Snapshot).unwrap()).unwrap();
    let expected =
        Selection { anchor: e.find(&cursors.0).unwrap(), head: e.find(&cursors.1).unwrap() };
    assert_ne!(expected, before);
    assert!(e.refresh());
    assert_eq!(e.selection(), expected);
    assert_eq!(e.text(), "abcd");
    assert!(!e.refresh());
}
