use zib_core::channel::Dirs;

use super::*;
use crate::test_support::{init_gtk, present, until};

fn editor_keys(widget: &EditorWidget) -> gtk::EventControllerKey {
    let controllers = widget.observe_controllers();
    (0..controllers.n_items())
        .filter_map(|i| controllers.item(i)?.downcast::<gtk::EventControllerKey>().ok())
        .find(|keys| keys.im_context().is_some())
        .expect("the editor's key controller")
}

fn input(widget: &EditorWidget) -> gtk::IMContext {
    editor_keys(widget).im_context().expect("the editor's input method")
}

fn descendants(root: &impl IsA<gtk::Widget>) -> Vec<gtk::Widget> {
    let mut pending = vec![root.as_ref().clone()];
    let mut result = Vec::new();
    while let Some(widget) = pending.pop() {
        let mut child = widget.first_child();
        while let Some(next) = child {
            child = next.next_sibling();
            pending.push(next);
        }
        result.push(widget);
    }
    result
}

fn task_title(window: &Window, title: &str) -> Option<gtk::Entry> {
    descendants(&window.inner().work.widget)
        .into_iter()
        .filter_map(|widget| widget.downcast::<gtk::Entry>().ok())
        .find(|entry| entry.has_css_class("zib-work-title-entry") && entry.text() == title)
}

fn task_row(window: &Window, title: &str) -> Option<gtk::Widget> {
    let mut next = task_title(window, title)?.parent();
    while let Some(widget) = next {
        if widget.has_css_class("zib-work-row") {
            return Some(widget);
        }
        next = widget.parent();
    }
    None
}

fn edit_title(window: &Window, title: &str) -> gtk::Entry {
    let entry = task_title(window, title).unwrap();
    let view = entry.parent().unwrap().downcast::<gtk::Stack>().unwrap();
    view.child_by_name("label").unwrap().downcast::<gtk::Button>().unwrap().emit_clicked();
    entry
}

fn work_button(window: &Window, label: &str) -> Option<gtk::Button> {
    descendants(&window.inner().work.widget)
        .into_iter()
        .filter_map(|widget| widget.downcast::<gtk::Button>().ok())
        .find(|button| button.label().as_deref() == Some(label))
}

fn reveal_work_addition(window: &Window, button: &str, placeholder: &str) -> gtk::Entry {
    until(|| work_button(window, button).is_some_and(|button| button.is_mapped()));
    work_button(window, button).unwrap().emit_clicked();
    let focused_title = || {
        let focus = gtk::prelude::RootExt::focus(&window.inner().window)?;
        focus
            .clone()
            .downcast::<gtk::Entry>()
            .ok()
            .or_else(|| focus.ancestor(gtk::Entry::static_type()).and_downcast::<gtk::Entry>())
            .filter(|entry| entry.placeholder_text().as_deref() == Some(placeholder))
    };
    until(|| focused_title().is_some());
    focused_title().unwrap()
}

#[test]
fn project_sidebar_splits_notes_and_restores_a_bounded_machine_width() {
    init_gtk();
    let runtime = crate::app::test_runtime();
    let root = tempfile::tempdir().unwrap();
    let dirs = Dirs::under(root.path());
    let state = runtime.block_on(State::for_test(dirs.clone())).unwrap();
    let (space, project, ordinary, standalone) = runtime.block_on(async {
        let space = state.spaces().await[0].0;
        let body = format!("# Project design\n\n{}", "Design notes.\n\n".repeat(80));
        let project = state.create_work(space, "project", &body, None).await.unwrap();
        for (field, date) in [("due-on", "2026-11-30"), ("scheduled-on", "2026-11-20")] {
            state
                .set_work_field(space, project, field, Some(Value::Date(date.into())))
                .await
                .unwrap();
        }
        state.create_work(space, "project", "Subproject", Some(project)).await.unwrap();
        state.create_work(space, "task", "Task", Some(project)).await.unwrap();
        let ordinary = state.create_note(space, "Ordinary").await.unwrap();
        let standalone = state.create_work(space, "task", "Standalone", None).await.unwrap();
        (space, project, ordinary, standalone)
    });
    let app = adw::Application::builder()
        .application_id("systems.thoughtfull.Zibaldone.ProjectPaneTest")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    let window = Window::new(&app, state.clone());
    window.inner().window.set_default_size(1800, 900);
    present(&window.inner().window.clone().upcast());
    let flush = |window: &Window| {
        let done = Rc::new(Cell::new(false));
        let (w, result) = (window.clone(), done.clone());
        glib::spawn_future_local(async move {
            w.finish_project_pane().await;
            result.set(true);
        });
        until(|| done.get());
    };
    window.open_note(space, project);
    until(|| task_title(&window, "Task").is_some());
    flush(&window);
    let inner = window.inner();
    let pane = &inner.project_pane;
    until(|| {
        pane.scroll.width() >= 600
            && (pane.scroll.width() - inner.backlinks.widget.width()).abs() <= 2
    });
    assert_eq!(inner.work.header.parent(), Some(pane.content.clone().upcast()));
    assert_eq!(inner.work.widget.parent(), Some(pane.content.clone().upcast()));
    assert!(!inner.work.widget.is_ancestor(&inner.editor_scroll));
    assert_eq!(runtime.block_on(state.project_sidebar_width()), None);
    assert_ne!(pane.scroll.vadjustment(), inner.editor_scroll.vadjustment());
    inner.editor_scroll.vadjustment().set_value(100.0);
    assert_eq!(pane.scroll.vadjustment().value(), 0.0);

    let resize = |position| {
        assert!(pane.widget.grab_focus());
        pane.widget.set_position(position);
        inner.widget.grab_focus();
        flush(&window);
    };
    let available = pane.scroll.width() + inner.backlinks.widget.width();
    resize(available - 640);
    until(|| pane.scroll.width() == 640);
    assert_eq!(runtime.block_on(state.project_sidebar_width()), Some(640));
    resize(i32::MAX);
    until(|| pane.scroll.width() == 600);
    assert!(pane.scroll.is_visible());
    let row = task_row(&window, "Task").unwrap();
    let grid = row.first_child().unwrap().downcast::<gtk::Grid>().unwrap();
    let controls = grid.child_at(1, 1).unwrap();
    let first = controls.first_child().unwrap().compute_bounds(&controls).unwrap();
    for child in std::iter::successors(
        controls.first_child().unwrap().next_sibling(),
        WidgetExt::next_sibling,
    ) {
        let bounds = child.compute_bounds(&controls).unwrap();
        assert!((bounds.y() - first.y()).abs() <= 1.0, "task controls must stay on one line");
    }
    let bounds = controls.compute_bounds(&pane.scroll).unwrap();
    assert!(bounds.x() + bounds.width() <= 601.0, "task controls must fit in the sidebar");
    resize(0);
    until(|| pane.scroll.width() >= 600 && inner.backlinks.widget.width() >= 280);
    assert!(pane.scroll.width() <= available * 7 / 10);
    assert!(pane.scroll.is_visible());
    resize(available - 640);

    window.open_note(space, ordinary);
    until(|| inner.widget.text().as_deref() == Some("Ordinary"));
    assert!(!pane.scroll.is_visible());
    window.open_note(space, standalone);
    until(|| {
        inner.work.widget.is_visible() && inner.widget.text().as_deref() == Some("Standalone")
    });
    until(|| pane.scroll.is_mapped() && pane.scroll.width() == 640);
    assert_eq!(inner.work.header.parent(), Some(pane.content.clone().upcast()));
    assert_eq!(inner.work.widget.parent(), Some(pane.content.clone().upcast()));
    window.open_note(space, project);
    until(|| pane.scroll.is_mapped() && pane.scroll.width() == 640);
    window.inner().window.close();
    flush(&window);
    window.inner().window.destroy();
    let reopened = runtime.block_on(State::for_test(dirs)).unwrap();
    let next = Window::new(&app, reopened.clone());
    next.inner().window.set_default_size(1800, 900);
    present(&next.inner().window.clone().upcast());
    next.open_note(space, project);
    until(|| {
        next.inner().project_pane.scroll.is_mapped()
            && next.inner().project_pane.scroll.width() == 640
    });
    assert_eq!(runtime.block_on(reopened.project_sidebar_width()), Some(640));
    next.inner().window.destroy();
    runtime.block_on(reopened.set_project_sidebar_width(320)).unwrap();
    let narrow = Window::new(&app, reopened.clone());
    narrow.inner().window.set_default_size(1800, 900);
    present(&narrow.inner().window.clone().upcast());
    narrow.open_note(space, project);
    until(|| narrow.inner().project_pane.scroll.is_mapped());
    flush(&narrow);
    until(|| narrow.inner().project_pane.scroll.width() == 600);
    assert_eq!(runtime.block_on(reopened.project_sidebar_width()), Some(320));
    narrow.inner().window.destroy();
}

#[test]
fn inline_additions_keep_focus_drafts_and_project_context() {
    init_gtk();
    let runtime = crate::app::test_runtime();
    let root = tempfile::tempdir().unwrap();
    let state = runtime.block_on(State::for_test(Dirs::under(root.path()))).unwrap();
    let (space, project) = runtime.block_on(async {
        let space = state.spaces().await[0].0;
        let project = state.create_work(space, "project", "Root", None).await.unwrap();
        (space, project)
    });
    let app = adw::Application::builder()
        .application_id("systems.thoughtfull.Zibaldone.InlineAddTest")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    let window = Window::new(&app, state.clone());
    present(&window.inner().window.clone().upcast());
    window.open_note(space, project);
    let addition = |placeholder| {
        descendants(&window.inner().work.widget)
            .into_iter()
            .filter_map(|widget| widget.downcast::<gtk::Entry>().ok())
            .find(|entry| entry.placeholder_text().as_deref() == Some(placeholder))
    };
    until(|| work_button(&window, "Add a task").is_some());
    assert!(addition("New task").is_none() && addition("New subproject").is_none());
    assert!(descendants(&window.inner().work.widget).iter().all(|widget| {
        widget
            .downcast_ref::<gtk::Label>()
            .is_none_or(|label| label.text() != "Subprojects" && label.text() != "Tasks")
    }));
    let entry = reveal_work_addition(&window, "Add a task", "New task");
    assert!(runtime.block_on(state.work_tree(space)).unwrap().children(project).is_empty());
    assert!(descendants(&window.inner().work.widget).iter().any(|widget| {
        widget.downcast_ref::<gtk::Label>().is_some_and(|label| label.text() == "Tasks")
    }));
    entry.emit_activate();
    assert!(runtime.block_on(state.work_tree(space)).unwrap().children(project).is_empty());
    entry.set_text("First task");
    entry.emit_activate();
    until(|| task_title(&window, "First task").is_some() && entry.text().is_empty());
    assert_eq!(entry, addition("New task").unwrap());
    entry.set_text("Second task");
    entry.emit_by_name::<()>("activate", &[]);
    entry.set_text("Keep typing");
    until(|| task_title(&window, "Second task").is_some());
    assert_eq!(entry.text(), "Keep typing");
    entry.set_text("");
    until(|| task_title(&window, "Second task").is_some() && entry.text().is_empty());
    assert!(
        gtk::prelude::RootExt::focus(&window.inner().window)
            .is_some_and(|focus| focus == entry || focus.is_ancestor(&entry))
    );
    entry.set_text("Keep this draft");
    runtime
        .block_on(state.set_work_field(
            space,
            project,
            "due-on",
            Some(Value::Date("2026-12-31".into())),
        ))
        .unwrap();
    window.refresh_work();
    until(|| window.inner().work.tree.borrow().date(project, "due-on").is_some());
    assert_eq!(entry, addition("New task").unwrap());
    assert_eq!(entry.text(), "Keep this draft");
    let project_entry = reveal_work_addition(&window, "Add a subproject", "New subproject");
    let tree = runtime.block_on(state.work_tree(space)).unwrap();
    assert!(
        tree.children(project)
            .into_iter()
            .all(|id| tree.items[&id].note.type_name == zib_core::schema::TASK)
    );
    project_entry.set_text("Subproject *title*");
    project_entry.emit_activate();
    until(|| {
        task_title(&window, "Subproject *title*").is_some() && project_entry.text().is_empty()
    });
    assert_eq!(project_entry, addition("New subproject").unwrap());
    let aligned = |input: &gtk::Entry, title: &gtk::Widget| {
        let container = &window.inner().work.widget;
        input.width() > 0
            && title.width() > 0
            && input.compute_bounds(container).zip(title.compute_bounds(container)).is_some_and(
                |(input, title)| {
                    (input.x() - title.x()).abs() < 1.0
                        && (input.width() - title.width()).abs() < 1.0
                },
            )
    };
    until(|| aligned(&entry, &task_title(&window, "First task").unwrap().parent().unwrap()));
    until(|| {
        let title = task_title(&window, "Subproject *title*").unwrap().parent().unwrap();
        aligned(&project_entry, &title)
    });
    assert_eq!(window.inner().editor.borrow().as_ref().unwrap().note, project);
    assert!(window.inner().window.visible_dialog().is_none());
    let tree = runtime.block_on(state.work_tree(space)).unwrap();
    assert_eq!(tree.children(project).len(), 3);
    for id in tree.children(project) {
        assert_eq!(tree.items[&id].parent, Some(project));
        assert!(tree.items[&id].explicit_date("due-on").is_none());
    }
    for id in tree.children(project) {
        runtime.block_on(state.delete_note(space, id)).unwrap();
    }
    window.space_changed(space);
    until(|| window.inner().work.tree.borrow().children(project).is_empty());
    assert!(work_button(&window, "Add a task").is_some());
    assert!(work_button(&window, "Add a subproject").is_some());
    assert!(addition("New task").is_none() && addition("New subproject").is_none());
    for (action, kind) in [("win.new-project", "project"), ("win.new-task", "task")] {
        WidgetExt::activate_action(&window.inner().window, action, None).unwrap();
        until(|| {
            window.inner().editor.borrow().as_ref().is_some_and(|e| e.type_name == kind)
                && window.inner().widget.text().as_deref() == Some("")
                && window.inner().widget.is_focus()
        });
        assert!(window.inner().window.visible_dialog().is_none());
    }
    window.inner().window.destroy();
}

#[test]
fn work_rows_align_title_actions_and_details_and_edit_titles_only_on_click() {
    init_gtk();
    let runtime = crate::app::test_runtime();
    let root = tempfile::tempdir().unwrap();
    let state = runtime.block_on(State::for_test(Dirs::under(root.path()))).unwrap();
    let (space, project, subproject, task, done) = runtime.block_on(async {
        let space = state.spaces().await[0].0;
        let project = state.create_work(space, "project", "Root", None).await.unwrap();
        let subproject = state
            .create_work(
                space,
                "project",
                "# Subproject\n\nKeep these design notes.",
                Some(project),
            )
            .await
            .unwrap();
        let task = state.create_work(space, "task", "Task", Some(project)).await.unwrap();
        let done = state.create_work(space, "task", "Finished", Some(project)).await.unwrap();
        state
            .set_work_field(space, done, "completed-on", Some(Value::Date(zib_core::work::today())))
            .await
            .unwrap();
        state
            .set_work_field(space, project, "scheduled-on", Some(Value::Date("2026-11-01".into())))
            .await
            .unwrap();
        state
            .set_work_field(space, project, "due-on", Some(Value::Date("2026-12-01".into())))
            .await
            .unwrap();
        state
            .set_work_status(space, project, Some("waiting".into()), "Reply".into())
            .await
            .unwrap();
        (space, project, subproject, task, done)
    });
    let app = adw::Application::builder()
        .application_id("systems.thoughtfull.Zibaldone.WorkRowTest")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    let window = Window::new(&app, state.clone());
    present(&window.inner().window.clone().upcast());
    window.open_note(space, done);
    until(|| task_title(&window, "Finished").is_some());
    assert_eq!(window.inner().editor.borrow().as_ref().unwrap().note, project);
    let entry = task_title(&window, "Task").unwrap();
    let view = entry.parent().unwrap().downcast::<gtk::Stack>().unwrap();
    assert_eq!(view.visible_child_name().as_deref(), Some("label"));
    assert!(!entry.is_mapped());
    let finished = task_title(&window, "Finished")
        .unwrap()
        .parent()
        .unwrap()
        .downcast::<gtk::Stack>()
        .unwrap();
    let button = finished.child_by_name("label").unwrap().downcast::<gtk::Button>().unwrap();
    assert!(!button.child().unwrap().has_css_class("dim-label"));
    assert!(task_row(&window, "Finished").unwrap().has_css_class("dim-label"));
    let row = task_row(&window, "Task").unwrap();
    let grid = row.first_child().unwrap().downcast::<gtk::Grid>().unwrap();
    until(|| grid.width() > 0 && grid.child_at(1, 1).unwrap().height() > 0);
    let title = view.compute_bounds(&grid).unwrap();
    let details = grid.child_at(1, 1).unwrap().compute_bounds(&grid).unwrap();
    let prefix = grid.child_at(0, 0).unwrap().compute_bounds(&grid).unwrap();
    let actions = grid.child_at(2, 0).unwrap().compute_bounds(&grid).unwrap();
    assert!((title.x() - details.x()).abs() < 1.0);
    assert!(details.y() >= title.y() + title.height());
    assert!((prefix.y() + prefix.height() / 2.0 - title.y() - title.height() / 2.0).abs() < 1.0);
    assert!(actions.x() >= title.x() + title.width());
    let popup = work_popup(&row, "Work status");
    let model = popup.downcast_ref::<gtk::PopoverMenu>().unwrap().menu_model().unwrap();
    let labels: Vec<_> = (0..model.n_items())
        .map(|i| model.item_attribute_value(i, "label", None).unwrap().str().unwrap().to_owned())
        .collect();
    assert_eq!(labels, ["Inherited", "Someday", "Maybe", "Waiting…", "Clear"]);
    popup.popdown();
    until(|| !popup.is_mapped());
    let popup = work_popup(&row, "Scheduled date");
    let date =
        descendants(&popup).into_iter().find_map(|w| w.downcast::<gtk::Entry>().ok()).unwrap();
    date.set_text("2026-11-03");
    date.emit_by_name::<()>("activate", &[]);
    until(|| {
        window
            .inner()
            .work
            .tree
            .borrow()
            .date(task, "scheduled-on")
            .is_some_and(|date| date.source == task)
    });
    let project_entry = edit_title(&window, "Subproject");
    project_entry.set_text("Renamed project");
    project_entry.emit_by_name::<()>("activate", &[]);
    until(|| window.inner().work.tree.borrow().items[&subproject].title == "Renamed project");
    until(|| {
        project_entry
            .parent()
            .unwrap()
            .downcast::<gtk::Stack>()
            .unwrap()
            .visible_child_name()
            .as_deref()
            == Some("label")
    });
    assert!(
        runtime
            .block_on(state.note(space, subproject))
            .unwrap()
            .body
            .ends_with("Keep these design notes.")
    );
    let open = descendants(&task_row(&window, "Renamed project").unwrap())
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Button>().ok())
        .find(|b| b.tooltip_text().as_deref() == Some("Open subproject"))
        .unwrap();
    open.emit_clicked();
    until(|| window.inner().editor.borrow().as_ref().is_some_and(|e| e.note == subproject));
    window.close_editor();
    runtime.block_on(state.finish());
    window.inner().window.destroy();
}

