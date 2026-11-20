//! Desktop controls for recurring tasks and their occurrence notes.

use std::fmt::Write as _;

use adw::prelude::*;
use chrono::{Datelike, NaiveDate};
use gtk::{gio, glib};
use zib_core::Id;
use zib_core::recurrence::{self, Frequency, Mode, Occurrence, Outcome, Pattern, Rule};
use zib_core::work::{Item, Tree};

use super::{Window, core, flow};

mod form;

use form::{ORDINALS, RuleForm};

const ADD_RECURRENCE: &str = "Add recurrence…";
const EDIT_RECURRENCE: &str = "Edit recurrence…";

impl Window {
    pub(super) fn add_recurrence_actions(
        &self,
        space: Id,
        id: Id,
        tree: &Tree,
        menu: &gio::Menu,
        popup: &gtk::PopoverMenu,
        actions: &gio::SimpleActionGroup,
    ) {
        let w = self.clone();
        let active = tree.recurrence_rules.contains_key(&id);
        let label = if active { EDIT_RECURRENCE } else { ADD_RECURRENCE };
        add(menu, popup, actions, label, "repeat", move || w.edit_recurrence(space, id));
        if active {
            let w = self.clone();
            add(menu, popup, actions, "Open recurring task", "open-series", move || {
                w.open_task(space, id);
            });
        }
    }

    pub(super) fn occurrence_completion(
        &self,
        space: Id,
        item: &Item,
        occurrence: &Occurrence,
    ) -> gtk::Box {
        let actions = flow::container();
        let id = item.note.id;
        let check = self.completion(space, item);
        check.add_css_class("zib-work-field-control");
        actions.append(&check);
        let open = occurrence.outcome == Outcome::Open;
        let with_note = |label, outcome| {
            let button = gtk::Button::with_label(label);
            let w = self.clone();
            let day = occurrence.day.to_string();
            button.connect_clicked(move |_| w.occurrence_comment(space, id, outcome, &day));
            button
        };
        if open {
            actions.append(&with_note("Complete with note…", Outcome::Completed));
        }
        let state = if open { Outcome::Skipped } else { Outcome::Open };
        let button = gtk::Button::with_label(if open { "Skip" } else { "Reopen" });
        let w = self.clone();
        button.connect_clicked(move |_| w.save_occurrence(space, id, state, String::new()));
        actions.append(&button);
        if open {
            actions.append(&with_note("Skip with note…", Outcome::Skipped));
        }
        actions
    }

    pub(super) fn recurrence_control(&self, space: Id, series: Id, tree: &Tree) -> gtk::Button {
        let button = if let Some(rule) = tree.recurrence_rules.get(&series) {
            let summary = recurrence_summary(rule);
            let label = gtk::Label::builder()
                .label(&summary)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .xalign(0.0)
                .build();
            gtk::Button::builder()
                .child(&label)
                .tooltip_text(format!("{EDIT_RECURRENCE}\n{summary}"))
                .build()
        } else {
            gtk::Button::with_label(ADD_RECURRENCE)
        };
        let w = self.clone();
        button.connect_clicked(move |_| w.edit_recurrence(space, series));
        button
    }

    fn edit_recurrence(&self, space: Id, note: Id) {
        let (w, state) = (self.clone(), self.inner().state.clone());
        glib::spawn_future_local(async move {
            let loaded = core({
                let state = state.clone();
                async move { state.recurrence_rule(space, note).await }
            })
            .await;
            let existing = match loaded {
                Ok(rule) => rule,
                Err(error) => {
                    w.toast(&format!("Couldn't read the repetition: {error}"));
                    return;
                }
            };
            let fallback = Rule {
                mode: Mode::Keep,
                start: recurrence::today(),
                pattern: "FREQ=WEEKLY".into(),
                scheduled: Some(0),
                due: None,
            };
            let form =
                match RuleForm::new(existing.as_ref().unwrap_or(&fallback), existing.is_some()) {
                    Ok(form) => form,
                    Err(error) => {
                        w.toast(&format!("Couldn't open repetition: {error}"));
                        return;
                    }
                };
            let heading = if existing.is_some() { "Edit recurrence" } else { "Add recurrence" };
            let dialog = adw::AlertDialog::new(Some(heading), None);
            dialog.set_prefer_wide_layout(true);
            dialog.add_responses(&[("cancel", "Cancel"), ("save", "Save")]);
            if existing.is_some() {
                dialog.add_response("end", "End recurrence");
                dialog.set_response_appearance("end", adw::ResponseAppearance::Destructive);
            }
            dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
            dialog.set_default_response(Some("save"));
            dialog.set_close_response("cancel");
            dialog.set_extra_child(Some(&form.rows));
            form.watch(&dialog);
            let response = dialog.choose_future(Some(&w.inner().window)).await;
            let rule = match response.as_str() {
                "end" => None,
                "save" => match form.read() {
                    Ok(rule) => Some(rule),
                    Err(error) => {
                        w.toast(&error.to_string());
                        return;
                    }
                },
                _ => return,
            };
            if let Err(error) =
                core(async move { state.set_recurrence(space, note, rule).await }).await
            {
                w.toast(&format!("Couldn't save repetition: {error}"));
            } else {
                w.refresh_work();
                w.reload_notes();
            }
        });
    }

