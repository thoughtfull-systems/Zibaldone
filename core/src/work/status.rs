//! Work status, effective inheritance, and durable Waiting completion history.

use std::collections::BTreeSet;

use loro::LoroValue;

use crate::schema::child_map;
use crate::search::Sort;
use crate::search::saved::Definition;
use crate::space::note_map;
use crate::{Error, Id, Result, Space};

use super::{PRE_STATUS_WORK_QUERY, Tree, WORK_QUERY, is_work};

const HISTORY: &str = "waiting-values";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Status {
    pub source: Id,
    pub value: String,
    pub waiting_for: Option<String>,
}

impl Status {
    pub(super) fn valid(value: &str) -> bool {
        matches!(value, "maybe" | "none" | "someday" | "waiting")
    }

    pub fn label(&self) -> String {
        match self.value.as_str() {
            "maybe" => "Maybe".into(),
            "none" => "None".into(),
            "someday" => "Someday".into(),
            _ => self
                .waiting_for
                .as_ref()
                .map_or_else(|| "Waiting".into(), |value| format!("Waiting · {value}")),
        }
    }
}

impl Tree {
    /// The effective deferral, with an explicit None treated as available for search.
    pub fn status(&self, id: Id) -> Option<Status> {
        self.resolved_status(id).filter(|status| status.value != "none")
    }

    /// The nearest status declaration, including a None that stops inheritance.
    pub fn resolved_status(&self, id: Id) -> Option<Status> {
        std::iter::once(id).chain(self.ancestors(id)).find_map(|source| {
            let fields = &self.items.get(&source)?.note.fields;
            let value = fields.get("status")?.as_string()?.as_str();
            if !Status::valid(value) {
                return None;
            }
            let waiting_for = (value == "waiting")
                .then(|| fields.get("waiting-for")?.as_string().map(|value| value.trim()))
                .flatten()
                .filter(|value| !value.is_empty())
                .map(str::to_owned);
            Some(Status { source, value: value.into(), waiting_for })
        })
    }

    pub fn inherited_status(&self, id: Id) -> Option<Status> {
        self.items.get(&id)?.parent.and_then(|parent| self.resolved_status(parent))
    }
}

impl Space {
    /// Changes status and Waiting detail together. An absent status resumes inheritance.
    pub fn set_work_status(&mut self, id: Id, status: Option<&str>, reason: &str) -> Result<()> {
        let note = self.note(id)?;
        if !is_work(&note.type_name) || status.is_some_and(|status| !Status::valid(status)) {
            return Err(Error::Space("Choose Maybe, None, Someday, Waiting, or Inherit".into()));
        }
        let fields = child_map(&note_map(self.doc(), id)?, "fields")?;
        if let Some(status) = status {
            fields.insert("status", status)?;
        } else {
            fields.delete("status")?;
        }
        let reason = reason.trim();
        if status == Some("waiting") && !reason.is_empty() {
            fields.insert("waiting-for", reason)?;
            self.record_waiting_value(reason)?;
        } else {
            fields.delete("waiting-for")?;
        }
        self.note_edited(id)
    }

    pub(crate) fn record_waiting_value(&self, value: &str) -> Result<()> {
        let value = value.trim();
        if !value.is_empty() {
            self.doc().get_map(HISTORY).insert(value, true)?;
        }
        Ok(())
    }

    /// Includes current values from writers that haven't recorded completion history.
    pub fn waiting_values(&self) -> Vec<String> {
        let history = self.doc().get_map(HISTORY);
        let mut values: BTreeSet<_> = history
            .keys()
            .filter(|key| {
                !key.trim().is_empty()
                    && history
                        .get(key)
                        .is_some_and(|value| value.get_deep_value() == LoroValue::Bool(true))
            })
            .map(|key| key.trim().to_owned())
            .collect();
        for id in self.note_ids() {
            if let Ok(note) = self.note(id)
                && is_work(&note.type_name)
                && let Some(value) = note.fields.get("waiting-for").and_then(LoroValue::as_string)
                && !value.trim().is_empty()
            {
                values.insert(value.trim().to_owned());
            }
        }
        values.into_iter().collect()
    }

    pub(crate) fn upgrade_work_search(&mut self) -> Result<()> {
        if self.setting(None, "zib.work-status-search")?.is_some() {
            return Ok(());
        }
        let space = self.id()?;
        for search in self.saved_searches() {
            let definition = &search.definition;
            if search.name == "What should I work on?"
                && definition.query == PRE_STATUS_WORK_QUERY
                && definition.scope == Some(space)
                && definition.sort == Sort::Due
            {
                self.update_saved_search(
                    &search,
                    &Definition { query: WORK_QUERY.into(), ..definition.clone() },
                )?;
            }
        }
        self.set_setting(None, "zib.work-status-search", true)
    }
}

#[cfg(test)]
mod tests {
    use crate::schema::{self, Value};

    use super::*;