#[test]
fn results_show_type_details_breadcrumbs_and_all_passages_and_edit_the_correct_space() {
    init_gtk();
    let runtime = crate::app::test_runtime();
    let root = tempfile::tempdir().unwrap();
    let state = runtime.block_on(State::for_test(Dirs::under(root.path()))).unwrap();
    let (space, project, task, note, other, elsewhere) = runtime.block_on(async {
        let space = state.spaces().await[0].0;
        let project = state.create_work(space, "project", "Root project", None).await.unwrap();
        let body =
            format!("# Task\n\nneedle first passage {} needle last passage", "gap ".repeat(40));
        let task = state.create_work(space, "task", &body, Some(project)).await.unwrap();
        let note = state.create_note(space, "# Ordinary note\n\nneedle in a note").await.unwrap();
        state
            .set_work_status(space, project, Some("waiting".into()), "Reply".into())
            .await
            .unwrap();
        state
            .set_work_field(space, project, "scheduled-on", Some(Value::Date("2026-11-01".into())))
            .await
            .unwrap();
        state
            .set_work_field(space, project, "due-on", Some(Value::Date("2026-12-01".into())))
            .await
            .unwrap();
        let other = state.create_space("Other space").await.unwrap();
        let elsewhere = state
            .create_work(other, "task", "# Elsewhere\n\nneedle elsewhere", None)
            .await
            .unwrap();
        state.set_work_status(other, elsewhere, Some("maybe".into()), String::new()).await.unwrap();
        for (field, value) in [("due-on", "2026-11-27"), ("scheduled-on", "2026-11-19")] {
            state
                .set_work_field(other, elsewhere, field, Some(Value::Date(value.into())))
                .await
                .unwrap();
        }
        (space, project, task, note, other, elsewhere)
    });
    let app = adw::Application::builder()
        .application_id("systems.thoughtfull.Zibaldone.ResultLayoutTest")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    let window = Window::new(&app, state.clone());
    present(&window.inner().window.clone().upcast());
    window.open_note(space, project);
    until(|| task_title(&window, "Task").is_some());
    window.inner().search.buffer().set_text("needle");
    until(|| window.inner().rows.borrow().len() == 3);
    let result = |id| {
        let index = window.inner().rows.borrow().iter().position(|(_, note)| *note == id).unwrap();
        window.inner().list.row_at_index(i32::try_from(index).unwrap()).unwrap()
    };
    let row = result(task);
    let content = row.child().unwrap();
    let heading = content.first_child().unwrap();
    let pill = heading.last_child().unwrap().downcast::<gtk::Label>().unwrap();
    assert_eq!(pill.text(), "task");
    assert!(pill.has_css_class("zib-type-pill"));
    assert_eq!(
        result(note)
            .child()
            .unwrap()
            .first_child()
            .unwrap()
            .last_child()
            .unwrap()
            .downcast::<gtk::Label>()
            .unwrap()
            .text(),
        "note"
    );
    let details = heading.next_sibling().unwrap();
    let path = details.next_sibling().unwrap();
    assert!(path.has_css_class("zib-result-path"));
    assert!(path.clone().downcast::<gtk::Label>().unwrap().text().contains("Root project"));
    let snippets: Vec<_> =
        descendants(&row).into_iter().filter(|w| w.has_css_class("zib-result-snippet")).collect();
    assert_eq!(snippets.len(), 2);
    until(|| row.width() > 0 && path.height() > 0);
    for snippet in snippets {
        let position = snippet.compute_bounds(&row).unwrap();
        let breadcrumb = path.compute_bounds(&row).unwrap();
        assert!(position.x() > breadcrumb.x());
        assert!(position.y() >= breadcrumb.y() + breadcrumb.height());
    }
    let controls =
        result(elsewhere).child().unwrap().first_child().unwrap().next_sibling().unwrap();
    // Force the due date to wrap; its width must not widen the status on the first line.
    let status = controls.first_child().unwrap();
    let scheduled = status.next_sibling().unwrap();
    let due = scheduled.next_sibling().unwrap();
    let width = status.measure(gtk::Orientation::Horizontal, -1).1
        + scheduled.measure(gtk::Orientation::Horizontal, -1).1
        + 4;
    let measured = controls.measure(gtk::Orientation::Vertical, width).1;
    controls.allocate(width, measured, -1, None);
    let status = status.compute_bounds(&controls).unwrap();
    let scheduled = scheduled.compute_bounds(&controls).unwrap();
    let due = due.compute_bounds(&controls).unwrap();
    assert!((scheduled.x() - status.x() - status.width() - 4.0).abs() <= 1.0);
    assert!(due.y() > status.y());
    let popup = work_popup(&result(elsewhere), "Work status");
    assert!(menu_item(&popup, "Clear").is_none());
    assert!(menu_item(&popup, "Inherited").unwrap().activate());
    until(|| runtime.block_on(state.work_tree(other)).unwrap().status(elsewhere).is_none());
    until(|| !popup.is_mapped());
    assert_eq!(
        runtime.block_on(state.work_tree(space)).unwrap().status(task).unwrap().value,
        "waiting"
    );
    assert_eq!(window.inner().editor.borrow().as_ref().unwrap().note, project);
    window.close_editor();
    runtime.block_on(state.finish());
    window.inner().window.destroy();
}

#[test]
fn search_sidebar_resizes_collapses_and_restores_machine_layout() {
    use zib_core::store::SidebarLayout;

    init_gtk();
    let runtime = crate::app::test_runtime();
    let root = tempfile::tempdir().unwrap();
    let dirs = Dirs::under(root.path());
    let state = runtime.block_on(State::for_test(dirs.clone())).unwrap();
    let app = adw::Application::builder()
        .application_id("systems.thoughtfull.Zibaldone.SearchPaneTest")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    let window = Window::new(&app, state.clone());
    present(&window.inner().window.clone().upcast());
    let flush = |window: &Window| {
        let done = Rc::new(Cell::new(false));
        let (w, finished) = (window.clone(), done.clone());
        glib::spawn_future_local(async move {
            w.finish_search_pane().await;
            finished.set(true);
        });
        until(|| done.get());
    };
    flush(&window);
    let pane = &window.inner().sidebar_split;
    window.inner().search.buffer().set_text("garden AND @type=note");
    pane.widget.set_position(460);
    flush(&window);
    assert_eq!(
        runtime.block_on(state.search_sidebar()),
        Some(SidebarLayout { visible: true, width: 460 })
    );
    pane.widget.set_position(140);
    pane.widget.set_position(90);
    assert!(!pane.shows_sidebar());
    flush(&window);
    assert_eq!(
        runtime.block_on(state.search_sidebar()),
        Some(SidebarLayout { visible: false, width: 460 })
    );
    pane.set_show_sidebar(true);
    assert_eq!(pane.widget.position(), 460);
    assert_eq!(window.search_definition().query, "garden AND @type=note");
    pane.widget.set_position(480);
    window.inner().window.close();
    flush(&window);
    assert_eq!(runtime.block_on(state.search_sidebar()).unwrap().width, 480);
    pane.set_show_sidebar(false);
    flush(&window);
    window.inner().window.destroy();
    let reopened = runtime.block_on(State::for_test(dirs)).unwrap();
    let next = Window::new(&app, reopened);
    flush(&next);
    assert!(!next.inner().sidebar_split.shows_sidebar());
    next.toggle_search();
    assert_eq!(next.inner().sidebar_split.widget.position(), 480);
    flush(&next);
    next.inner().window.destroy();
    let pending = Window::new(&app, next.inner().state.clone());
    pending.toggle_search();
    flush(&pending);
    assert!(!pending.inner().sidebar_split.shows_sidebar());
    assert_eq!(pending.inner().sidebar_split.widget.position(), 480);
    flush(&pending);
    pending.inner().window.destroy();
}

