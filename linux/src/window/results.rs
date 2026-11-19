//! Search result rows keep type, work details, breadcrumbs, and all excerpts separate.

use std::collections::HashMap;

use adw::prelude::*;
use gtk::{gdk, glib};
use zib_core::Id;
use zib_core::search::{Hit, Sort};
use zib_core::work::Tree;

use crate::app::core;

use super::{Window, snippet_markup};

struct ResultItem {
    hit: Hit,
    space: Id,
}

impl Window {
    pub(super) fn reload_notes(&self) {
        let work = &self.inner().work;
        if work.popovers.get() > 0 {
            work.results_pending.set(true);
            return;
        }
        let generation = self.inner().generation.get() + 1;
        self.inner().generation.set(generation);
        let (w, state) = (self.clone(), self.inner().state.clone());
        let query = self.search_definition().query;
        let sort = self.inner().sort.get();
        let spaces = self.search_spaces();
        glib::spawn_future_local(async move {
            let names: HashMap<_, _> = spaces.iter().cloned().collect();
            let loaded = core(async move {
                let mut rows = Vec::new();
                if query.trim().is_empty() && sort != Sort::Due {
                    let mut notes = Vec::new();
                    for (space, _) in &spaces {
                        notes.extend(
                            state.notes(*space).await?.into_iter().map(|note| (*space, note)),
                        );
                    }
                    notes.sort_by(|(a_space, a), (b_space, b)| {
                        let order = if sort == Sort::Title {
                            Sort::compare_titles(&a.plain, &b.plain)
                        } else {
                            b.updated.cmp(&a.updated)
                        };
                        order.then_with(|| a.id.cmp(&b.id)).then_with(|| a_space.cmp(b_space))
                    });
                    rows.extend(notes.into_iter().map(|(space, note)| ResultItem {
                        hit: Hit {
                            id: note.id,
                            title: note.plain,
                            type_name: note.type_name,
                            snippets: Vec::new(),
                            snippet: String::new(),
                        },
                        space,
                    }));
                } else {
                    let ids: Vec<_> = spaces.iter().map(|(id, _)| *id).collect();
                    rows.extend(
                        state
                            .search_shown(&ids, &query, sort, 50)?
                            .into_iter()
                            .map(|(space, hit)| ResultItem { hit, space }),
                    );
                }
                let mut trees = HashMap::new();
                for (space, _) in spaces {
                    trees.insert(space, state.work_tree(space).await?);
                }
                Ok::<_, zib_core::Error>((rows, trees))
            })
            .await;
            if w.inner().generation.get() != generation {
                return;
            }
            if w.inner().work.popovers.get() > 0 {
                w.inner().work.results_pending.set(true);
                return;
            }
            let (rows, trees) = match loaded {
                Ok(loaded) => loaded,
                Err(error) => {
                    w.search_error(&error);
                    return;
                }
            };
            w.show_results(rows, &trees, &names);
        });
    }

    fn show_results(
        &self,
        rows: Vec<ResultItem>,
        trees: &HashMap<Id, Tree>,
        names: &HashMap<Id, String>,
    ) {
        self.search_validated(true);
        let inner = self.inner();
        let selected = inner
            .list
            .selected_row()
            .and_then(|row| usize::try_from(row.index()).ok())
            .and_then(|index| inner.rows.borrow().get(index).copied());
        let adjustment = inner.results_scroll.vadjustment();
        let scroll = adjustment.value();
        inner.list.remove_all();
        for item in &rows {
            let row = self.result_row(item, trees.get(&item.space), names);
            inner.list.append(&row);
            if Some((item.space, item.hit.id)) == selected {
                inner.list.select_row(Some(&row));
            }
        }
        *inner.rows.borrow_mut() = rows.into_iter().map(|item| (item.space, item.hit.id)).collect();
        glib::idle_add_local_once(move || adjustment.set_value(scroll));
    }

    fn result_row(
        &self,
        item: &ResultItem,
        tree: Option<&Tree>,
        names: &HashMap<Id, String>,
    ) -> gtk::ListBoxRow {
        style();
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .margin_start(10)
            .margin_end(10)
            .margin_top(6)
            .margin_bottom(6)
            .build();
        let heading = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let title = if item.hit.title.trim().is_empty() { "Untitled" } else { &item.hit.title };
        heading.append(
            &gtk::Label::builder()
                .label(title)
                .css_classes(["zib-work-title-text"])
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build(),
        );
        let pill = gtk::Label::new(Some(&item.hit.type_name));
        pill.add_css_class("caption");
        pill.add_css_class("zib-type-pill");
        heading.append(&pill);
        content.append(&heading);
        let mut breadcrumbs = Vec::new();
        if names.len() > 1
            && let Some(name) = names.get(&item.space)
        {
            breadcrumbs.push(name.clone());
        }
        let row = gtk::ListBoxRow::builder().child(&content).activatable(true).build();
        if let Some(tree) = tree
            && let Some(work) = tree.items.get(&item.hit.id)
        {
            let details = self.work_details(item.space, work, tree);
            content.append(&details);
            let path = tree.path(item.hit.id);
            if !path.is_empty() {
                breadcrumbs.push(path);
            }
            super::tasks::style_work_row(&row, work, tree);
        }
        if !breadcrumbs.is_empty() {
            let path =
                gtk::Label::builder().label(breadcrumbs.join(" › ")).xalign(0.0).wrap(true).build();
            path.add_css_class("caption");
            path.add_css_class("dim-label");
            path.add_css_class("zib-result-path");
            content.append(&path);
        }
        for excerpt in &item.hit.snippets {
            let label = gtk::Label::builder()
                .label(snippet_markup(excerpt))
                .use_markup(true)
                .wrap(true)
                .xalign(0.0)
                .margin_start(12)
                .build();
            label.add_css_class("caption");
            label.add_css_class("dim-label");
            label.add_css_class("zib-result-snippet");
            content.append(&label);
        }
        row
    }
}

fn style() {
    thread_local! { static ADDED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
    if ADDED.replace(true) {
        return;
    }
    let Some(display) = gdk::Display::default() else { return };
    let provider = gtk::CssProvider::new();
    provider.load_from_string(
        "label.zib-type-pill { background: alpha(currentColor, 0.1); \
        border-radius: 99px; padding: 1px 6px; }",
    );
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