    #[test]
    fn nearest_status_overrides_and_none_stops_inheritance_until_cleared() {
        let mut space = Space::create(Id::generate()).unwrap();
        let root = space.create_work(schema::PROJECT, "Root", None).unwrap();
        let project = space.create_work(schema::PROJECT, "Subproject", Some(root)).unwrap();
        let task = space.create_work(schema::TASK, "Task", Some(project)).unwrap();
        let subtask = space.create_work(schema::TASK, "Subtask", Some(task)).unwrap();
        space.set_work_status(root, Some("waiting"), "Alex").unwrap();
        space.set_work_status(task, Some("maybe"), "").unwrap();
        assert_eq!(Tree::read(&space).status(project).unwrap().source, root);
        assert_eq!(Tree::read(&space).status(subtask).unwrap().source, task);
        assert_eq!(Tree::read(&space).inherited_status(task).unwrap().source, root);
        space.set_work_status(subtask, Some("waiting"), "Bea").unwrap();
        assert_eq!(Tree::read(&space).status(subtask).unwrap().waiting_for.as_deref(), Some("Bea"));
        space.set_work_status(subtask, None, "").unwrap();
        assert_eq!(Tree::read(&space).status(subtask).unwrap().value, "maybe");
        space.set_work_status(task, Some("none"), "").unwrap();
        for id in [subtask, task] {
            assert!(Tree::read(&space).status(id).is_none());
            let resolved = Tree::read(&space).resolved_status(id).unwrap();
            assert_eq!(resolved.value, "none");
            assert_eq!(resolved.source, task);
            assert_eq!(resolved.waiting_for, None);
        }
        space.set_work_status(project, Some("someday"), "").unwrap();
        assert!(Tree::read(&space).status(subtask).is_none());
        space.set_work_status(task, None, "").unwrap();
        assert_eq!(Tree::read(&space).status(subtask).unwrap().source, project);
        space.set_work_status(project, None, "").unwrap();
        assert_eq!(Tree::read(&space).status(subtask).unwrap().source, root);
        space.set_field(task, "status", &Value::Choice("waiting".into())).unwrap();
        space.set_field(task, "waiting-for", &Value::Text("Casey".into())).unwrap();
        assert_eq!(
            Tree::read(&space).status(subtask).unwrap().waiting_for.as_deref(),
            Some("Casey")
        );
        space.set_field(task, "due-on", &Value::Date("2026-01-01".into())).unwrap();
        space.set_field(subtask, "completed-on", &Value::Date(super::super::today())).unwrap();
        assert!(Tree::read(&space).items[&subtask].completed());
        space.set_work_status(task, Some("none"), "").unwrap();
        let reopened = Space::open(&space.snapshot().unwrap(), Id::generate()).unwrap();
        assert!(Tree::read(&reopened).status(subtask).is_none());
        space.move_task(task, Some(root), None).unwrap();
        assert!(Tree::read(&space).status(subtask).is_none());
        space.set_work_status(task, None, "").unwrap();
        assert_eq!(Tree::read(&space).status(subtask).unwrap().source, root);
        assert!(space.set_work_status(task, Some("active"), "").is_err());
    }

    #[test]
    fn waiting_history_survives_changes_deletion_reopen_and_concurrent_additions() {
        let mut first = Space::create(Id::generate()).unwrap();
        let a = first.create_work(schema::TASK, "A", None).unwrap();
        let b = first.create_work(schema::TASK, "B", None).unwrap();
        let mut second = Space::open(&first.snapshot().unwrap(), Id::generate()).unwrap();
        first.set_work_status(a, Some("waiting"), "  Alex  ").unwrap();
        second.set_work_status(b, Some("waiting"), "Bea").unwrap();
        first.set_work_status(a, Some("someday"), "").unwrap();
        assert!(!first.note(a).unwrap().fields.contains_key("waiting-for"));
        first.delete_note(a).unwrap();
        first.import_checked(&second.snapshot().unwrap()).unwrap();
        second.import_checked(&first.snapshot().unwrap()).unwrap();
        assert_eq!(first.waiting_values(), ["Alex", "Bea"]);
        assert_eq!(second.waiting_values(), first.waiting_values());
        first.set_work_status(b, None, "").unwrap();
        first.set_work_status(b, Some("waiting"), "").unwrap();
        assert_eq!(Tree::read(&first).status(b).unwrap().waiting_for, None);
        let reopened = Space::open(&first.snapshot().unwrap(), Id::generate()).unwrap();
        assert_eq!(reopened.waiting_values(), ["Alex", "Bea"]);
    }

    #[test]
    fn work_search_upgrade_changes_only_the_untouched_default_once() {
        let mut space = Space::create(Id::generate()).unwrap();
        let original = Definition {
            query: PRE_STATUS_WORK_QUERY.into(),
            scope: Some(space.id().unwrap()),
            sort: Sort::Due,
        };
        let default = space.create_saved_search("What should I work on?", &original).unwrap();
        let edited = space
            .create_saved_search(
                "What should I work on?",
                &Definition { query: "@type=task".into(), ..original.clone() },
            )
            .unwrap();
        let global = space
            .create_saved_search(
                "What should I work on?",
                &Definition { scope: None, ..original.clone() },
            )
            .unwrap();
        let reordered = space
            .create_saved_search(
                "What should I work on?",
                &Definition { sort: Sort::Title, ..original.clone() },
            )
            .unwrap();
        let mut reopened = Space::open(&space.snapshot().unwrap(), Id::generate()).unwrap();
        let searches = reopened.saved_searches();
        let upgraded = searches.iter().find(|s| s.id == default.id).unwrap();
        assert_eq!(upgraded.definition.query, WORK_QUERY);
        for search in [edited, global, reordered] {
            assert_eq!(
                searches.iter().find(|s| s.id == search.id).unwrap().definition,
                search.definition
            );
        }
        reopened.update_saved_search(upgraded, &original).unwrap();
        let again = Space::open(&reopened.snapshot().unwrap(), Id::generate()).unwrap();
        assert_eq!(
            again.saved_searches().into_iter().find(|s| s.id == default.id).unwrap().definition,
            original
        );
    }
}