#[test]
fn task_rows_and_results_share_completion_and_inherited_due_emphasis() {
    init_gtk();
    let runtime = crate::app::test_runtime();
    let root = tempfile::tempdir().unwrap();
    let state = runtime.block_on(State::for_test(Dirs::under(root.path()))).unwrap();
    let (space, project, open, overdue, completed, soon) = runtime.block_on(async {
        let space = state.spaces().await[0].0;
        let project = state.create_work(space, "project", "Project", None).await.unwrap();
        let open = state.create_work(space, "task", "Open", Some(project)).await.unwrap();
        let overdue = state.create_work(space, "task", "Overdue", Some(project)).await.unwrap();
        let completed = state.create_work(space, "task", "Completed", Some(project)).await.unwrap();
        let soon = state.create_work(space, "task", "Soon", Some(project)).await.unwrap();
        for field in ["due-on", "scheduled-on"] {
            state
                .set_work_field(space, project, field, Some(Value::Date(zib_core::work::today())))
                .await
                .unwrap();
        }
        state.set_work_status(space, project, Some("maybe".into()), String::new()).await.unwrap();
        state
            .set_work_field(space, open, "due-on", Some(Value::Date("9999-12-31".into())))
            .await
            .unwrap();
        for note in [overdue, completed] {
            state
                .set_work_field(space, note, "due-on", Some(Value::Date("0001-01-01".into())))
                .await
                .unwrap();
        }
        state
            .set_work_field(
                space,
                completed,
                "completed-on",
                Some(Value::Date(zib_core::work::today())),
            )
            .await
            .unwrap();
        (space, project, open, overdue, completed, soon)
    });
    let app = adw::Application::builder()
        .application_id("systems.thoughtfull.Zibaldone.ResultEmphasisTest")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    let window = Window::new(&app, state.clone());
    present(&window.inner().window.clone().upcast());
    window.inner().search.buffer().set_text("@type=task");
    window.open_note(space, completed);
    until(|| window.inner().rows.borrow().len() == 4 && task_row(&window, "Completed").is_some());
    let row = |note| {
        let index = window.inner().rows.borrow().iter().position(|(_, id)| *id == note).unwrap();
        window.inner().list.row_at_index(i32::try_from(index).unwrap()).unwrap()
    };
    let bold = |widget: &gtk::Widget| {
        widget.pango_context().font_description().unwrap().weight() == gtk::pango::Weight::Bold
    };
    let provider = gtk::CssProvider::new();
    provider.load_from_string("label.zib-test-accent { color: var(--accent-bg-color); }");
    gtk::style_context_add_provider_for_display(
        &gtk::gdk::Display::default().unwrap(),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    let mut accent = None;
    for (id, title, class) in [
        (completed, "Completed", "dim-label"),
        (open, "Open", ""),
        (overdue, "Overdue", "zib-work-overdue"),
        (soon, "Soon", "zib-work-warning"),
    ] {
        let work = task_row(&window, title).unwrap();
        let result = row(id);
        for candidate in ["dim-label", "zib-work-overdue", "zib-work-warning"] {
            assert_eq!(work.has_css_class(candidate), candidate == class, "{title}");
            assert_eq!(result.has_css_class(candidate), candidate == class, "{title}");
        }
        let grid = work.first_child().unwrap().downcast::<gtk::Grid>().unwrap();
        let view = grid.child_at(1, 0).unwrap().downcast::<gtk::Stack>().unwrap();
        let button = view.visible_child().unwrap().downcast::<gtk::Button>().unwrap();
        let title = button.child().unwrap();
        let details = grid.child_at(1, 1).unwrap();
        until(|| details.height() > 0);
        assert_eq!(title.color(), work.color(), "title must share the row emphasis");
        let urgent = matches!(class, "zib-work-overdue" | "zib-work-warning");
        assert_eq!(bold(&title), urgent, "urgent task titles must be bold");
        let result_title = result.child().unwrap().first_child().unwrap().first_child().unwrap();
        assert_eq!(bold(&result_title), urgent, "search results must share the same weight");
        for control in std::iter::successors(details.first_child(), WidgetExt::next_sibling) {
            let content = control.downcast::<gtk::MenuButton>().unwrap().child().unwrap();
            let label =
                descendants(&content).into_iter().find(|widget| widget.is::<gtk::Label>()).unwrap();
            assert_eq!(label.color(), title.color(), "title and metadata must share emphasis");
            assert!(!bold(&label), "task details must keep normal weight");
        }
        assert!(!title.has_css_class("dim-label"), "don't dim completed titles twice");
        for parent in [work, result.upcast()] {
            let color = picker_accent(&parent);
            assert_eq!(*accent.get_or_insert(color), color, "menu accents must ignore urgency");
        }
    }
    let manager = adw::StyleManager::default();
    let original = manager.color_scheme();
    // Switch back to light to check updates in both directions without rebuilding the rows.
    for (scheme, color) in [
        (adw::ColorScheme::ForceLight, "#d6a400"),
        (adw::ColorScheme::ForceDark, "#ffc300"),
        (adw::ColorScheme::ForceLight, "#d6a400"),
    ] {
        manager.set_color_scheme(scheme);
        let expected = gtk::gdk::RGBA::parse(color).unwrap();
        until(|| {
            row(soon).color() == expected && task_row(&window, "Soon").unwrap().color() == expected
        });
    }
    manager.set_color_scheme(original);
    runtime.block_on(state.set_work_field(space, completed, "completed-on", None)).unwrap();
    window.space_changed(space);
    until(|| {
        !row(completed).has_css_class("dim-label")
            && task_row(&window, "Completed")
                .is_some_and(|task| task.has_css_class("zib-work-overdue"))
    });
    assert!(row(completed).has_css_class("zib-work-overdue"));
    runtime.block_on(state.set_work_field(space, project, "due-on", None)).unwrap();
    window.space_changed(space);
    until(|| !row(soon).has_css_class("zib-work-warning"));
    until(|| task_row(&window, "Soon").is_some_and(|task| !task.has_css_class("zib-work-warning")));
    window.close_editor();
    runtime.block_on(state.finish());
    window.inner().window.destroy();
}

#[test]
fn remote_title_changes_preserve_dirty_inline_drafts() {
    init_gtk();
    let runtime = crate::app::test_runtime();
    let root = tempfile::tempdir().unwrap();
    let state = runtime.block_on(State::for_test(Dirs::under(root.path()))).unwrap();
    let (space, project, task) = runtime.block_on(async {
        let space = state.spaces().await[0].0;
        let project = state.create_work(space, "project", "Project", None).await.unwrap();
        let task = state.create_work(space, "task", "Task", Some(project)).await.unwrap();
        (space, project, task)
    });
    let app = adw::Application::builder()
        .application_id("systems.thoughtfull.Zibaldone.TitleSyncTest")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    let window = Window::new(&app, state.clone());
    window.open_note(space, task);
    until(|| task_title(&window, "Task").is_some());
    let entry = edit_title(&window, "Task");
    entry.set_text("My draft");
    runtime.block_on(async {
        let document = state.node.spaces().lock().await.get(&space).cloned().unwrap();
        let mut document = document.lock().await;
        let mut remote =
            zib_core::Space::open(&document.snapshot().unwrap(), Id::generate()).unwrap();
        remote.set_body(task, "Remote title\n\nRemote notes").unwrap();
        remote.commit();
        document.import_checked(&remote.snapshot().unwrap()).unwrap();
    });
    window.live_event(space, project, LiveEvent::Changed { machine: Id::generate() });
    until(|| window.inner().work.tree.borrow().items[&task].title == "Remote title");
    assert_eq!(entry.text(), "My draft");
    entry.emit_by_name::<()>("activate", &[]);
    until(|| entry.has_css_class("error"));
    assert_eq!(
        runtime.block_on(state.work_tree(space)).unwrap().items[&task].note.body,
        "Remote title\n\nRemote notes"
    );
    let keys = entry.observe_controllers();
    let keys = (0..keys.n_items())
        .filter_map(|i| keys.item(i)?.downcast::<gtk::EventControllerKey>().ok())
        .next()
        .unwrap();
    keys.emit_by_name::<bool>(
        "key-pressed",
        &[&gdk::Key::Escape, &0u32, &gdk::ModifierType::empty()],
    );
    assert_eq!(entry.text(), "Remote title");
    window.close_editor();
    runtime.block_on(state.finish());
    window.inner().window.destroy();
}

#[track_caller]
fn work_popup(parent: &impl IsA<gtk::Widget>, tooltip: &str) -> gtk::Popover {
    let mut button = None;
    // A second refresh can replace a row before its first allocation. Follow the mapped control.
    until(|| {
        button = descendants(parent)
            .into_iter()
            .filter_map(|widget| widget.downcast::<gtk::MenuButton>().ok())
            .find(|button| {
                button.tooltip_text().is_some_and(|text| text.starts_with(tooltip))
                    && button.is_mapped()
                    && button.width() > 0
                    && button.height() > 0
            });
        button.is_some()
    });
    let button = button.unwrap();
    button.popup();
    let popup = button.popover().unwrap();
    until(|| popup.is_mapped());
    popup
}

fn picker_row_metrics(popup: &gtk::Popover) -> (i32, f32, f32) {
    let mut row = None;
    until(|| {
        row = descendants(popup)
            .into_iter()
            .filter_map(|widget| widget.downcast::<gtk::ListBoxRow>().ok())
            .find(|row| row.is_mapped() && row.width() > 0 && row.height() > 0);
        row.is_some()
    });
    let row = row.unwrap();
    let label = row.child().unwrap();
    let bounds = label.compute_bounds(&row).unwrap();
    (row.measure(gtk::Orientation::Vertical, row.width()).1, bounds.x(), bounds.y())
}

fn picker_accent(parent: &gtk::Widget) -> gtk::gdk::RGBA {
    let popup = work_popup(parent, "Scheduled date");
    let entry = descendants(&popup)
        .into_iter()
        .find_map(|widget| widget.downcast::<gtk::Entry>().ok())
        .unwrap();
    let probe = gtk::Label::new(Some("Accent"));
    probe.add_css_class("zib-test-accent");
    entry.parent().unwrap().downcast::<gtk::Box>().unwrap().append(&probe);
    until(|| probe.is_mapped());
    let color = probe.color();
    popup.popdown();
    until(|| !popup.is_mapped());
    color
}

fn click_calendar(calendar: &gtk::Calendar, target: &gtk::Widget) {
    until(|| target.width() > 0 && target.height() > 0);
    let bounds = target.compute_bounds(calendar).unwrap();
    let x = f64::from(bounds.x() + bounds.width() / 2.0);
    let y = f64::from(bounds.y() + bounds.height() / 2.0);
    let model = calendar.observe_controllers();
    let gestures: Vec<_> = (0..model.n_items())
        .filter_map(|i| model.item(i)?.downcast::<gtk::GestureClick>().ok())
        .collect();
    // Dispatch capture before GTK's native selection handler, as a real pointer event does.
    for phase in [gtk::PropagationPhase::Capture, gtk::PropagationPhase::Bubble] {
        for gesture in gestures.iter().filter(|gesture| gesture.propagation_phase() == phase) {
            gesture.emit_by_name::<()>("pressed", &[&1i32, &x, &y]);
        }
    }
    for gesture in &gestures {
        gesture.emit_by_name::<()>("released", &[&1i32, &x, &y]);
    }
}

fn menu_item(popup: &gtk::Popover, label: &str) -> Option<gtk::Widget> {
    descendants(popup).into_iter().find(|widget| {
        widget.accessible_role() == gtk::AccessibleRole::MenuItem
            && widget.is_mapped()
            && descendants(widget).iter().any(|child| {
                child.downcast_ref::<gtk::Label>().is_some_and(|text| text.text() == label)
            })
    })
}

fn subproject_row(window: &Window, title: &str) -> Option<gtk::Widget> {
    task_row(window, title)
}

#[test]
fn subproject_drag_targets_reorder_siblings_without_changing_parents() {
    init_gtk();
    let runtime = crate::app::test_runtime();
    let root = tempfile::tempdir().unwrap();
    let state = runtime.block_on(State::for_test(Dirs::under(root.path()))).unwrap();
    let (space, project, a, b, c, task, child) = runtime.block_on(async {
        let space = state.spaces().await[0].0;
        let project = state.create_work(space, "project", "Root", None).await.unwrap();
        let a = state.create_work(space, "project", "Alpha", Some(project)).await.unwrap();
        let task = state.create_work(space, "task", "Task", Some(project)).await.unwrap();
        let b = state.create_work(space, "project", "Beta", Some(project)).await.unwrap();
        let c = state.create_work(space, "project", "Charlie", Some(project)).await.unwrap();
        let child = state.create_work(space, "task", "Subtask", Some(c)).await.unwrap();
        (space, project, a, b, c, task, child)
    });
    let app = adw::Application::builder()
        .application_id("systems.thoughtfull.Zibaldone.ProjectDragTest")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    let window = Window::new(&app, state.clone());
    present(&window.inner().window.clone().upcast());
    window.open_note(space, project);
    until(|| subproject_row(&window, "Charlie").is_some());
    let projects = || {
        let tree = window.inner().work.tree.borrow();
        tree.children(project)
            .into_iter()
            .filter(|id| tree.items[id].note.type_name == "project")
            .collect::<Vec<_>>()
    };
    let drop_on = |source, target, fraction| {
        drop_work_row(
            &subproject_row(&window, source).unwrap(),
            &subproject_row(&window, target).unwrap(),
            fraction,
        )
    };
    assert!(drop_on("Charlie", "Alpha", 0.4));
    until(|| projects() == [c, a, b]);
    assert!(drop_on("Charlie", "Beta", 0.6));
    until(|| projects() == [a, b, c]);
    assert!(!drop_on("Alpha", "Alpha", 0.5));
    let task_row = task_row(&window, "Task").unwrap();
    let project_row = subproject_row(&window, "Alpha").unwrap();
    assert!(!drop_work_row(&task_row, &project_row, 0.4));
    assert!(!drop_work_row(&project_row, &task_row, 0.4));
    let tree = runtime.block_on(state.work_tree(space)).unwrap();
    assert_eq!(tree.children(c), [child]);
    for id in [a, b, c, task] {
        assert_eq!(tree.items[&id].parent, Some(project));
    }
    assert_eq!(window.inner().editor.borrow().as_ref().unwrap().note, project);
    let button = descendants(&subproject_row(&window, "Beta").unwrap())
        .into_iter()
        .filter_map(|widget| widget.downcast::<gtk::Button>().ok())
        .find(|button| button.tooltip_text().as_deref() == Some("Open subproject"))
        .unwrap();
    button.emit_clicked();
    until(|| window.inner().editor.borrow().as_ref().is_some_and(|e| e.note == b));
    window.inner().window.destroy();
}

fn drop_work_row(source_row: &gtk::Widget, target_row: &gtk::Widget, fraction: f64) -> bool {
    let source = descendants(source_row)
        .iter()
        .find_map(|widget| {
            let controllers = widget.observe_controllers();
            (0..controllers.n_items())
                .find_map(|i| controllers.item(i)?.downcast::<gtk::DragSource>().ok())
        })
        .unwrap();
    let controllers = target_row.observe_controllers();
    let target = (0..controllers.n_items())
        .find_map(|i| controllers.item(i)?.downcast::<gtk::DropTarget>().ok())
        .unwrap();
    let content = source
        .emit_by_name::<Option<gdk::ContentProvider>>("prepare", &[&0.0_f64, &0.0_f64])
        .unwrap();
    let value = glib::BoxedValue(content.value(target.types()[0]).unwrap());
    let y = f64::from(target_row.height().max(1)) * fraction;
    target.emit_by_name::<bool>("drop", &[&value, &0.0_f64, &y])
}

#[test]
fn task_drag_targets_reorder_nest_and_reject_descendant_drops() {
    init_gtk();
    let runtime = crate::app::test_runtime();
    let root = tempfile::tempdir().unwrap();
    let state = runtime.block_on(State::for_test(Dirs::under(root.path()))).unwrap();
    let (space, project, a, b, c, child) = runtime.block_on(async {
        let space = state.spaces().await[0].0;
        let project = state.create_work(space, "project", "Project", None).await.unwrap();
        let a = state.create_work(space, "task", "Alpha", Some(project)).await.unwrap();
        let b = state.create_work(space, "task", "Beta", Some(project)).await.unwrap();
        let c = state.create_work(space, "task", "Charlie", Some(project)).await.unwrap();
        let child = state.create_work(space, "task", "Subtask", Some(a)).await.unwrap();
        (space, project, a, b, c, child)
    });
    let app = adw::Application::builder()
        .application_id("systems.thoughtfull.Zibaldone.TaskDragTest")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    let window = Window::new(&app, state.clone());
    present(&window.inner().window.clone().upcast());
    window.open_note(space, project);
    until(|| task_title(&window, "Charlie").is_some());

    let drop_on = |source: &str, destination: &str, fraction: f64| {
        let row = task_row(&window, source).unwrap();
        let target = task_row(&window, destination).unwrap();
        drop_work_row(&row, &target, fraction)
    };

    assert!(drop_on("Charlie", "Alpha", 0.0));
    until(|| window.inner().work.tree.borrow().children(project) == [c, a, b]);
    assert!(drop_on("Beta", "Alpha", 0.5));
    until(|| window.inner().work.tree.borrow().items[&b].parent == Some(a));
    until(|| task_title(&window, "Subtask").is_some());
    assert_eq!(window.inner().work.tree.borrow().children(a), [child, b]);
    assert!(!drop_on("Alpha", "Beta", 0.5));
    assert!(!drop_on("Alpha", "Alpha", 0.0));
    assert!(drop_on("Beta", "Charlie", 1.0));
    until(|| window.inner().work.tree.borrow().children(project) == [c, b, a]);
    let tree = runtime.block_on(state.work_tree(space)).unwrap();
    assert_eq!(tree.children(project), [c, b, a]);
    assert_eq!(tree.children(a), [child]);
    assert_eq!(tree.items[&b].note.body, "Beta");
    window.inner().window.destroy();
}

#[test]
fn task_titles_are_compact_preserve_notes_and_flush_pending_edits() {
    init_gtk();
    let runtime = crate::app::test_runtime();
    let root = tempfile::tempdir().unwrap();
    let dirs = Dirs::under(root.path());
    let state = runtime.block_on(State::for_test(dirs.clone())).unwrap();
    let (space, project, task) = runtime.block_on(async {
        let space = state.spaces().await[0].0;
        let project = state.create_work(space, "project", "Project design", None).await.unwrap();
        let task = state
            .create_work(space, "task", "# Task\n\nKeep these notes.\n", Some(project))
            .await
            .unwrap();
        (space, project, task)
    });
    let app = adw::Application::builder()
        .application_id("systems.thoughtfull.Zibaldone.CompactTest")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    let window = Window::new(&app, state.clone());
    present(&window.inner().window.clone().upcast());
    window.open_note(space, task);
    until(|| task_title(&window, "Task").is_some());
    let row = task_row(&window, "Task").unwrap();
    assert!(row.measure(gtk::Orientation::Vertical, 700).1 <= 64);
    assert_eq!(window.inner().editor.borrow().as_ref().unwrap().note, project);
    assert!(!descendants(&window.inner().work.widget).iter().any(|w| w.is::<EditorWidget>()));
    let entry = edit_title(&window, "Task");
    entry.grab_focus_without_selecting();
    entry.set_text("Updated task");
    entry.emit_by_name::<()>("activate", &[]);
    entry.set_text("Latest task");
    entry.set_position(3);
    let flushed = Rc::new(Cell::new(None));
    let (w, result) = (window.clone(), flushed.clone());
    glib::spawn_future_local(async move {
        result.set(Some(w.finish_task_titles().await));
    });
    until(|| flushed.get().is_some());
    assert_eq!(flushed.get(), Some(true));
    until(|| task_title(&window, "Latest task").is_some());
    assert!(gtk::prelude::RootExt::focus(&window.inner().window).unwrap().is_ancestor(&entry));
    assert_eq!(entry.position(), 3);
    assert_eq!(
        runtime.block_on(state.work_tree(space)).unwrap().items[&task].note.body,
        "# Latest task\n\nKeep these notes.\n"
    );
    window.inner().search.buffer().set_text(zib_core::work::WORK_QUERY);
    until(|| window.inner().rows.borrow().iter().any(|(_, id)| *id == task));
    let latest_check = || {
        descendants(&window.inner().work.widget)
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::CheckButton>().ok())
            .find(|c| c.tooltip_text().as_deref() == Some("Complete Latest task"))
    };
    until(|| latest_check().is_some());
    let check = latest_check().unwrap();
    check.set_active(true);
    until(|| !window.inner().rows.borrow().iter().any(|(_, id)| *id == task));
    assert_eq!(
        runtime.block_on(state.work_tree(space)).unwrap().items[&task]
            .explicit_date("completed-on"),
        Some(zib_core::work::today())
    );
    assert!(task_title(&window, "Latest task").is_some());
    window.close_editor();
    runtime.block_on(state.finish());
    window.inner().window.destroy();
    let reopened = runtime.block_on(State::for_test(dirs)).unwrap();
    assert_eq!(
        runtime.block_on(reopened.work_tree(space)).unwrap().items[&task].title,
        "Latest task"
    );
    runtime.block_on(reopened.finish());
}