    fn occurrence_comment(&self, space: Id, id: Id, outcome: Outcome, day: &str) {
        let dialog = adw::AlertDialog::new(
            Some(if outcome == Outcome::Completed {
                "Complete with note"
            } else {
                "Skip with note"
            }),
            Some(&format!("Occurrence on {day}")),
        );
        let view = gtk::TextView::builder().wrap_mode(gtk::WrapMode::WordChar).build();
        let scroll = gtk::ScrolledWindow::builder().min_content_height(90).child(&view).build();
        dialog.set_extra_child(Some(&scroll));
        dialog.set_focus(Some(&view));
        dialog.add_responses(&[("cancel", "Cancel"), ("save", "Save")]);
        dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
        dialog.set_close_response("cancel");
        let w = self.clone();
        glib::spawn_future_local(async move {
            if dialog.clone().choose_future(Some(&w.inner().window)).await == "save" {
                let buffer = view.buffer();
                let comment =
                    buffer.text(&buffer.start_iter(), &buffer.end_iter(), true).to_string();
                if !w.write_occurrence(space, id, outcome, comment.clone()).await {
                    // A remote close can make the outcome stale. Keep the draft available.
                    dialog
                        .set_body("Add the text to this occurrence without changing its outcome?");
                    dialog.set_response_label("save", "Add note");
                    if dialog.choose_future(Some(&w.inner().window)).await == "save" {
                        let updated =
                            buffer.text(&buffer.start_iter(), &buffer.end_iter(), true).to_string();
                        let state = w.inner().state.clone();
                        if let Err(error) = core(async move {
                            state.add_occurrence_comment(space, id, updated).await
                        })
                        .await
                        {
                            w.toast(&format!("Couldn't add the note: {error}"));
                        }
                    }
                }
            }
        });
    }

    pub(super) fn save_occurrence(&self, space: Id, id: Id, outcome: Outcome, comment: String) {
        let w = self.clone();
        glib::spawn_future_local(async move {
            w.write_occurrence(space, id, outcome, comment).await;
        });
    }

    async fn write_occurrence(&self, space: Id, id: Id, outcome: Outcome, comment: String) -> bool {
        let state = self.inner().state.clone();
        let result =
            core(async move { state.record_occurrence(space, id, outcome, comment).await }).await;
        if let Err(error) = &result {
            self.toast(&format!("Couldn't update occurrence: {error}"));
        }
        self.refresh_work();
        if result.is_ok() {
            self.reload_notes();
        }
        result.is_ok()
    }

