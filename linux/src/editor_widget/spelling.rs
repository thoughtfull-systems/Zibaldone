//! The note editor's spelling marks and correction menu.

use std::ops::Range;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use adw::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, gio, glib, pango};
use zib_core::spelling::Dictionary;

use super::{Content, EditorWidget, PasteTarget, Piece, units};

const CHECK_PAUSE: Duration = Duration::from_millis(150);

#[derive(Clone, Copy)]
pub(crate) enum SpellingAction {
    AddToSpace,
    IgnoreInNote,
}

#[derive(Default)]
pub(super) struct Check {
    generation: u64,
    pending: Option<glib::SourceId>,
    misspellings: Vec<Range<usize>>,
    pub(super) menu: Option<gtk::PopoverMenu>,
}

impl Check {
    pub(super) fn cancel(&mut self) {
        if let Some(pending) = self.pending.take() {
            pending.remove();
        }
        self.generation = self.generation.wrapping_add(1);
    }
}

fn base_dictionary() -> Option<&'static Dictionary> {
    static BASE: OnceLock<Option<Dictionary>> = OnceLock::new();
    BASE.get_or_init(|| {
        let dir =
            Path::new(option_env!("ZIB_SPELL_DICTIONARY_DIR").unwrap_or("/usr/share/hunspell"));
        let affix = std::fs::read_to_string(dir.join("en_US.aff")).ok()?;
        let words = std::fs::read_to_string(dir.join("en_US.dic")).ok()?;
        Dictionary::new(&affix, &words).ok()
    })
    .as_ref()
}

impl EditorWidget {
    /// Enables spelling and connects the actions that accept a word.
    pub(crate) fn connect_spelling_word(&self, f: impl Fn(SpellingAction, String) + 'static) {
        *self.imp().spelling_word.borrow_mut() =
            Some(Box::new(move |(action, word)| f(action, word)));
    }

    /// Schedules a check after changes to the source or its space's dictionary.
    pub(super) fn check_spelling(&self, text_changed: bool) {
        let imp = self.imp();
        let mut check = imp.spelling.borrow_mut();
        check.cancel();
        if text_changed {
            check.misspellings.clear();
        }
        if imp.spelling_word.borrow().is_none() || imp.editor.borrow().is_none() {
            return;
        }
        let generation = check.generation;
        let widget = self.downgrade();
        check.pending = Some(glib::timeout_add_local_once(CHECK_PAUSE, move || {
            if let Some(widget) = widget.upgrade() {
                widget.run_spelling_check(generation);
            }
        }));
    }

    pub(crate) fn refresh_spelling(&self) {
        self.check_spelling(false);
    }

    fn run_spelling_check(&self, generation: u64) {
        self.imp().spelling.borrow_mut().pending.take();
        let (text, accepted) = {
            let editor = self.imp().editor.borrow();
            let Some(editor) = editor.as_ref() else { return };
            (editor.text().to_owned(), editor.spelling_words())
        };
        let widget = self.downgrade();
        glib::spawn_future_local(async move {
            let checked = gio::spawn_blocking(move || {
                base_dictionary().map_or_else(Vec::new, |base| base.misspellings(&text, &accepted))
            })
            .await;
            let (Some(widget), Ok(checked)) = (widget.upgrade(), checked) else { return };
            let mut check = widget.imp().spelling.borrow_mut();
            if check.generation != generation || check.misspellings == checked {
                return;
            }
            check.misspellings = checked;
            drop(check);
            widget.relayout();
        });
    }

    pub(super) fn underline_misspellings(
        &self,
        attributes: &pango::AttrList,
        pieces: &[Piece],
        line: Range<usize>,
    ) {
        let check = self.imp().spelling.borrow();
        let first = check.misspellings.partition_point(|word| word.end <= line.start);
        for word in check.misspellings[first..].iter().take_while(|word| word.start < line.end) {
            for piece in pieces.iter().filter(|p| p.content == Content::Source) {
                let start = word.start.max(piece.source);
                let end = word.end.min(piece.source + piece.source_len);
                if start >= end {
                    continue;
                }
                let index =
                    |at| u32::try_from(piece.shown + (at - piece.source)).unwrap_or(u32::MAX);
                for mut attr in [
                    pango::AttrInt::new_underline(pango::Underline::Error).upcast(),
                    pango::AttrColor::new_underline_color(0xe600, 0x3400, 0x3400).upcast(),
                ] {
                    attr.set_start_index(index(start));
                    attr.set_end_index(index(end));
                    attributes.change(attr);
                }
            }
        }
    }