#[test]
fn show_completed_is_saved_per_work_note_and_restored_after_restart() {
    init_gtk();
    let runtime = crate::app::test_runtime();
    let root = tempfile::tempdir().unwrap();
    let dirs = Dirs::under(root.path());
    let state = runtime.block_on(State::for_test(dirs.clone())).unwrap();
    let (space, project, subproject, standalone) = runtime.block_on(async {
        let space = state.spaces().await[0].0;
        let project = state.create_work(space, "project", "First project", None).await.unwrap();
        let subproject =
            state.create_work(space, "project", "Second project", Some(project)).await.unwrap();
        let standalone = state.create_work(space, "task", "Standalone", None).await.unwrap();
        for (parent, suffix) in [(project, "A"), (subproject, "B"), (standalone, "C")] {
            state
                .create_work(space, "task", &format!("Active {suffix}"), Some(parent))
                .await
                .unwrap();
            let done = state
                .create_work(space, "task", &format!("Done {suffix}"), Some(parent))
                .await
                .unwrap();
            state
                .set_work_field(
                    space,
                    done,
                    "completed-on",
                    Some(Value::Date(zib_core::work::today())),
                )
                .await
                .unwrap();
        }
        (space, project, subproject, standalone)
    });
    let before = [project, subproject, standalone]
        .map(|note| runtime.block_on(state.note(space, note)).unwrap());
    let app = adw::Application::builder()
        .application_id("systems.thoughtfull.Zibaldone.CompletedPreferenceTest")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    let window = Window::new(&app, state.clone());
    present(&window.inner().window.clone().upcast());
    let check = |window: &Window| {
        descendants(&window.inner().work.widget)
            .into_iter()
            .filter_map(|widget| widget.downcast::<gtk::CheckButton>().ok())
            .find(|button| button.label().as_deref() == Some("Show completed"))
            .unwrap()
    };
    let open = |window: &Window, note, suffix| {
        window.open_note(space, note);
        until(|| task_title(window, &format!("Active {suffix}")).is_some());
    };
    open(&window, project, "A");
    assert!(
        descendants(&window.inner().work.header)
            .iter()
            .all(|widget| { widget.tooltip_text().as_deref() != Some("Project actions") })
    );
    assert!(!check(&window).is_active());
    assert!(task_title(&window, "Done A").is_none());
    check(&window).set_active(true);
    until(|| task_title(&window, "Done A").is_some());
    assert!(runtime.block_on(state.show_completed(space, project)).unwrap());
    open(&window, subproject, "B");
    assert!(!check(&window).is_active(), "subprojects don't inherit this preference");
    assert!(task_title(&window, "Done B").is_none());
    check(&window).set_active(true);
    until(|| task_title(&window, "Done B").is_some());
    check(&window).set_active(false);
    until(|| task_title(&window, "Done B").is_none());
    assert!(!runtime.block_on(state.show_completed(space, subproject)).unwrap());
    runtime.block_on(state.set_show_completed(space, subproject, true)).unwrap();
    window.space_changed(space);
    until(|| check(&window).is_active() && task_title(&window, "Done B").is_some());
    runtime.block_on(state.set_show_completed(space, subproject, false)).unwrap();
    window.space_changed(space);
    until(|| !check(&window).is_active() && task_title(&window, "Done B").is_none());
    open(&window, project, "A");
    assert!(check(&window).is_active());
    assert!(task_title(&window, "Done A").is_some());
    open(&window, standalone, "C");
    assert!(!check(&window).is_active());
    let inner = window.inner();
    assert!(inner.project_pane.scroll.is_visible());
    assert!(inner.work.widget.is_ancestor(&inner.project_pane.scroll));
    assert!(descendants(&inner.work.widget).iter().any(|widget| {
        widget.downcast_ref::<gtk::Label>().is_some_and(|label| label.text() == "Subtasks")
    }));
    check(&window).set_active(true);
    // Navigation mustn't redirect a pending setting write into the newly opened note.
    open(&window, subproject, "B");
    until(|| runtime.block_on(state.show_completed(space, standalone)).unwrap());
    assert!(!check(&window).is_active());
    for note in &before {
        assert_eq!(runtime.block_on(state.note(space, note.id)).unwrap(), *note);
    }
    open(&window, standalone, "C");
    let header = &window.inner().work.header;
    for name in ["Due date", "Move…", "Scheduled date", "Work status"] {
        assert!(descendants(header).iter().any(|widget| {
            widget.tooltip_text().is_some_and(|tooltip| tooltip.starts_with(name))
        }));
    }
    assert!(
        descendants(header)
            .iter()
            .all(|widget| { widget.tooltip_text().as_deref() != Some("Task actions") })
    );
    let grid = header.first_child().and_downcast::<gtk::Grid>().unwrap();
    for (row, name) in
        (0..).zip(["Completed", "Status", "Scheduled date", "Due date", "Parent", "Recurrence"])
    {
        let label = grid.child_at(0, row).and_downcast::<gtk::Label>().unwrap();
        assert_eq!(label.text(), name);
        assert!(label.xalign() > 0.99);
        assert!(label.has_css_class("zib-work-field-name") && !label.has_css_class("caption"));
        assert!(grid.child_at(1, row).is_some());
    }
    let control_labels: Vec<_> = (1..=4)
        .map(|row| {
            descendants(&grid.child_at(1, row).unwrap())
                .into_iter()
                .find_map(|widget| widget.downcast::<gtk::Label>().ok())
                .unwrap()
        })
        .collect();
    until(|| control_labels.iter().all(|label| label.width() > 0));
    let left = control_labels[0].compute_bounds(&grid).unwrap().x();
    for label in control_labels {
        assert!(!label.has_css_class("caption"));
        assert!((label.compute_bounds(&grid).unwrap().x() - left).abs() < 1.0);
        if let Some(button) =
            label.ancestor(gtk::MenuButton::static_type()).and_downcast::<gtk::MenuButton>()
        {
            assert!(!button.has_css_class("flat"));
            assert!(!button.has_css_class("zib-work-quiet"));
            assert!(left - button.compute_bounds(&grid).unwrap().x() >= 12.0);
            let weight = label.pango_context().font_description().unwrap().weight();
            assert_eq!(weight, gtk::pango::Weight::Bold);
        }
    }
    let complete = grid
        .child_at(1, 0)
        .and_then(|cell| cell.first_child())
        .and_downcast::<gtk::CheckButton>()
        .unwrap();
    complete.set_active(true);
    until(|| window.inner().work.tree.borrow().items[&standalone].completed());
    window.close_editor();
    runtime.block_on(state.finish());
    window.inner().window.destroy();
    let reopened = runtime.block_on(State::for_test(dirs)).unwrap();
    let next = Window::new(&app, reopened.clone());
    present(&next.inner().window.clone().upcast());
    for (note, suffix, visible) in
        [(project, "A", true), (subproject, "B", false), (standalone, "C", true)]
    {
        open(&next, note, suffix);
        assert_eq!(check(&next).is_active(), visible);
        assert_eq!(task_title(&next, &format!("Done {suffix}")).is_some(), visible);
    }
    next.close_editor();
    runtime.block_on(reopened.finish());
    next.inner().window.destroy();
}

#[test]
fn task_dates_show_only_effective_values_and_add_from_the_actions_menu() {
    init_gtk();
    let runtime = crate::app::test_runtime();
    let root = tempfile::tempdir().unwrap();
    let state = runtime.block_on(State::for_test(Dirs::under(root.path()))).unwrap();
    let (space, project, task) = runtime.block_on(async {
        let space = state.spaces().await[0].0;
        let project = state.create_work(space, "project", "Project", None).await.unwrap();
        let task = state.create_work(space, "task", "Task", Some(project)).await.unwrap();
        (space, project, task)
    });
    let app = adw::Application::builder()
        .application_id("systems.thoughtfull.Zibaldone.OptionalTaskDatesTest")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    let window = Window::new(&app, state.clone());
    present(&window.inner().window.clone().upcast());
    window.open_note(space, project);
    until(|| task_title(&window, "Task").is_some());
    let row = || task_row(&window, "Task").unwrap();
    let shows = |prefix: &str| {
        descendants(&row()).iter().any(|widget| {
            widget.downcast_ref::<gtk::MenuButton>().is_some_and(|button| {
                button.tooltip_text().is_some_and(|text| text.starts_with(prefix))
            })
        })
    };
    let button = |popup: &gtk::Popover, label: &str| {
        descendants(popup)
            .into_iter()
            .filter_map(|widget| widget.downcast::<gtk::Button>().ok())
            .find(|button| button.label().as_deref() == Some(label))
    };
    assert!(!shows("Due date") && !shows("Scheduled date"));
    // Exercise each date independently so neither leaves an empty control behind.
    for (action, prefix, field, date) in [
        ("Add schedule date", "Scheduled date", "scheduled-on", "2026-11-20"),
        ("Add due date", "Due date", "due-on", "2026-11-30"),
    ] {
        let popup = work_popup(&row(), "Task actions");
        assert!(menu_item(&popup, action).unwrap().activate());
        let entry = descendants(&popup)
            .into_iter()
            .find_map(|widget| widget.downcast::<gtk::Entry>().ok())
            .unwrap();
        until(|| {
            gtk::prelude::RootExt::focus(&window.inner().window)
                .is_some_and(|focus| focus == entry || focus.is_ancestor(&entry))
        });
        entry.set_text(date);
        entry.emit_by_name::<()>("activate", &[]);
        until(|| shows(prefix));
        assert_eq!(window.inner().work.tree.borrow().date(task, field).unwrap().value, date);
        let popup = work_popup(&row(), "Task actions");
        assert!(menu_item(&popup, action).is_none());
        popup.popdown();
        until(|| !popup.is_mapped());
        let popup = work_popup(&row(), prefix);
        button(&popup, "Inherited").unwrap().emit_clicked();
        until(|| !shows(prefix));
        assert!(window.inner().work.tree.borrow().date(task, field).is_none());
        let popup = work_popup(&row(), "Task actions");
        assert!(menu_item(&popup, action).is_some());
        popup.popdown();
        until(|| !popup.is_mapped());
    }
    for (field, date) in [("due-on", "2026-12-10"), ("scheduled-on", "2026-12-01")] {
        runtime
            .block_on(state.set_work_field(space, project, field, Some(Value::Date(date.into()))))
            .unwrap();
    }
    window.space_changed(space);
    until(|| shows("Due date") && shows("Scheduled date"));
    let popup = work_popup(&row(), "Task actions");
    assert!(menu_item(&popup, "Add due date").is_none());
    assert!(menu_item(&popup, "Add schedule date").is_none());
    popup.popdown();
    until(|| !popup.is_mapped());
    let popup = work_popup(&row(), "Due date");
    assert!(!button(&popup, "Inherited").unwrap().is_sensitive());
    let entry = descendants(&popup)
        .into_iter()
        .find_map(|widget| widget.downcast::<gtk::Entry>().ok())
        .unwrap();
    entry.set_text("2026-12-12");
    entry.emit_by_name::<()>("activate", &[]);
    until(|| window.inner().work.tree.borrow().date(task, "due-on").unwrap().source == task);
    let popup = work_popup(&row(), "Due date");
    button(&popup, "Inherited").unwrap().emit_clicked();
    until(|| window.inner().work.tree.borrow().date(task, "due-on").unwrap().source == project);
    assert!(shows("Due date"));
    runtime.block_on(state.set_work_field(space, project, "due-on", None)).unwrap();
    window.space_changed(space);
    until(|| !shows("Due date"));
    assert!(shows("Scheduled date"));
    window.inner().window.destroy();
}

#[test]
fn calendar_popovers_sync_typed_dates_selection_and_clear_to_inherit() {
    init_gtk();
    let runtime = crate::app::test_runtime();
    let root = tempfile::tempdir().unwrap();
    let state = runtime.block_on(State::for_test(Dirs::under(root.path()))).unwrap();
    let (space, project, task) = runtime.block_on(async {
        let space = state.spaces().await[0].0;
        let project = state.create_work(space, "project", "Project", None).await.unwrap();
        let task = state.create_work(space, "task", "Task", Some(project)).await.unwrap();
        state
            .set_work_field(space, project, "due-on", Some(Value::Date("2026-11-30".into())))
            .await
            .unwrap();
        (space, project, task)
    });
    let app = adw::Application::builder()
        .application_id("systems.thoughtfull.Zibaldone.CalendarTest")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    let window = Window::new(&app, state.clone());
    present(&window.inner().window.clone().upcast());
    window.open_note(space, task);
    until(|| task_title(&window, "Task").is_some());
    let popup = work_popup(&window.inner().work.widget, "Due date");
    let widgets = descendants(&popup);
    let entry = widgets.iter().find_map(|w| w.clone().downcast::<gtk::Entry>().ok()).unwrap();
    let calendar = widgets.iter().find_map(|w| w.clone().downcast::<gtk::Calendar>().ok()).unwrap();
    assert!(!widgets.iter().any(|widget| {
        widget
            .downcast_ref::<gtk::Button>()
            .is_some_and(|button| button.label().as_deref() == Some("Apply"))
    }));
    let inherited = widgets
        .iter()
        .find_map(|widget| {
            widget
                .downcast_ref::<gtk::Button>()
                .filter(|button| button.label().as_deref() == Some("Inherited"))
        })
        .unwrap();
    assert_eq!(entry.parent(), inherited.parent());
    until(|| entry.width() > 0 && inherited.width() > 0);
    let row = entry.parent().unwrap();
    let input_bounds = entry.compute_bounds(&row).unwrap();
    let button_bounds = inherited.compute_bounds(&row).unwrap();
    assert!(button_bounds.x() >= input_bounds.x() + input_bounds.width());
    assert!(!widgets.iter().any(|widget| {
        widget.downcast_ref::<gtk::Label>().is_some_and(|label| label.text().contains("Clearing"))
    }));
    entry.set_text("2026-02-30");
    window.space_changed(space);
    assert!(popup.is_visible());
    assert_eq!(entry.text(), "2026-02-30");
    entry.emit_activate();
    assert!(popup.is_visible());
    assert!(entry.has_css_class("error"));
    assert_eq!(window.inner().work.tree.borrow().date(task, "due-on").unwrap().source, project);
    entry.set_text("2026-12-02");
    assert!(!entry.has_css_class("error"));
    assert_eq!(calendar.date().format("%Y-%m-%d").unwrap(), "2026-12-02");
    assert!(popup.is_visible(), "typing must not submit the date");
    let day = descendants(&calendar)
        .into_iter()
        .find(|widget| {
            widget.has_css_class("day-number")
                && !widget.has_css_class("other-month")
                && widget.downcast_ref::<gtk::Label>().is_some_and(|label| label.text() == "3")
        })
        .unwrap();
    click_calendar(&calendar, &day);
    until(|| !popup.is_mapped());
    until(|| {
        window
            .inner()
            .work
            .tree
            .borrow()
            .date(task, "due-on")
            .is_some_and(|d| d.source == task && d.value == "2026-12-03")
    });
    let popup = work_popup(&window.inner().work.widget, "Due date");
    let clear = descendants(&popup)
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Button>().ok())
        .find(|b| b.label().as_deref() == Some("Inherited"))
        .unwrap();
    clear.emit_clicked();
    until(|| {
        window.inner().work.tree.borrow().date(task, "due-on").is_some_and(|d| d.source == project)
    });
    let popup = work_popup(&window.inner().work.header, "Scheduled date");
    let calendar = descendants(&popup)
        .into_iter()
        .find_map(|widget| widget.downcast::<gtk::Calendar>().ok())
        .unwrap();
    let today = calendar.date().format("%Y-%m-%d").unwrap().to_string();
    let selected = descendants(&calendar)
        .into_iter()
        .find(|widget| widget.has_css_class("day-number") && widget.has_css_class("today"))
        .unwrap();
    click_calendar(&calendar, &selected);
    until(|| !popup.is_mapped());
    until(|| {
        window
            .inner()
            .work
            .tree
            .borrow()
            .date(project, "scheduled-on")
            .is_some_and(|date| date.value == today)
    });
    let popup = work_popup(&window.inner().work.header, "Scheduled date");
    let entry =
        descendants(&popup).into_iter().find_map(|w| w.downcast::<gtk::Entry>().ok()).unwrap();
    entry.set_text("2026-11-20");
    entry.emit_by_name::<()>("activate", &[]);
    until(|| {
        window
            .inner()
            .work
            .tree
            .borrow()
            .date(task, "scheduled-on")
            .is_some_and(|d| d.source == project && d.value == "2026-11-20")
    });
    window.close_editor();
    runtime.block_on(state.finish());
    window.inner().window.destroy();
}