    pub(super) fn draw_occurrence_history(&self, space: Id, series: Id, tree: &Tree) {
        let mut rows: Vec<_> =
            tree.occurrences.iter().filter(|(_, row)| row.series == series).collect();
        if rows.is_empty() {
            return;
        }
        let history = &self.inner().work.widget;
        let heading = gtk::Label::builder().label("Occurrence history").xalign(0.0).build();
        heading.add_css_class("heading");
        history.append(&heading);
        let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
        rows.sort_by_key(|(id, row)| std::cmp::Reverse((row.day, **id)));
        let earlier = gtk::Button::with_label("Show earlier");
        for (index, (&id, occurrence)) in rows.iter().copied().enumerate() {
            let detail = match occurrence.outcome {
                Outcome::Open => occurrence
                    .superseded_on
                    .map_or_else(|| "Open".to_owned(), |day| format!("Superseded {day}")),
                outcome => format!(
                    "{} {}",
                    if outcome == Outcome::Completed { "Completed" } else { "Skipped" },
                    occurrence.closed.as_deref().unwrap_or_default()
                ),
            };
            let row = adw::ActionRow::builder()
                .title(format!("{} · {}", occurrence.day, tree.items[&id].title))
                .subtitle(detail)
                .activatable(true)
                .build();
            row.set_use_markup(false);
            if index >= 20 {
                row.set_visible(false);
            }
            let w = self.clone();
            row.connect_activated(move |_| w.open_note(space, id));
            list.append(&row);
        }
        earlier.set_visible(rows.len() > 20);
        let children = list.downgrade();
        earlier.connect_clicked(move |button| {
            let Some(list) = children.upgrade() else { return };
            let mut count = 0;
            let mut child = list.first_child();
            while let Some(row) = child {
                child = row.next_sibling();
                if !row.is_visible() {
                    if count == 20 {
                        return;
                    }
                    row.set_visible(true);
                    count += 1;
                }
            }
            button.set_visible(false);
        });
        history.append(&list);
        history.append(&earlier);
    }
}

fn recurrence_summary(rule: &Rule) -> String {
    let Ok(pattern) = rule.pattern.parse::<Pattern>() else { return EDIT_RECURRENCE.into() };
    let unit = match pattern.frequency {
        Frequency::Daily => "day",
        Frequency::Weekly => "week",
        Frequency::Monthly => "month",
        Frequency::Yearly => "year",
    };
    let mut summary = if pattern.interval == 1 {
        format!("Every {unit}")
    } else {
        format!("Every {} {unit}s", pattern.interval)
    };
    if rule.mode == Mode::Interval {
        summary.push_str(" after completion");
    } else {
        append_calendar_details(&mut summary, rule, &pattern);
    }
    if let Some(count) = pattern.count {
        let unit = if count == 1 { "occurrence" } else { "occurrences" };
        let _ = write!(summary, " for {count} {unit}");
    } else if let Some(date) = pattern.until {
        let _ = write!(summary, " through {date}");
    }
    if rule.mode == Mode::Jump {
        summary.push_str(" · jump when resolved");
    }
    summary
}

fn append_calendar_details(summary: &mut String, rule: &Rule, pattern: &Pattern) {
    let mut weekdays = pattern.weekdays.clone();
    if weekdays.is_empty() && pattern.frequency == Frequency::Weekly {
        weekdays.push((None, rule.start.weekday()));
    }
    let default_day = (weekdays.is_empty()
        && matches!(pattern.frequency, Frequency::Monthly | Frequency::Yearly))
    .then_some(rule.start.day());
    let day = pattern.month_day.or(default_day);
    if let Some(day) = day {
        let _ = write!(summary, " on day {day}");
    }
    if !weekdays.is_empty() {
        weekdays.sort_by_key(|(ordinal, day)| (*ordinal, day.num_days_from_sunday()));
        weekdays.dedup();
        let labels: Vec<_> = weekdays
            .iter()
            .map(|(ordinal, day)| {
                ORDINALS.iter().find(|(number, _)| Some(*number) == *ordinal).map_or_else(
                    || day.to_string(),
                    |(_, text)| format!("{} {day}", text.to_lowercase()),
                )
            })
            .collect();
        let join = if day.is_some() { " and " } else { " on " };
        let _ = write!(summary, "{join}{}", labels.join(", "));
    }
    let default_month = (pattern.frequency == Frequency::Yearly
        && pattern.weekdays.is_empty()
        && pattern.month_day.is_none())
    .then_some(rule.start.month());
    if let Some(month) = pattern.month.or(default_month)
        && let Some(date) = NaiveDate::from_ymd_opt(2000, month, 1)
    {
        let _ = write!(summary, " in {}", date.format("%B"));
    }
}

fn add(
    menu: &gio::Menu,
    popup: &gtk::PopoverMenu,
    actions: &gio::SimpleActionGroup,
    label: &str,
    name: &str,
    run: impl Fn() + 'static,
) {
    menu.append(Some(label), Some(&format!("task.{name}")));
    let action = gio::SimpleAction::new(name, None);
    let popup = popup.downgrade();
    action.connect_activate(move |_, _| {
        if let Some(popup) = popup.upgrade() {
            popup.popdown();
        }
        run();
    });
    actions.add_action(&action);
}