    fn misspelling_at(&self, x: f64, y: f64) -> Option<Range<usize>> {
        let lines = self.imp().lines.borrow();
        let line = lines.iter().find(|line| line.y <= y && y < line.y + line.height)?;
        let (inside, index, _) = line.layout.xy_to_index(units(x - line.left), units(y - line.y));
        let index = usize::try_from(index).ok()?;
        if !inside
            || !line.pieces.iter().any(|p| {
                p.content == Content::Source && (p.shown..p.shown + p.len).contains(&index)
            })
        {
            return None;
        }
        let at = line.source(index);
        let check = self.imp().spelling.borrow();
        let i = check.misspellings.partition_point(|word| word.end <= at);
        check.misspellings.get(i).filter(|word| word.contains(&at)).cloned()
    }

    pub(super) fn spelling_menu(&self, x: f64, y: f64) -> bool {
        if self.imp().read_only.get() {
            return false;
        }
        let Some(range) = self.misspelling_at(x, y) else { return false };
        let (word, target) = {
            let editor = self.imp().editor.borrow();
            let Some(editor) = editor.as_ref() else { return false };
            let Some(target) = editor.edit_target_for(range.clone()) else { return false };
            (
                editor.text()[range].to_owned(),
                PasteTarget { session: self.session(), selection: target },
            )
        };
        let Some(dictionary) = base_dictionary() else { return false };
        let suggestions = gio::Menu::new();
        for suggestion in dictionary.suggestions(&word).into_iter().take(6) {
            let item = gio::MenuItem::new(Some(&suggestion), None);
            item.set_action_and_target_value(
                Some("spelling.replace"),
                Some(&suggestion.to_variant()),
            );
            suggestions.append_item(&item);
        }
        if suggestions.n_items() == 0 {
            suggestions.append(Some("No suggestions"), None);
        }
        let model = gio::Menu::new();
        model.append_section(None, &suggestions);
        model.append(Some("Add to Space Dictionary"), Some("spelling.add"));
        model.append(Some("Ignore in This Note"), Some("spelling.ignore"));
        let menu = gtk::PopoverMenu::from_model(Some(&model));
        #[allow(clippy::cast_possible_truncation)]
        menu.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        menu.set_has_arrow(false);
        let actions = gio::SimpleActionGroup::new();
        let replace = gio::SimpleAction::new("replace", Some(glib::VariantTy::STRING));
        let widget = self.downgrade();
        replace.connect_activate(move |_, value| {
            if let (Some(widget), Some(text)) =
                (widget.upgrade(), value.and_then(glib::Variant::str))
            {
                widget.paste_text(&target, text);
            }
        });
        actions.add_action(&replace);
        let session = self.session();
        for (name, choice) in
            [("add", SpellingAction::AddToSpace), ("ignore", SpellingAction::IgnoreInNote)]
        {
            let action = gio::SimpleAction::new(name, None);
            let widget = self.downgrade();
            let word = word.clone();
            action.connect_activate(move |_, _| {
                if let Some(widget) = widget.upgrade()
                    && widget.session() == session
                    && let Some(accept) = widget.imp().spelling_word.borrow().as_ref()
                {
                    accept((choice, word.clone()));
                }
            });
            actions.add_action(&action);
        }
        menu.insert_action_group("spelling", Some(&actions));
        let widget = self.downgrade();
        menu.connect_closed(move |menu| {
            if let Some(widget) = widget.upgrade() {
                let mut check = widget.imp().spelling.borrow_mut();
                if check.menu.as_ref() == Some(menu) {
                    check.menu.take();
                }
            }
            let menu = menu.clone();
            glib::idle_add_local_once(move || {
                if menu.parent().is_some() {
                    menu.unparent();
                }
            });
        });
        let previous = self.imp().spelling.borrow_mut().menu.replace(menu.clone());
        if let Some(previous) = previous {
            previous.popdown();
        }
        menu.set_parent(self);
        menu.popup();
        true
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashSet;
    use std::rc::Rc;
    use std::time::Instant;

    use zib_core::Id;
    use zib_core::editor::Editor;
    use zib_core::space::Space;

    use super::*;
    use crate::editor_widget::pixels;

    // Omit the second "l" to exercise the correction path.
    const MISSPELLED: &str = concat!("spel", "ing");

    #[test]
    fn packaged_dictionary_checks_and_suggests() {
        if option_env!("ZIB_SPELL_DICTIONARY_DIR").is_none() {
            return;
        }
        let dictionary = base_dictionary().expect("the packaged dictionary loads");
        let text = format!("Check {MISSPELLED}");
        let words: Vec<_> = dictionary
            .misspellings(&text, &HashSet::new())
            .into_iter()
            .map(|range| &text[range])
            .collect();
        assert_eq!(words, [MISSPELLED]);
        assert!(dictionary.suggestions(MISSPELLED).iter().take(6).any(|word| word == "spelling"));
    }

    #[test]
    fn spelling_menu_replaces_ignores_in_one_note_and_adds_to_the_space() {
        if adw::init().is_err() || base_dictionary().is_none() {
            return;
        }
        let space = Rc::new(RefCell::new(Space::create(Id::generate()).unwrap()));
        let source = format!("A **{MISSPELLED}** test with {MISSPELLED}");
        let note = space.borrow_mut().create_note("note", &source).unwrap();
        let widget = EditorWidget::default();
        let dictionary_space = space.clone();
        let editor = widget.downgrade();
        widget.connect_spelling_word(move |action, word| {
            let mut space = dictionary_space.borrow_mut();
            match action {
                SpellingAction::AddToSpace => {
                    space.add_spelling_word(&word).unwrap();
                }
                SpellingAction::IgnoreInNote => space.ignore_spelling_word(note, &word).unwrap(),
            }
            editor.upgrade().unwrap().refresh_spelling();
        });
        widget.set_editor(Some(Editor::new(space.borrow_mut().open_body(note).unwrap())));
        let window = gtk::Window::builder().child(&widget).default_width(400).build();
        window.present();
        wait_until(|| widget.imp().spelling.borrow().misspellings.len() == 2);

        let point = || {
            let lines = widget.imp().lines.borrow();
            let line = &lines[0];
            let shown = u32::try_from(line.shown(4)).unwrap();
            let attrs = line.layout.attributes().unwrap().attributes();
            assert!(attrs.iter().any(|attr| {
                attr.type_() == pango::AttrType::UnderlineColor
                    && attr.start_index() == shown
                    && attr.end_index() == shown + u32::try_from(MISSPELLED.len()).unwrap()
            }));
            let rect = line.layout.index_to_pos(i32::try_from(shown).unwrap());
            (
                line.left + pixels(rect.x()) + pixels(rect.width()) / 2.0,
                line.y + pixels(rect.y()) + pixels(rect.height()) / 2.0,
            )
        };
        let (x, y) = point();
        assert!(widget.spelling_menu(x, y));
        let menu = widget.imp().spelling.borrow().menu.clone().unwrap();
        menu.activate_action("spelling.replace", Some(&"spelling".to_variant())).unwrap();
        menu.popdown();
        assert_eq!(widget.text().unwrap(), format!("A **spelling** test with {MISSPELLED}"));
        widget.edit(|editor| assert!(editor.undo()));
        assert_eq!(widget.text().as_deref(), Some(source.as_str()));

        wait_until(|| widget.imp().spelling.borrow().misspellings.len() == 2);
        let (x, y) = point();
        assert!(widget.spelling_menu(x, y));
        let menu = widget.imp().spelling.borrow().menu.clone().unwrap();
        menu.activate_action("spelling.ignore", None).unwrap();
        menu.popdown();
        wait_until(|| widget.imp().spelling.borrow().misspellings.is_empty());
        assert_eq!(widget.text().as_deref(), Some(source.as_str()));

        let other = space.borrow_mut().create_note("note", &source).unwrap();
        widget.set_editor(Some(Editor::new(space.borrow_mut().open_body(other).unwrap())));
        wait_until(|| widget.imp().spelling.borrow().misspellings.len() == 2);
        let (x, y) = point();
        assert!(widget.spelling_menu(x, y));
        let menu = widget.imp().spelling.borrow().menu.clone().unwrap();
        menu.activate_action("spelling.add", None).unwrap();
        menu.popdown();
        wait_until(|| widget.imp().spelling.borrow().misspellings.is_empty());
        assert_eq!(widget.text().as_deref(), Some(source.as_str()));
        let dictionary = space.borrow_mut().spelling_dictionary().unwrap();
        assert_eq!(
            space.borrow().note(dictionary).unwrap().body,
            format!("# Spelling Dictionary\n\n{MISSPELLED}")
        );
        let text = format!("{} again", MISSPELLED.to_uppercase());
        let next = space.borrow_mut().create_note("note", &text).unwrap();
        let next = Editor::new(space.borrow_mut().open_body(next).unwrap());
        assert_eq!(
            base_dictionary().unwrap().misspellings(next.text(), &next.spelling_words()),
            [] as [Range<usize>; 0]
        );
        window.destroy();
    }

    fn wait_until(mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready() {
            assert!(Instant::now() < deadline, "the spelling check didn't finish");
            glib::MainContext::default().iteration(false);
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