#[test]
fn work_status_menus_override_inherit_and_complete_previous_waiting_values() {
    init_gtk();
    let runtime = crate::app::test_runtime();
    let root = tempfile::tempdir().unwrap();
    let dirs = Dirs::under(root.path());
    let state = runtime.block_on(State::for_test(dirs.clone())).unwrap();
    let (space, project, task, subtask) = runtime.block_on(async {
        let space = state.spaces().await[0].0;
        let project = state.create_work(space, "project", "Project", None).await.unwrap();
        let task = state.create_work(space, "task", "Task", Some(project)).await.unwrap();
        let subtask = state.create_work(space, "task", "Subtask", Some(task)).await.unwrap();
        let other = state.create_space("Other").await.unwrap();
        let old = state.create_work(other, "task", "Old", None).await.unwrap();
        state
            .set_work_status(other, old, Some("waiting".into()), "Alex reply".into())
            .await
            .unwrap();
        state.delete_note(other, old).await.unwrap();
        (space, project, task, subtask)
    });
    let app = adw::Application::builder()
        .application_id("systems.thoughtfull.Zibaldone.WorkStatusTest")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    let window = Window::new(&app, state.clone());
    present(&window.inner().window.clone().upcast());
    window.open_note(space, subtask);
    until(|| task_title(&window, "Subtask").is_some());
    window.inner().search.buffer().set_text(zib_core::work::WORK_QUERY);
    until(|| window.inner().rows.borrow().iter().any(|(_, id)| *id == task));
    let task_status = || {
        let row = task_row(&window, "Task").unwrap();
        let popup = work_popup(&row, "Task actions");
        assert!(menu_item(&popup, "Set status…").is_none());
        until(|| menu_item(&popup, "Waiting…").is_some());
        popup
    };
    for label in ["Maybe", "Someday"] {
        let popup = task_status();
        assert!(menu_item(&popup, "Clear").is_none());
        assert!(menu_item(&popup, "None").is_none());
        assert!(menu_item(&popup, label).unwrap().activate());
        until(|| {
            window
                .inner()
                .work
                .tree
                .borrow()
                .status(task)
                .is_some_and(|status| status.label() == label)
        });
        until(|| !window.inner().rows.borrow().iter().any(|(_, id)| [subtask, task].contains(id)));
        let popup = task_status();
        assert!(menu_item(&popup, label).is_none());
        assert!(menu_item(&popup, "Inherited").unwrap().activate());
        until(|| window.inner().work.tree.borrow().resolved_status(task).is_none());
    }
    let popup = task_status();
    assert!(menu_item(&popup, "Someday").unwrap().activate());
    until(|| window.inner().work.tree.borrow().status(task).is_some());
    let inherited = work_popup(&task_row(&window, "Subtask").unwrap(), "Work status");
    for label in ["Inherited", "Maybe", "Clear", "Someday", "Waiting…"] {
        assert!(menu_item(&inherited, label).is_some());
    }
    assert!(menu_item(&inherited, "Clear").unwrap().activate());
    until(|| window.inner().work.tree.borrow().status(subtask).is_none());
    until(|| window.inner().rows.borrow().iter().any(|(_, id)| *id == subtask));
    assert!(!descendants(&task_row(&window, "Subtask").unwrap()).iter().any(|widget| {
        widget.tooltip_text().is_some_and(|text| text.starts_with("Work status"))
    }));
    let index = window.inner().rows.borrow().iter().position(|(_, id)| *id == subtask).unwrap();
    let result = window.inner().list.row_at_index(i32::try_from(index).unwrap()).unwrap();
    assert!(!descendants(&result).iter().any(|widget| {
        widget.tooltip_text().is_some_and(|text| text.starts_with("Work status"))
            || widget.downcast_ref::<gtk::Label>().is_some_and(|label| label.text() == "None")
    }));
    let popup = work_popup(&task_row(&window, "Subtask").unwrap(), "Task actions");
    assert!(menu_item(&popup, "Clear").is_none());
    assert!(menu_item(&popup, "Inherited").unwrap().activate());
    until(|| window.inner().work.tree.borrow().status(subtask).is_some());
    let popup = task_status();
    assert!(menu_item(&popup, "Someday").is_none());
    assert!(menu_item(&popup, "Inherited").unwrap().activate());
    until(|| window.inner().work.tree.borrow().resolved_status(task).is_none());
    let popup = work_popup(&window.inner().work.header, "Work status");
    assert!(menu_item(&popup, "Waiting…").unwrap().activate());
    let entry = descendants(&popup)
        .into_iter()
        .find_map(|widget| widget.downcast::<gtk::Entry>().ok())
        .unwrap();
    until(|| {
        gtk::prelude::RootExt::focus(&window.inner().window)
            .is_some_and(|focus| focus == entry || focus.is_ancestor(&entry))
    });
    entry.set_text("aLe");
    until(|| {
        descendants(&popup).iter().any(|widget| {
            widget
                .downcast_ref::<gtk::Label>()
                .is_some_and(|label| label.text() == "Alex reply" && label.is_visible())
        })
    });
    let controllers = entry.observe_controllers();
    let keys = (0..controllers.n_items())
        .find_map(|i| controllers.item(i)?.downcast::<gtk::EventControllerKey>().ok())
        .unwrap();
    assert!(keys.emit_by_name::<bool>(
        "key-pressed",
        &[&gdk::Key::Tab, &0u32, &gdk::ModifierType::empty()]
    ));
    assert_eq!(entry.text(), "Alex reply");
    entry.emit_by_name::<()>("activate", &[]);
    until(|| {
        window
            .inner()
            .work
            .tree
            .borrow()
            .status(task)
            .is_some_and(|status| status.source == project)
    });
    let status = window.inner().work.tree.borrow().status(subtask).unwrap();
    assert_eq!(status.waiting_for.as_deref(), Some("Alex reply"));
    let popup = task_status();
    assert!(menu_item(&popup, "Maybe").unwrap().activate());
    until(|| {
        window.inner().work.tree.borrow().status(task).is_some_and(|status| status.value == "maybe")
    });
    assert_eq!(window.inner().work.tree.borrow().status(subtask).unwrap().waiting_for, None);
    let popup = work_popup(&window.inner().work.header, "Work status");
    assert!(menu_item(&popup, "Clear").is_none());
    assert!(menu_item(&popup, "Inherited").unwrap().activate());
    until(|| window.inner().work.tree.borrow().status(project).is_none());
    assert_eq!(window.inner().work.tree.borrow().status(task).unwrap().value, "maybe");
    let popup = task_status();
    assert!(menu_item(&popup, "Inherited").unwrap().activate());
    until(|| window.inner().rows.borrow().iter().any(|(_, id)| *id == task));
    let popup = task_status();
    assert!(menu_item(&popup, "Waiting…").unwrap().activate());
    let entry = descendants(&popup)
        .into_iter()
        .find_map(|widget| widget.downcast::<gtk::Entry>().ok())
        .unwrap();
    entry.set_text("New reason");
    entry.emit_by_name::<()>("activate", &[]);
    until(|| {
        window
            .inner()
            .work
            .tree
            .borrow()
            .status(task)
            .is_some_and(|status| status.waiting_for.as_deref() == Some("New reason"))
    });
    let popup = task_status();
    assert!(menu_item(&popup, "Waiting…").unwrap().activate());
    let entry = descendants(&popup)
        .into_iter()
        .find_map(|widget| widget.downcast::<gtk::Entry>().ok())
        .unwrap();
    assert_eq!(entry.text(), "New reason");
    let back = menu_item(&popup, "Waiting…").unwrap();
    assert!(back.grab_focus());
    until(|| {
        gtk::prelude::RootExt::focus(&window.inner().window)
            .is_some_and(|focus| focus == entry || focus.is_ancestor(&entry))
    });
    entry.set_text("aLe");
    let list = descendants(&popup)
        .into_iter()
        .find_map(|widget| widget.downcast::<gtk::ListBox>().ok())
        .unwrap();
    until(|| list.row_at_index(0).is_some_and(|row| row.is_mapped()));
    let selected = list.row_at_index(0).unwrap();
    assert_eq!(selected.child().and_downcast::<gtk::Label>().unwrap().text(), "Alex reply");
    assert!(list.activates_on_single_click());
    list.emit_by_name::<()>("row-activated", &[&selected]);
    until(|| {
        window
            .inner()
            .work
            .tree
            .borrow()
            .status(task)
            .is_some_and(|status| status.waiting_for.as_deref() == Some("Alex reply"))
    });
    until(|| !popup.is_mapped());
    runtime
        .block_on(state.set_work_status(space, project, Some("none".into()), String::new()))
        .unwrap();
    window.space_changed(space);
    until(|| {
        window.inner().editor.borrow().as_ref().unwrap().fields.iter().any(|(name, value)| {
            name == "status" && matches!(value, Value::Choice(status) if status == "none")
        })
    });
    until(|| {
        window
            .inner()
            .work
            .tree
            .borrow()
            .resolved_status(project)
            .is_some_and(|status| status.value == "none")
    });
    let metadata = window.inner().note_metadata.popover().unwrap();
    assert!(!descendants(&metadata).iter().any(|widget| {
        widget
            .downcast_ref::<gtk::Label>()
            .is_some_and(|label| matches!(label.text().as_str(), "none" | "None" | "status"))
    }));
    let popup = task_status();
    assert!(menu_item(&popup, "Clear").is_none());
    popup.popdown();
    until(|| !popup.is_mapped());
    window.close_editor();
    runtime.block_on(state.finish());
    window.inner().window.destroy();
    let reopened = runtime.block_on(State::for_test(dirs)).unwrap();
    assert_eq!(runtime.block_on(reopened.waiting_values()), ["Alex reply", "New reason"]);
    assert_eq!(
        runtime.block_on(reopened.work_tree(space)).unwrap().status(subtask).unwrap().value,
        "waiting"
    );
    runtime.block_on(reopened.finish());
}

#[test]
fn task_actions_choose_parent_without_an_inline_parent_selector() {
    init_gtk();
    let runtime = crate::app::test_runtime();
    let root = tempfile::tempdir().unwrap();
    let state = runtime.block_on(State::for_test(Dirs::under(root.path()))).unwrap();
    let (space, project, other, task, child) = runtime.block_on(async {
        let space = state.spaces().await[0].0;
        let project = state.create_work(space, "project", "Current", None).await.unwrap();
        let other = state.create_work(space, "project", "Destination", None).await.unwrap();
        let task = state.create_work(space, "task", "Task", Some(project)).await.unwrap();
        let child = state.create_work(space, "task", "Subtask", Some(task)).await.unwrap();
        state
            .set_work_status(space, other, Some("waiting".into()), "Awaiting reply".into())
            .await
            .unwrap();
        (space, project, other, task, child)
    });
    let app = adw::Application::builder()
        .application_id("systems.thoughtfull.Zibaldone.TaskParentMenuTest")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    let window = Window::new(&app, state.clone());
    present(&window.inner().window.clone().upcast());
    window.open_note(space, project);
    until(|| task_title(&window, "Task").is_some());
    let mut parent_metrics = None;
    for query in ["", "Destination"] {
        let popup = work_popup(&window.inner().work.header, "Move…");
        let search = descendants(&popup)
            .into_iter()
            .find_map(|widget| widget.downcast::<gtk::SearchEntry>().ok())
            .unwrap();
        parent_metrics.get_or_insert_with(|| picker_row_metrics(&popup));
        search.grab_focus();
        search.set_text(query);
        // SearchEntry consumes Escape and emits stop-search instead of passing it to the popup.
        search.emit_stop_search();
        until(|| !popup.is_mapped());
        assert_eq!(window.inner().work.popovers.get(), 0);
        assert_eq!(runtime.block_on(state.work_tree(space)).unwrap().items[&project].parent, None);
    }
    let row = task_row(&window, "Task").unwrap();
    assert!(
        !descendants(&row).iter().any(|widget| {
            widget.tooltip_text().is_some_and(|text| text.starts_with("Move…"))
        })
    );
    let popup = work_popup(&row, "Task actions");
    assert!(popup.is::<gtk::PopoverMenu>());
    assert!(menu_item(&popup, "Add subtask").is_none());
    let model = popup.downcast_ref::<gtk::PopoverMenu>().unwrap().menu_model().unwrap();
    let labels: Vec<_> = (0..model.n_items())
        .map(|index| {
            model.item_attribute_value(index, "label", None).unwrap().str().unwrap().to_owned()
        })
        .collect();
    assert_eq!(
        labels,
        [
            "Add schedule date",
            "Add due date",
            "Add recurrence…",
            "Someday",
            "Maybe",
            "Waiting…",
            "Move…",
            "Delete task…"
        ]
    );
    until(|| menu_item(&popup, "Waiting…").is_some());
    assert!(menu_item(&popup, "Waiting…").unwrap().activate());
    until(|| {
        descendants(&popup).iter().any(|widget| widget.is::<gtk::Entry>() && widget.is_mapped())
    });
    assert_eq!(picker_row_metrics(&popup), parent_metrics.unwrap());
    assert!(!descendants(&popup).iter().any(|widget| {
        widget
            .downcast_ref::<gtk::Button>()
            .is_some_and(|button| button.label().as_deref() == Some("Apply"))
    }));
    assert!(menu_item(&popup, "Waiting…").unwrap().activate());

    let choose = || {
        until(|| menu_item(&popup, "Move…").is_some());
        assert!(menu_item(&popup, "Move…").unwrap().activate());
        let back = menu_item(&popup, "Move…").unwrap();
        // Start with the native navigation row focused, before the opening frame settles.
        assert!(back.grab_focus());
    };
    choose();
    let search = descendants(&popup)
        .into_iter()
        .find_map(|widget| widget.downcast::<gtk::SearchEntry>().ok())
        .unwrap();
    let focused = || {
        gtk::prelude::RootExt::focus(&window.inner().window)
            .is_some_and(|focus| focus == search || focus.is_ancestor(&search))
    };
    until(focused);
    assert_eq!(picker_row_metrics(&popup), parent_metrics.unwrap());
    search.set_text("draft");
    assert!(menu_item(&popup, "Move…").unwrap().activate());
    until(|| !search.is_mapped());
    choose();
    until(focused);
    assert_eq!(search.text(), "draft");
    popup.popdown();
    until(|| !popup.is_mapped());
    let reopened = work_popup(&row, "Task actions");
    assert_eq!(reopened, popup);
    assert!(!search.is_mapped());
    choose();
    until(focused);
    let widgets = descendants(&popup);
    let search =
        widgets.iter().find_map(|widget| widget.downcast_ref::<gtk::SearchEntry>()).unwrap();
    search.set_text("dest");
    let list = widgets
        .iter()
        .find_map(|widget| widget.downcast_ref::<gtk::ListBox>().filter(|list| list.is_mapped()))
        .unwrap();
    until(|| !list.row_at_index(0).unwrap().is_visible());
    let destination = list.row_at_index(1).unwrap();
    assert!(destination.is_visible());
    list.emit_by_name::<()>("row-activated", &[&destination]);
    until(|| window.inner().editor.borrow().as_ref().is_some_and(|editor| editor.note == other));
    until(|| task_title(&window, "Task").is_some());
    let tree = runtime.block_on(state.work_tree(space)).unwrap();
    assert_eq!(tree.items[&task].parent, Some(other));
    assert_eq!(tree.items[&child].parent, Some(task));
    assert!(window.inner().window.visible_dialog().is_none());
    window.inner().window.destroy();
}

#[test]
fn deleting_work_cascades_from_task_menu_and_project_header_and_persists() {
    init_gtk();
    let runtime = crate::app::test_runtime();
    let root = tempfile::tempdir().unwrap();
    let dirs = Dirs::under(root.path());
    let state = runtime.block_on(State::for_test(dirs.clone())).unwrap();
    let (space, project, child, task, subtask, completed, nested_task, standalone) = runtime
        .block_on(async {
            let space = state.spaces().await[0].0;
            let project = state.create_work(space, "project", "Root", None).await.unwrap();
            let child =
                state.create_work(space, "project", "Subproject", Some(project)).await.unwrap();
            let task = state.create_work(space, "task", "Task", Some(project)).await.unwrap();
            let subtask = state.create_work(space, "task", "Subtask", Some(task)).await.unwrap();
            let completed =
                state.create_work(space, "task", "Completed", Some(task)).await.unwrap();
            state
                .set_work_field(
                    space,
                    completed,
                    "completed-on",
                    Some(Value::Date(zib_core::work::today())),
                )
                .await
                .unwrap();
            let nested_task =
                state.create_work(space, "task", "Nested task", Some(child)).await.unwrap();
            let standalone = state.create_work(space, "task", "Standalone", None).await.unwrap();
            (space, project, child, task, subtask, completed, nested_task, standalone)
        });
    let app = adw::Application::builder()
        .application_id("systems.thoughtfull.Zibaldone.WorkDeletionTest")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    let window = Window::new(&app, state.clone());
    present(&window.inner().window.clone().upcast());
    window.open_note(space, subtask);
    until(|| task_title(&window, "Subtask").is_some());
    let draft = edit_title(&window, "Subtask");
    draft.set_text("Unsaved subtask draft");
    assert!(task_title(&window, "Completed").is_none());
    let ask = || {
        let row = task_row(&window, "Task").unwrap();
        let popup = work_popup(&row, "Task actions");
        assert!(menu_item(&popup, "Delete task…").unwrap().activate());
    };
    let respond = |heading, detail: &str, response| {
        until(|| window.inner().window.visible_dialog().is_some());
        let dialog =
            window.inner().window.visible_dialog().unwrap().downcast::<adw::AlertDialog>().unwrap();
        assert_eq!(dialog.heading().as_deref(), Some(heading));
        assert!(dialog.body().contains(detail));
        dialog.emit_by_name::<()>("response", &[&response]);
        dialog.close();
        until(|| window.inner().window.visible_dialog().is_none());
    };
    ask();
    respond("Delete this task?", "all subtasks", "cancel");
    for id in [completed, subtask, task] {
        assert!(runtime.block_on(state.note(space, id)).is_ok());
    }
    // An invalid draft cannot autosave; deleting its ancestor must discard it too.
    draft.set_text("");
    ask();
    respond("Delete this task?", "all subtasks", "delete");
    until(|| task_title(&window, "Task").is_none());
    for id in [completed, subtask, task] {
        assert!(runtime.block_on(state.note(space, id)).is_err());
        assert!(
            state.search_shown(&[space], &format!("@id={id}"), Sort::Title, 20).unwrap().is_empty()
        );
    }
    assert_eq!(window.inner().editor.borrow().as_ref().unwrap().note, project);
    let flushed = Rc::new(Cell::new(None));
    let (w, result) = (window.clone(), flushed.clone());
    glib::spawn_future_local(async move {
        result.set(Some(w.finish_task_titles().await));
    });
    until(|| flushed.get().is_some());
    assert_eq!(flushed.get(), Some(true));
    window.open_note(space, child);
    until(|| window.inner().editor.borrow().as_ref().is_some_and(|e| e.note == child));
    window.open_note(space, project);
    until(|| window.inner().editor.borrow().as_ref().is_some_and(|e| e.note == project));
    window.delete_note();
    respond("Delete this project?", "all subprojects and tasks", "delete");
    until(|| window.inner().editor.borrow().is_none());
    assert!(!window.inner().back.borrow().iter().any(|(_, id)| [child, project].contains(id)));
    assert!(!window.inner().forward.borrow().iter().any(|(_, id)| [child, project].contains(id)));
    assert_eq!(
        state
            .search_shown(&[space], "@type=task", Sort::Title, 20)
            .unwrap()
            .into_iter()
            .map(|(_, hit)| hit.id)
            .collect::<Vec<_>>(),
        [standalone]
    );
    let reopened = runtime.block_on(State::for_test(dirs)).unwrap();
    for id in [child, completed, nested_task, project, subtask, task] {
        assert!(runtime.block_on(reopened.note(space, id)).is_err());
    }
    assert!(runtime.block_on(reopened.note(space, standalone)).is_ok());
    window.inner().window.destroy();
    runtime.block_on(reopened.finish());
    runtime.block_on(state.finish());
}

#[test]
fn parent_popup_filters_projects_and_excludes_cycles_and_tasks() {
    init_gtk();
    let runtime = crate::app::test_runtime();
    let root = tempfile::tempdir().unwrap();
    let state = runtime.block_on(State::for_test(Dirs::under(root.path()))).unwrap();
    let (space, project, beta) = runtime.block_on(async {
        let space = state.spaces().await[0].0;
        let project = state.create_work(space, "project", "Current", None).await.unwrap();
        state.create_work(space, "project", "Subproject", Some(project)).await.unwrap();
        state.create_work(space, "task", "Not a project", Some(project)).await.unwrap();
        state.create_work(space, "project", "Alpha", None).await.unwrap();
        let beta = state.create_work(space, "project", "Beta", None).await.unwrap();
        (space, project, beta)
    });
    let app = adw::Application::builder()
        .application_id("systems.thoughtfull.Zibaldone.ParentTest")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    let window = Window::new(&app, state.clone());
    present(&window.inner().window.clone().upcast());
    window.open_note(space, project);
    until(|| window.inner().work.header.is_visible());
    let empty_parent = descendants(&window.inner().work.header)
        .into_iter()
        .filter_map(|widget| widget.downcast::<gtk::Button>().ok())
        .find(|button| button.label().as_deref() == Some("No parent"))
        .unwrap();
    assert!(empty_parent.has_css_class("zib-work-field-control"));
    assert!(!empty_parent.is_sensitive());
    let popup = work_popup(&window.inner().work.header, "Move…");
    let widgets = descendants(&popup);
    let search =
        widgets.iter().find_map(|w| w.clone().downcast::<gtk::SearchEntry>().ok()).unwrap();
    let list = widgets.iter().find_map(|w| w.clone().downcast::<gtk::ListBox>().ok()).unwrap();
    assert!(list.row_at_index(2).is_none());
    search.set_text("BE");
    until(|| !list.row_at_index(0).unwrap().is_visible());
    assert!(list.row_at_index(1).unwrap().is_visible());
    list.emit_by_name::<()>("row-activated", &[&list.row_at_index(1).unwrap()]);
    until(|| {
        runtime.block_on(state.work_tree(space)).unwrap().items[&project].parent == Some(beta)
    });
    window.close_editor();
    runtime.block_on(state.finish());
    window.inner().window.destroy();
}

fn undo(widget: &EditorWidget) {
    editor_keys(widget).emit_by_name::<bool>(
        "key-pressed",
        &[&gdk::Key::z, &0u32, &gdk::ModifierType::CONTROL_MASK],
    );
}

#[test]
fn last_note_restores_after_close_and_restart_and_explicit_navigation_takes_precedence() {
    init_gtk();
    let runtime = crate::app::test_runtime();
    let root = tempfile::tempdir().unwrap();
    let dirs = Dirs::under(root.path());
    let state = runtime.block_on(State::for_test(dirs.clone())).unwrap();
    let (space, first, second) = runtime.block_on(async {
        let space = state.spaces().await[0].0;
        let first = state.create_note(space, "First note").await.unwrap();
        let second = state.create_note(space, "Second note").await.unwrap();
        (space, first, second)
    });
    let app = adw::Application::builder()
        .application_id("systems.thoughtfull.Zibaldone.RestoreTest")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    let window = Window::new(&app, state.clone());
    window.open_note(space, first);
    until(|| runtime.block_on(state.last_note()) == Some((space, first)));
    window.close_editor();
    window.restore_last_note();
    until(|| window.inner().widget.text().as_deref() == Some("First note"));
    window.close_editor();
    window.restore_last_note();
    window.open_uri(&format!("zib:note/{second}"));
    until(|| runtime.block_on(state.last_note()) == Some((space, second)));
    assert_eq!(window.inner().widget.text().as_deref(), Some("Second note"));
    window.close_editor();
    runtime.block_on(state.finish());
    window.inner().window.destroy();

    let reopened = runtime.block_on(State::for_test(dirs)).unwrap();
    let window = Window::new(&app, reopened.clone());
    window.restore_last_note();
    until(|| window.inner().widget.text().as_deref() == Some("Second note"));
    window.close_editor();
    runtime.block_on(reopened.delete_note(space, second)).unwrap();
    assert_eq!(runtime.block_on(reopened.last_note()), None);
    runtime.block_on(reopened.finish());
    window.inner().window.destroy();
}

#[test]
fn chat_draft_undo_is_independent_and_transcripts_are_editable_notes() {
    init_gtk();
    let runtime = crate::app::test_runtime();
    let root = tempfile::tempdir().unwrap();
    let dirs = Dirs::under(root.path());
    let state = runtime.block_on(State::for_test(dirs.clone())).unwrap();
    let (space, note) = runtime.block_on(async {
        let space = state.spaces().await[0].0;
        (space, state.create_note(space, "Original note").await.unwrap())
    });
    let app = adw::Application::builder()
        .application_id("systems.thoughtfull.Zibaldone.ChatTest")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    let window = Window::new(&app, state.clone());
    window.open_note(space, note);
    let chat = &window.inner().chat;
    let draft = chat.composer();
    until(|| {
        window.inner().widget.text().as_deref() == Some("Original note") && draft.is_sensitive()
    });
    input(&window.inner().widget).emit_by_name::<()>("commit", &[&"Note edit "]);
    input(draft).emit_by_name::<()>("commit", &[&"**Chat draft**"]);
    assert_eq!(state.agent.snapshot().draft, "**Chat draft**");
    undo(draft);
    assert_eq!(draft.text().as_deref(), Some(""));
    assert_eq!(window.inner().widget.text().as_deref(), Some("Note edit Original note"));
    input(draft).emit_by_name::<()>("commit", &[&"Archive this discussion"]);
    // Agent-created notes use excluded origins even while the main editor has an undo manager.
    let creation = zib_core::agent::Creation { id: Id::generate(), space: Some(space) };
    runtime
        .block_on(zib_core::agent::Host::create_note(
            &*state,
            creation.clone(),
            "note".into(),
            "Agent-created note".into(),
        ))
        .unwrap();
    runtime
        .block_on(zib_core::agent::Host::write_note(
            &*state,
            note,
            "Note edit Original note".into(),
            "Note edit Original note\n\nAgent addition".into(),
        ))
        .unwrap();
    undo(&window.inner().widget);
    assert_eq!(window.inner().widget.text().as_deref(), Some("Original note\n\nAgent addition"));
    assert_eq!(draft.text().as_deref(), Some("Archive this discussion"));
    assert!(runtime.block_on(state.note(space, creation.id)).is_ok());
    runtime.block_on(state.agent.archive(&*state, space)).unwrap();
    let transcript = runtime
        .block_on(state.notes(space))
        .unwrap()
        .into_iter()
        .find(|note| note.type_name == "agent-transcript")
        .unwrap();
    window.open_note(space, transcript.id);
    until(|| {
        window.inner().widget.text().is_some_and(|text| text.contains("Archive this discussion"))
    });
    input(&window.inner().widget).emit_by_name::<()>("commit", &[&"Editable "]);
    assert!(
        runtime.block_on(state.note(space, transcript.id)).unwrap().body.starts_with("Editable ")
    );
    // The instance saves the latest draft even when its debounce timer hasn't fired.
    state.agent.edit_draft("Unsent at shutdown".into());
    runtime.block_on(state.finish());
    let reopened = runtime.block_on(State::for_test(dirs)).unwrap();
    assert_eq!(reopened.agent.snapshot().draft, "Unsent at shutdown");
    runtime.block_on(reopened.finish());
    window.inner().window.destroy();
}

#[test]
fn default_editor_edits_the_shared_body_and_keeps_it_through_navigation_and_close() {
    init_gtk();
    let runtime = crate::app::test_runtime();
    let root = std::env::temp_dir().join(format!("zib-window-{}", Id::generate()));
    let dirs = Dirs::under(&root);
    let state = runtime.block_on(State::for_test(dirs.clone())).unwrap();
    let (space, first, second, target) = runtime.block_on(async {
        let space = state.spaces().await[0].0;
        let first = state.create_note(space, "prefix\n[[").await.unwrap();
        let second = state.create_note(space, "another note").await.unwrap();
        let target = state.create_note(space, "Linked title").await.unwrap();
        (space, first, second, target)
    });
    let app = adw::Application::builder()
        .application_id("systems.thoughtfull.Zibaldone.WindowTest")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    let window = Window::new(&app, state.clone());
    window.open_note(space, first);
    let widget = &window.inner().widget;
    until(|| widget.text().as_deref() == Some("prefix\n[["));
    assert!(widget.ancestor(adw::ApplicationWindow::static_type()).is_some());
    input(widget).emit_by_name::<()>("commit", &[&"Typed "]);
    assert_eq!(widget.text().as_deref(), Some("Typed prefix\n[["));
    assert_eq!(window.inner().title.title(), "Typed prefix");
    let text = widget.text().unwrap();
    widget.insert_link(text.len() - 2..text.len(), "[[", target);
    let expected = format!("Typed prefix\n[[{target}]]");
    // The body changes before the metadata timer, and displayed titles never enter it.
    assert_eq!(runtime.block_on(state.note(space, first)).unwrap().body, expected);
    let peer = Id::generate();
    window.live_event(space, first, LiveEvent::Started { machine: peer });
    assert_eq!(window.inner().title.subtitle(), "Editing live");
    window.live_event(space, first, LiveEvent::Ended { machine: peer });
    assert_eq!(window.inner().title.subtitle(), "");
    window.open_note(space, second);
    until(|| widget.text().as_deref() == Some("another note"));
    window.go(true);
    until(|| widget.text().as_deref() == Some(expected.as_str()));
    // An immediate note switch reports the old note, without copying a view buffer over its body.
    input(widget).emit_by_name::<()>("commit", &[&"Again "]);
    let expected = format!("Again {expected}");
    window.open_note(space, second);
    until(|| widget.text().as_deref() == Some("another note"));
    window.close_editor();
    assert_eq!(widget.text(), None);
    // Closing a note reports pending edits to the search index too.
    until(|| state.search("Again", 20).iter().any(|(s, hit)| *s == space && hit.id == first));
    runtime.block_on(state.finish());
    let reopened = runtime.block_on(State::for_test(dirs)).unwrap();
    assert_eq!(runtime.block_on(reopened.note(space, first)).unwrap().body, expected);
    runtime.block_on(reopened.finish());
    window.inner().window.destroy();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn quick_search_opens_the_result_selected_with_the_arrow_keys() {
    init_gtk();
    let runtime = crate::app::test_runtime();
    let root = tempfile::tempdir().unwrap();
    let state = runtime.block_on(State::for_test(Dirs::under(root.path()))).unwrap();
    let (space, exact, longer) = runtime.block_on(async {
        let space = state.spaces().await[0].0;
        let exact = state.create_note(space, "# Garden").await.unwrap();
        let longer = state.create_note(space, "# Garden plans").await.unwrap();
        state.create_note(space, "# Groceries\n\nGarden supplies").await.unwrap();
        add_recurring_picker_task(&state, space).await;
        (space, exact, longer)
    });
    let app = adw::Application::builder()
        .application_id("systems.thoughtfull.Zibaldone.QuickSearchTest")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    let window = Window::new(&app, state.clone());
    present(window.inner().window.upcast_ref());
    window.open_note(space, exact);
    until(|| !window.shown_spaces().is_empty() && window.current_space_id() == Some(space));

    assert!(window.inner().widget.grab_focus());
    let controllers = window.inner().window.observe_controllers();
    let shortcuts = (0..controllers.n_items())
        .filter_map(|i| controllers.item(i)?.downcast::<gtk::EventControllerKey>().ok())
        .find(|keys| keys.propagation_phase() == gtk::PropagationPhase::Capture)
        .unwrap();
    assert!(shortcuts.emit_by_name::<bool>(
        "key-pressed",
        &[&gdk::Key::o, &0u32, &gdk::ModifierType::CONTROL_MASK],
    ));
    let popup = &window.inner().quick_search;
    assert_eq!(popup.parent().as_ref(), Some(window.inner().title.upcast_ref()));
    let widgets = descendants(popup);
    let entry =
        widgets.iter().find_map(|widget| widget.downcast_ref::<gtk::SearchEntry>()).unwrap();
    let list = widgets.iter().find_map(|widget| widget.downcast_ref::<gtk::ListBox>()).unwrap();
    until(|| list.row_at_index(2).is_some());
    let focus = gtk::prelude::RootExt::focus(&window.inner().window).unwrap();
    assert!(focus.is_ancestor(entry) || &focus == entry.upcast_ref::<gtk::Widget>());
    entry.set_text("Recurring review");
    until(|| picker_has_recurring_task(list));
    entry.set_text("Garden");
    until(|| list.row_at_index(1).is_some());
    assert!(list.row_at_index(2).is_none());
    assert_eq!(list.selected_row().unwrap().index(), 0);
    assert_eq!(picker_badges(&list.selected_row().unwrap()), (Some("note".into()), false));

    let controllers = entry.observe_controllers();
    let arrows = (0..controllers.n_items())
        .filter_map(|i| controllers.item(i)?.downcast::<gtk::EventControllerKey>().ok())
        .find(|keys| keys.propagation_phase() == gtk::PropagationPhase::Capture)
        .unwrap();
    assert!(arrows.emit_by_name::<bool>(
        "key-pressed",
        &[&gdk::Key::Down, &0u32, &gdk::ModifierType::empty()],
    ));
    assert_eq!(list.selected_row().unwrap().index(), 1);
    entry.emit_by_name::<()>("activate", &[]);
    until(|| window.inner().editor.borrow().as_ref().is_some_and(|editor| editor.note == longer));
    until(|| !popup.is_mapped());
    window.go(true);
    until(|| window.inner().editor.borrow().as_ref().is_some_and(|editor| editor.note == exact));
    window.close_editor();
    runtime.block_on(state.finish());
    window.inner().window.destroy();
}

async fn add_recurring_picker_task(state: &State, space: Id) {
    use zib_core::recurrence::{Mode, Rule};

    let task = state.create_work(space, "task", "Recurring review", None).await.unwrap();
    let rule = Rule {
        due: None,
        mode: Mode::Keep,
        pattern: "FREQ=DAILY;COUNT=1".into(),
        scheduled: Some(0),
        start: zib_core::recurrence::today(),
    };
    let replacement = state.recurrence_replacement(space, task, Some(rule.clone())).await.unwrap();
    state.set_recurrence(space, task, Some(rule), replacement).await.unwrap();
}

fn picker_badges(row: &gtk::ListBoxRow) -> (Option<String>, bool) {
    let widgets = descendants(row);
    let pill = widgets.iter().find_map(|widget| {
        widget
            .has_css_class("zib-type-pill")
            .then(|| widget.downcast_ref::<gtk::Label>().unwrap().text().to_string())
    });
    let recurring = widgets.iter().any(|widget| {
        widget.downcast_ref::<gtk::Image>().is_some_and(|image| {
            image.icon_name().as_deref() == Some("media-playlist-repeat-symbolic")
        })
    });
    (pill, recurring)
}

fn picker_has_recurring_task(list: &gtk::ListBox) -> bool {
    (0..)
        .map_while(|index| list.row_at_index(index))
        .any(|row| picker_badges(&row) == (Some("task".into()), true))
}

#[test]
fn link_picker_creates_a_note_from_the_search_and_links_to_it() {
    init_gtk();
    let runtime = crate::app::test_runtime();
    let root = tempfile::tempdir().unwrap();
    let state = runtime.block_on(State::for_test(Dirs::under(root.path()))).unwrap();
    let (space, source) = runtime.block_on(async {
        let space = state.spaces().await[0].0;
        state.create_note(space, "Existing topic").await.unwrap();
        add_recurring_picker_task(&state, space).await;
        (space, state.create_note(space, "See [[").await.unwrap())
    });
    let app = adw::Application::builder()
        .application_id("systems.thoughtfull.Zibaldone.LinkPickerTest")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    let window = Window::new(&app, state.clone());
    present(window.inner().window.upcast_ref());
    window.open_note(space, source);
    let widget = &window.inner().widget;
    until(|| {
        window.current_space_id() == Some(space) && widget.text().as_deref() == Some("See [[")
    });
    window.open_link_picker(widget, 4..6);
    let popover = widget.last_child().unwrap().downcast::<gtk::Popover>().unwrap();
    let content = popover.child().unwrap();
    let search = content.first_child().unwrap().downcast::<gtk::SearchEntry>().unwrap();
    let scroll = content.last_child().unwrap().downcast::<gtk::ScrolledWindow>().unwrap();
    let viewport = scroll.child().unwrap().downcast::<gtk::Viewport>().unwrap();
    let list = viewport.child().unwrap().downcast::<gtk::ListBox>().unwrap();

    search.set_text("Recurring review");
    until(|| picker_has_recurring_task(&list));
    search.set_text("Existing topic");
    until(|| list.selected_row().is_some_and(|row| row.index() == 1));
    assert_eq!(picker_badges(&list.selected_row().unwrap()), (Some("note".into()), false));
    search.set_text("  New topic & details  ");
    until(|| list.selected_row().is_some_and(|row| row.index() == 0));
    search.emit_by_name::<()>("activate", &[]);
    until(|| widget.text().is_some_and(|text| text.ends_with("]]")));

    let created = runtime
        .block_on(state.notes(space))
        .unwrap()
        .into_iter()
        .find(|note| note.plain == "New topic & details")
        .expect("the note created by the link picker");
    assert_eq!(
        runtime.block_on(state.note(space, created.id)).unwrap().body,
        "# New topic & details"
    );
    assert_eq!(widget.text().unwrap(), format!("See [[{}]]", created.id));
    window.close_editor();
    runtime.block_on(state.finish());
    window.inner().window.destroy();
}

#[test]
fn space_dictionary_menu_opens_the_note_created_by_adding_a_word() {
    init_gtk();
    let runtime = crate::app::test_runtime();
    let root = tempfile::tempdir().unwrap();
    let dirs = Dirs::under(root.path());
    let state = runtime.block_on(State::for_test(dirs.clone())).unwrap();
    let (space, note) = runtime.block_on(async {
        let space = state.spaces().await[0].0;
        (space, state.create_note(space, "Original note").await.unwrap())
    });
    let app = adw::Application::builder()
        .application_id("systems.thoughtfull.Zibaldone.DictionaryTest")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    let window = Window::new(&app, state.clone());
    window.open_note(space, note);
    let widget = &window.inner().widget;
    until(|| widget.text().as_deref() == Some("Original note"));
    window.spelling_dictionary(Some("Quxley".into()));
    until(|| !state.search("quxley", 20).is_empty());
    assert_eq!(widget.text().as_deref(), Some("Original note"));
    WidgetExt::activate_action(&window.inner().window, "win.edit-space-dictionary", None).unwrap();
    until(|| widget.text().as_deref() == Some("# Spelling Dictionary\n\nQuxley"));
    let dictionary = window.inner().editor.borrow().as_ref().unwrap().note;
    input(widget).emit_by_name::<()>("commit", &[&"Edited "]);
    assert_eq!(widget.text().as_deref(), Some("Edited # Spelling Dictionary\n\nQuxley"));
    window.close_editor();
    until(|| state.search("edited", 20).iter().any(|(_, hit)| hit.id == dictionary));
    runtime.block_on(state.finish());
    window.inner().window.destroy();

    let reopened = runtime.block_on(State::for_test(dirs)).unwrap();
    assert_eq!(runtime.block_on(reopened.spelling_dictionary(space, None)).unwrap(), dictionary);
    assert_eq!(
        runtime.block_on(reopened.note(space, dictionary)).unwrap().body,
        "Edited # Spelling Dictionary\n\nQuxley"
    );
    runtime.block_on(reopened.finish());
}

fn detail_values(details: &gtk::MenuButton) -> Vec<(String, gtk::Label)> {
    let scroll =
        details.popover().unwrap().child().unwrap().downcast::<gtk::ScrolledWindow>().unwrap();
    assert!(scroll.measure(gtk::Orientation::Horizontal, -1).0 >= 420);
    let viewport = scroll.child().unwrap().downcast::<gtk::Viewport>().unwrap();
    let rows = viewport.child().unwrap().downcast::<gtk::Grid>().unwrap();
    (0..)
        .map_while(|row| {
            let key = rows.child_at(0, row)?.downcast::<gtk::Label>().unwrap();
            let value = rows.child_at(1, row).unwrap().downcast::<gtk::Label>().unwrap();
            Some((key.text().to_string(), value))
        })
        .collect()
}

#[test]
fn note_metadata_shows_the_note_id_and_populated_fields_without_selecting_them() {
    use zib_core::space::FileHash;
    use zib_core::web_clip::{Clip, Part};

    init_gtk();
    let runtime = crate::app::test_runtime();
    let root = tempfile::tempdir().unwrap();
    let state = runtime.block_on(State::for_test(Dirs::under(root.path()))).unwrap();
    let url = "https://example.org/article?a=1&b=2";
    let bytes = b"file contents";
    let (space, article, file, other) = runtime.block_on(async {
        let space = state.spaces().await[0].0;
        let article = Id::generate();
        let clip = Clip {
            space,
            note: article,
            title: "Article title".into(),
            url: url.into(),
            captured_at: 1_700_000_000_000,
            published_on: Some("2023-02-28".into()),
            authors: vec!["Test Author".into()],
            body: vec![Part::Markdown("Article body".into())],
        };
        state.import_web_clip(clip, Vec::new()).await.unwrap();
        let file = state
            .add_bytes(
                space,
                bytes.to_vec(),
                "Example file".into(),
                "application/octet-stream".into(),
            )
            .await
            .unwrap();
        (space, article, file, state.create_note(space, "Other note").await.unwrap())
    });
    let app = adw::Application::builder()
        .application_id("systems.thoughtfull.Zibaldone.ArticleTest")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    let window = Window::new(&app, state.clone());
    window.open_note(space, article);
    until(|| window.inner().widget.text().as_deref() == Some("# Article title\n\nArticle body"));

    let details = &window.inner().note_metadata;
    assert!(details.get_visible());
    assert_eq!(details.label().as_deref(), Some("Note metadata"));
    let values = detail_values(details);
    assert_eq!(
        values.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(),
        ["authors", "captured-at", "created-at", "id", "published-on", "updated-at", "url"],
    );
    assert_eq!(values[0].1.text(), "Test Author");
    assert!(!values[1].1.text().is_empty());
    assert!(!values[2].1.text().is_empty());
    assert_eq!(values[3].1.text(), article.to_string());
    assert_eq!(values[4].1.text(), "2023-02-28");
    assert!(!values[5].1.text().is_empty());
    let source = &values[6].1;
    assert_eq!(source.text(), url);
    assert!(source.label().starts_with("<a href=\"https://example.org/article?a=1&amp;b=2\">"));
    assert_eq!(source.xalign().to_bits(), values[0].1.xalign().to_bits());

    present(window.inner().window.upcast_ref());
    details.popup();
    let popover = details.popover().unwrap();
    until(|| popover.is_mapped());
    assert!(values.iter().all(|(_, label)| label.selection_bounds().is_none()));
    let authors = &values[0].1;
    assert!(authors.grab_focus());
    authors.select_region(0, 4);
    assert_eq!(authors.selection_bounds(), Some((0, 4)));
    details.popdown();
    until(|| !popover.is_mapped());
    details.popup();
    until(|| popover.is_mapped());
    assert!(values.iter().all(|(_, label)| label.selection_bounds().is_none()));
    details.popdown();

    window.open_note(space, file);
    until(|| window.inner().widget.text().is_some_and(|body| body.starts_with("Example file")));
    assert!(details.get_visible());
    let values = detail_values(details);
    assert_eq!(values.len(), 3);
    assert_eq!(values[0].0, "created-at");
    assert_eq!(values[1].0, "id");
    assert_eq!(values[1].1.text(), file.to_string());
    assert_eq!(values[2].0, "updated-at");
    let (stored, mut fields) = runtime.block_on(state.note_with_fields(space, file)).unwrap();
    assert_eq!(
        stored.fields["file"].as_string().unwrap().as_str(),
        FileHash::of(bytes).to_string()
    );
    for name in ["text", "thumbnail"] {
        fields.push((name.into(), Value::File(FileHash::of(bytes))));
    }
    window.show_note_metadata(fields);
    assert_eq!(
        detail_values(details).iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(),
        ["created-at", "id", "updated-at"]
    );

    window.open_note(space, other);
    until(|| window.inner().widget.text().as_deref() == Some("Other note"));
    assert!(details.get_visible());
    let values = detail_values(details);
    assert_eq!(values.len(), 3);
    assert_eq!(values[1].0, "id");
    assert_eq!(values[1].1.text(), other.to_string());
    assert!(values[1].1.is_selectable());
    let metadata = runtime.block_on(state.note(space, other)).unwrap().meta;
    assert_eq!(values[0].1.text(), field_text(&Value::Timestamp(metadata.created_at)));
    assert_eq!(values[2].1.text(), field_text(&Value::Timestamp(metadata.updated_at)));
    let search = &window.inner().search;
    search.grab_focus();
    for name in ["created-at", "updated-at"] {
        search.buffer().set_text(&format!("@{}", &name[..3]));
        search.buffer().place_cursor(&search.buffer().end_iter());
        until(|| {
            descendants(search).iter().any(|widget| {
                widget.downcast_ref::<gtk::Label>().is_some_and(|label| label.text() == name)
            })
        });
    }
    search.buffer().set_text("@created-at? @updated-at?");
    until(|| window.inner().rows.borrow().len() == 3);
    window.inner().widget.grab_focus();
    input(&window.inner().widget).emit_by_name::<()>("commit", &[&"Changed "]);
    window.report_body_edited();
    until(|| {
        window.inner().editor.borrow().as_ref().is_some_and(|editor| {
            editor.fields.iter().any(|(name, value)| {
                name == "updated-at"
                    && matches!(value, Value::Timestamp(time) if *time > metadata.updated_at)
            })
        })
    });
    window.close_editor();
    runtime.block_on(state.finish());
    window.inner().window.destroy();
}

#[test]
fn sidebar_search_sorts_matches_and_shows_invalid_values() {
    init_gtk();
    let runtime = crate::app::test_runtime();
    let root = tempfile::tempdir().unwrap();
    let state = runtime.block_on(State::for_test(Dirs::under(root.path()))).unwrap();
    let (space, alpha, zulu) = runtime.block_on(async {
        let space = state.spaces().await[0].0;
        let alpha = state.create_note(space, "Alpha gardening").await.unwrap();
        let zulu = state.create_note(space, "Zulu gardening").await.unwrap();
        (space, alpha, zulu)
    });
    let app = adw::Application::builder()
        .application_id("systems.thoughtfull.Zibaldone.SearchTest")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    let window = Window::new(&app, state.clone());
    let inner = window.inner();
    gtk::prelude::WidgetExt::activate_action(
        &inner.window,
        "win.sort-notes",
        Some(&"title".to_variant()),
    )
    .unwrap();
    inner.search.buffer().set_text("gardening @type=note @published-on!?");
    until(|| *inner.rows.borrow() == [(space, alpha), (space, zulu)]);
    inner.search.buffer().set_text("@published-on>=2025-02-30");
    until(|| {
        inner.list.row_at_index(0).and_downcast::<adw::ActionRow>().is_some_and(|row| {
            row.title() == "Couldn't search"
                && row.subtitle().is_some_and(|text| text.contains("2025-02-30"))
        })
    });
    assert!(inner.rows.borrow().is_empty());
    inner.search.buffer().set_text("gardening @type=note @published-on>=2025-01-01");
    until(|| inner.list.row_at_index(0).is_none());
    runtime.block_on(state.finish());
    inner.window.destroy();
}

#[test]
fn saved_sidebar_keeps_drafts_saves_copies_and_restores_after_restart() {
    use zib_core::search::saved::Definition;

    init_gtk();
    let runtime = crate::app::test_runtime();
    let root = tempfile::tempdir().unwrap();
    let dirs = Dirs::under(root.path());
    let state = runtime.block_on(State::for_test(dirs.clone())).unwrap();
    let (space, note) = runtime.block_on(async {
        let space = state.spaces().await[0].0;
        let note = state.create_note(space, "Garden planning").await.unwrap();
        state
            .save_search(
                space,
                None,
                "Garden".into(),
                Definition { query: "garden\n@type=note".into(), ..Definition::default() },
            )
            .await
            .unwrap();
        (space, note)
    });
    let app = adw::Application::builder()
        .application_id("systems.thoughtfull.Zibaldone.SavedSearchTest")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    let window = Window::new(&app, state.clone());
    let inner = window.inner();
    until(|| inner.search_sidebar.picker.model().is_some_and(|model| model.n_items() == 2));
    inner.search_sidebar.picker.set_selected(1);
    until(|| window.search_definition().query == "garden\n@type=note");
    assert!(!inner.search_sidebar.save.is_sensitive());
    window.open_note(space, note);
    until(|| inner.widget.text().as_deref() == Some("Garden planning"));
    inner.search.buffer().set_text("garden AND NOT @type=article");
    until(|| inner.search_sidebar.save.is_sensitive());
    inner.search_sidebar.picker.set_selected(0);
    inner.search_sidebar.picker.set_selected(1);
    assert_eq!(window.search_definition().query, "garden AND NOT @type=article");
    until(|| inner.search_sidebar.save.is_sensitive());
    inner.search_sidebar.save.emit_clicked();
    until(|| {
        runtime.block_on(state.saved_searches())[0].search.definition.query
            == "garden AND NOT @type=article"
    });
    until(|| !inner.search_sidebar.save.is_sensitive());
    let entries = runtime.block_on(state.saved_searches());
    assert_eq!(entries[0].search.definition.query, "garden AND NOT @type=article");
    assert_eq!(inner.widget.text().as_deref(), Some("Garden planning"));

    // A copy changes only the new entry, and an empty query restores the full note list.
    inner.search.buffer().set_text("");
    until(|| inner.search_sidebar.copy.is_sensitive());
    inner.search_sidebar.copy.emit_clicked();
    let dialog = inner.window.visible_dialog().unwrap().downcast::<adw::AlertDialog>().unwrap();
    let name =
        dialog.extra_child().unwrap().first_child().unwrap().downcast::<gtk::Entry>().unwrap();
    name.set_text("Everything");
    dialog.emit_by_name::<()>("response", &[&"save"]);
    dialog.close();
    until(|| runtime.block_on(state.saved_searches()).len() == 2);
    until(|| inner.search_sidebar.picker.model().is_some_and(|model| model.n_items() == 3));
    assert_eq!(
        runtime.block_on(state.saved_searches())[1].search.definition.query,
        "garden AND NOT @type=article"
    );
    gtk::prelude::WidgetExt::activate_action(&inner.window, "win.toggle-search", None).unwrap();
    assert!(!inner.sidebar_split.shows_sidebar());
    assert_eq!(inner.widget.text().as_deref(), Some("Garden planning"));
    window.close_editor();
    runtime.block_on(state.finish());
    inner.window.destroy();

    let reopened = runtime.block_on(State::for_test(dirs)).unwrap();
    let saved = runtime.block_on(reopened.saved_searches());
    assert_eq!(saved.len(), 2);
    assert_eq!(saved[0].search.name, "Everything");
    assert!(saved[0].search.definition.query.is_empty());
    runtime.block_on(reopened.finish());
}

fn check_recurrence_calendar_dismissal(dialog: &adw::AlertDialog, ending: &gtk::DropDown) {
    let calendar = |picker: &gtk::Popover| picker.child().and_downcast::<gtk::Calendar>().unwrap();
    let start = work_popup(dialog, "Choose start date");
    let first = calendar(&start);
    let selected = descendants(&first)
        .into_iter()
        .find(|widget| widget.has_css_class("day-number") && widget.has_css_class("today"))
        .unwrap();
    click_calendar(&first, &selected);
    until(|| !start.is_mapped());
    assert!(dialog.is_visible());

    ending.set_selected(1);
    let end_input = descendants(dialog)
        .into_iter()
        .find(|widget| widget.tooltip_text().as_deref() == Some("Choose end date"))
        .and_then(|button| button.prev_sibling())
        .and_downcast::<gtk::Entry>()
        .unwrap();
    end_input.set_text("2026-12-02");
    let end = work_popup(dialog, "Choose end date");
    let last = calendar(&end);
    let later = descendants(&last)
        .into_iter()
        .find(|widget| {
            widget.has_css_class("day-number")
                && !widget.has_css_class("other-month")
                && widget.downcast_ref::<gtk::Label>().is_some_and(|label| label.text() == "3")
        })
        .unwrap();
    click_calendar(&last, &later);
    until(|| !end.is_mapped());
    assert_eq!(end_input.text(), "2026-12-03");
    assert!(dialog.is_visible());
}

fn check_recurrence_response_layout(dialog: &adw::AlertDialog) {
    let widgets = descendants(dialog);
    let mode = widgets
        .iter()
        .filter_map(|widget| widget.downcast_ref::<gtk::DropDown>())
        .find(|choice| {
            choice
                .model()
                .and_downcast::<gtk::StringList>()
                .is_some_and(|model| model.string(0).as_deref() == Some("Keep every occurrence"))
        })
        .unwrap();
    let buttons = ["Cancel", "Save", "End recurrence"].map(|label| {
        widgets
            .iter()
            .filter_map(|widget| widget.downcast_ref::<gtk::Button>())
            .find(|button| button.label().as_deref() == Some(label))
            .unwrap()
    });
    let clock = dialog.frame_clock().unwrap();
    let painted = Rc::new(Cell::new(false));
    let handler = clock.connect_after_paint({
        let painted = painted.clone();
        move |_| painted.set(true)
    });
    for choice in 0..3 {
        painted.set(false);
        mode.set_selected(choice);
        dialog.queue_draw();
        until(|| painted.get());
        let [cancel, save, end] = buttons.map(|button| button.compute_bounds(dialog).unwrap());
        assert_eq!(cancel.y(), save.y());
        assert_eq!(save.y(), end.y());
        assert!(cancel.x() < save.x() && save.x() < end.x());
    }
    clock.disconnect(handler);
}

fn check_recurrence_end_preview(window: &Window, dialog: &adw::AlertDialog, date: &str) {
    let host = &window.inner().window;
    let visible_heading = |heading: &str| {
        host.visible_dialog()
            .and_downcast::<adw::AlertDialog>()
            .is_some_and(|dialog| dialog.heading().as_deref() == Some(heading))
    };
    dialog.emit_by_name::<()>("response", &[&"end"]);
    dialog.close();
    until(|| visible_heading("End recurrence?"));
    let preview = host.visible_dialog().unwrap().downcast::<adw::AlertDialog>().unwrap();
    assert!(preview.body().contains("One open occurrence"));
    assert!(descendants(&preview).iter().any(|child| {
        child.downcast_ref::<gtk::Label>().is_some_and(|label| label.text() == date)
    }));
    preview.emit_by_name::<()>("response", &[&"back"]);
    preview.close();
    until(|| visible_heading("Edit recurrence"));
}

fn check_occurrence_comment_focus(window: &Window) {
    let header = &window.inner().work.header;
    let check = || {
        descendants(header)
            .into_iter()
            .find(|widget| widget.is_mapped() && widget.is::<gtk::CheckButton>())
    };
    until(|| check().is_some());
    let buttons: Vec<_> =
        std::iter::successors(check().unwrap().next_sibling(), |child| child.next_sibling())
            .map(|child| child.downcast::<gtk::Button>().unwrap().label().unwrap())
            .collect();
    assert_eq!(buttons, ["Complete with note…", "Skip", "Skip with note…"]);
    for label in ["Complete with note…", "Skip with note…"] {
        let find = || {
            descendants(&window.inner().work.header)
                .into_iter()
                .filter_map(|widget| widget.downcast::<gtk::Button>().ok())
                .find(|button| button.is_mapped() && button.label().as_deref() == Some(label))
        };
        until(|| find().is_some());
        find().unwrap().emit_clicked();
        let host = &window.inner().window;
        until(|| host.visible_dialog().is_some());
        let dialog = host.visible_dialog().unwrap().downcast::<adw::AlertDialog>().unwrap();
        assert_eq!(dialog.heading().as_deref(), label.strip_suffix('…'));
        let view = descendants(&dialog)
            .into_iter()
            .find_map(|child| child.downcast::<gtk::TextView>().ok())
            .unwrap();
        until(|| view.is_focus());
        assert!(view.is_editable());
        dialog.emit_by_name::<()>("response", &[&"cancel"]);
        dialog.close();
        until(|| host.visible_dialog().is_none());
    }
}

#[test]
fn repeating_task_expands_and_opens_occurrence_history() {
    init_gtk();
    let runtime = crate::app::test_runtime();
    let root = tempfile::tempdir().unwrap();
    let state = runtime.block_on(State::for_test(Dirs::under(root.path()))).unwrap();
    let (space, project, task, subtask) = runtime.block_on(async {
        let space = state.spaces().await[0].0;
        let project = state.create_work(space, "project", "Project", None).await.unwrap();
        let task = state.create_work(space, "task", "# Review", Some(project)).await.unwrap();
        state.create_work(space, "task", "Later task", Some(project)).await.unwrap();
        let subtask = state.create_work(space, "task", "Actual subtask", Some(task)).await.unwrap();
        (space, project, task, subtask)
    });
    let app = adw::Application::builder()
        .application_id("systems.thoughtfull.Zibaldone.RecurrenceTest")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.register(None::<&gio::Cancellable>).unwrap();
    let window = Window::new(&app, state.clone());
    present(&window.inner().window.clone().upcast());
    let select_result = |id| {
        window.inner().search.buffer().set_text(&format!("@id={id}"));
        until(|| window.inner().rows.borrow().as_slice() == [(space, id)]);
        let results = &window.inner().list;
        results.emit_by_name::<()>("row-activated", &[&results.row_at_index(0).unwrap()]);
    };
    select_result(task);
    until(|| task_row(&window, "Review").is_some());
    assert_eq!(window.inner().editor.borrow().as_ref().unwrap().note, project);
    let link = |label: &str| {
        let row = task_row(&window, "Review").unwrap();
        descendants(&row)
            .into_iter()
            .filter_map(|widget| widget.downcast::<gtk::Button>().ok())
            .find(|button| button.tooltip_text().is_some_and(|text| text.starts_with(label)))
            .unwrap()
    };
    link("Open task").emit_clicked();
    let widget = &window.inner().widget;
    until(|| widget.text().as_deref() == Some("# Review") && widget.is_focus());
    assert_eq!(window.inner().editor.borrow().as_ref().unwrap().note, task);
    editor_keys(widget).emit_by_name::<bool>(
        "key-pressed",
        &[&gdk::Key::End, &0u32, &gdk::ModifierType::CONTROL_MASK],
    );
    input(widget).emit_by_name::<()>("commit", &[&"\n\n- [x] Check the logs"]);
    let template = "# Review\n\n- [x] Check the logs";
    assert_eq!(runtime.block_on(state.note(space, task)).unwrap().body, template);
    let parent = || {
        descendants(&window.inner().work.header)
            .into_iter()
            .filter_map(|widget| widget.downcast::<gtk::Button>().ok())
            .find(|button| button.tooltip_text().as_deref() == Some("Open parent · Project"))
    };
    until(|| parent().is_some());
    assert!(window.inner().project_pane.scroll.is_visible());
    assert!(
        descendants(&window.inner().work.header)
            .iter()
            .any(|widget| { widget.downcast_ref::<gtk::CheckButton>().is_some() })
    );
    let add = descendants(&window.inner().work.header)
        .into_iter()
        .filter_map(|widget| widget.downcast::<gtk::Button>().ok())
        .find(|button| button.label().as_deref() == Some("Add recurrence…"))
        .unwrap();
    add.emit_clicked();
    until(|| window.inner().window.visible_dialog().is_some());
    let dialog =
        window.inner().window.visible_dialog().unwrap().downcast::<adw::AlertDialog>().unwrap();
    assert_eq!(dialog.heading().as_deref(), Some("Add recurrence"));
    let ending = descendants(&dialog)
        .into_iter()
        .filter_map(|widget| widget.downcast::<gtk::DropDown>().ok())
        .find(|choice| {
            choice
                .model()
                .and_downcast::<gtk::StringList>()
                .is_some_and(|model| model.string(0).as_deref() == Some("Never"))
        })
        .unwrap();
    check_recurrence_calendar_dismissal(&dialog, &ending);
    ending.set_selected(2);
    let set_number = |label, value| {
        let spin = descendants(&dialog)
            .into_iter()
            .filter_map(|widget| widget.downcast::<gtk::SpinButton>().ok())
            .find(|spin| spin.tooltip_text().as_deref() == Some(label))
            .unwrap();
        spin.set_value(value);
    };
    set_number("Repeat interval", 2.0);
    set_number("Number of occurrences", 3.0);
    for name in ["Mon", "Wed"] {
        let find_day = || {
            descendants(&dialog)
                .into_iter()
                .filter_map(|widget| widget.downcast::<gtk::ToggleButton>().ok())
                .find(|day| day.is_mapped() && day.label().as_deref() == Some(name))
        };
        until(|| find_day().is_some());
        find_day().unwrap().set_active(true);
    }
    dialog.emit_by_name::<()>("response", &[&"save"]);
    dialog.close();
    until(|| window.inner().work.tree.borrow().series.contains(&task));
    let tree = runtime.block_on(state.work_tree(space)).unwrap();
    let current = tree.current_occurrences[&task];
    assert_eq!(tree.children(task), [subtask]);
    let rule = runtime.block_on(state.recurrence_rule(space, task)).unwrap().unwrap();
    assert_eq!(rule.pattern, "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;COUNT=3;WKST=SU");
    parent().unwrap().emit_clicked();
    until(|| task_row(&window, "Review").is_some());
    assert_eq!(window.inner().editor.borrow().as_ref().unwrap().note, project);

    let add_task = || {
        descendants(&window.inner().work.widget)
            .into_iter()
            .filter_map(|widget| widget.downcast::<gtk::Entry>().ok())
            .find(|entry| {
                gtk::prelude::WidgetExt::is_visible(entry)
                    && entry.placeholder_text().as_deref() == Some("New task")
            })
    };
    let toggle_series = || {
        // The series is the only expandable row. Its occurrence can have the same title.
        let toggle = descendants(&window.inner().work.widget)
            .into_iter()
            .filter_map(|widget| widget.downcast::<gtk::Button>().ok())
            .find(|button| button.has_css_class("zib-task-expander") && button.is_sensitive())
            .unwrap();
        toggle.emit_clicked();
    };
    let assert_no_occurrence_rows = || {
        let titles: Vec<_> = descendants(&window.inner().work.widget)
            .into_iter()
            .filter_map(|widget| widget.downcast::<gtk::Entry>().ok())
            .filter(|entry| entry.has_css_class("zib-work-title-entry") && entry.text() == "Review")
            .collect();
        assert_eq!(titles.len(), 1);
    };
    add_task().unwrap().set_text("Draft task");
    toggle_series();
    until(|| task_row(&window, "Actual subtask").is_some());
    assert_no_occurrence_rows();
    assert!(task_row(&window, "Later task").is_some_and(|row| row.is_visible()));
    assert_eq!(add_task().unwrap().text(), "Draft task");
    toggle_series();
    until(|| task_row(&window, "Actual subtask").is_none());
    assert_no_occurrence_rows();
    assert_eq!(add_task().unwrap().text(), "Draft task");

    let row = task_row(&window, "Review").unwrap();
    assert!(
        descendants(&row)
            .iter()
            .any(|widget| { widget.tooltip_text().as_deref() == Some("Recurring task") })
    );
    assert!(descendants(&row).iter().all(|widget| {
        widget.tooltip_text().is_none_or(|text| {
            text != "Occurrence history" && !text.starts_with("Open current occurrence")
        })
    }));
    until(|| {
        let inner = window.inner();
        inner.rows.borrow().as_slice() == [(space, task)]
            && inner.list.row_at_index(0).is_some_and(|row| {
                descendants(&row)
                    .iter()
                    .any(|widget| widget.tooltip_text().as_deref() == Some("Recurring task"))
            })
    });
    select_result(task);
    until(|| widget.text().as_deref() == Some(template));
    assert_eq!(window.inner().editor.borrow().as_ref().unwrap().note, task);
    until(|| parent().is_some());
    assert!(window.inner().project_pane.scroll.is_visible());
    assert!(
        descendants(&window.inner().work.header)
            .iter()
            .all(|widget| { widget.downcast_ref::<gtk::CheckButton>().is_none() })
    );
    let summary = descendants(&window.inner().work.header)
        .into_iter()
        .find_map(|child| {
            child
                .downcast::<gtk::Label>()
                .ok()
                .filter(|label| label.text() == "Every 2 weeks on Mon, Wed for 3 occurrences")
        })
        .unwrap();
    assert!(!summary.has_css_class("caption"));
    assert!(descendants(&window.inner().work.header).iter().all(|widget| {
        widget.tooltip_text().is_none_or(|text| !text.starts_with("Open current occurrence"))
    }));
    let sidebar = &window.inner().work.widget;
    let heading = sidebar.first_child().unwrap().downcast::<gtk::Label>().unwrap();
    assert_eq!(heading.text(), "Occurrence history");
    until(|| summary.height() > 0 && heading.height() > 0);
    let content = &window.inner().project_pane.content;
    let bottom = summary.compute_bounds(content).unwrap();
    let top = heading.compute_bounds(content).unwrap().y();
    let gap = top - bottom.y() - bottom.height();
    assert!((0.0..40.0).contains(&gap), "recurrence summary to history gap: {gap}");
    summary.parent().and_downcast::<gtk::Button>().unwrap().emit_clicked();
    let host = &window.inner().window;
    until(|| host.visible_dialog().is_some());
    let dialog = host.visible_dialog().unwrap().downcast::<adw::AlertDialog>().unwrap();
    assert!(!dialog.has_response("apply-template"));
    check_recurrence_response_layout(&dialog);
    let date = tree.occurrences[&current].day.to_string();
    check_recurrence_end_preview(&window, &dialog, &date);
    assert_eq!(runtime.block_on(state.recurrence_rule(space, task)).unwrap(), Some(rule));
    let dialog = host.visible_dialog().unwrap().downcast::<adw::AlertDialog>().unwrap();
    dialog.emit_by_name::<()>("response", &[&"cancel"]);
    dialog.close();
    until(|| host.visible_dialog().is_none());
    let list = heading.next_sibling().unwrap().downcast::<gtk::ListBox>().unwrap();
    let current_row = descendants(&list)
        .into_iter()
        .filter_map(|child| child.downcast::<adw::ActionRow>().ok())
        .find(|row| row.title().starts_with(&date))
        .unwrap();
    adw::prelude::ActionRowExt::activate(&current_row);
    until(|| window.inner().editor.borrow().as_ref().is_some_and(|editor| editor.note == current));
    let body = runtime.block_on(state.note(space, current)).unwrap().body;
    assert_eq!(body, "# Review\n\n- [ ] Check the logs");
    check_occurrence_comment_focus(&window);
    let status = descendants(&window.inner().work.header)
        .into_iter()
        .filter_map(|child| child.downcast::<gtk::MenuButton>().ok())
        .find(|button| button.tooltip_text().as_deref() == Some("Work status · No status"))
        .unwrap();
    assert!(status.is_mapped() && status.has_css_class("zib-work-field-control"));
    assert!(!status.is_sensitive());
    assert_eq!(status.child().and_downcast::<gtk::Label>().unwrap().text(), "No status");
    let entry = reveal_work_addition(&window, "Add a subtask", "New subtask");
    assert!(runtime.block_on(state.work_tree(space)).unwrap().children(current).is_empty());
    entry.set_text("Occurrence subtask");
    entry.emit_activate();
    until(|| task_row(&window, "Occurrence subtask").is_some());
    let occurrence_subtask = window.inner().work.tree.borrow().children(current)[0];
    let series = descendants(&window.inner().work.header)
        .into_iter()
        .filter_map(|child| child.downcast::<gtk::Button>().ok())
        .find(|button| button.tooltip_text().as_deref() == Some("Open parent · Review"))
        .unwrap();
    series.emit_clicked();
    until(|| task_row(&window, "Actual subtask").is_some());
    assert_eq!(window.inner().editor.borrow().as_ref().unwrap().note, task);
    assert!(task_row(&window, "Occurrence subtask").is_none());
    select_result(current);
    until(|| task_row(&window, "Occurrence subtask").is_some());
    assert_eq!(window.inner().editor.borrow().as_ref().unwrap().note, current);
    select_result(task);
    until(|| {
        window.inner().editor.borrow().as_ref().is_some_and(|editor| editor.note == task)
            && task_row(&window, "Actual subtask").is_some()
    });
    select_result(occurrence_subtask);
    until(|| task_row(&window, "Occurrence subtask").is_some());
    assert_eq!(window.inner().editor.borrow().as_ref().unwrap().note, current);
    window.inner().window.close();
}
