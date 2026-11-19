//! Projects, tasks, and their derived hierarchy. See the projects and tasks specification.

use std::collections::{BTreeMap, BTreeSet};

use crate::schema::{self, Value};
use crate::space::Note;
use crate::{Error, Id, Result, Space};

mod order;
mod status;

pub const PRE_STATUS_WORK_QUERY: &str =
    "@type=task AND @completed-on!? AND (@scheduled-on!? OR @scheduled-on<=today)";
pub const WORK_QUERY: &str = concat!(
    "@type=task AND @completed-on!? AND (@scheduled-on!? OR @scheduled-on<=today)",
    " AND @status!?"
);

pub use status::Status;

pub fn is_work(type_name: &str) -> bool {
    matches!(type_name, schema::PROJECT | schema::TASK)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub note: Note,
    pub parent: Option<Id>,
    pub title: String,
}

impl Item {
    pub fn completed(&self) -> bool {
        self.note.type_name == schema::TASK && self.explicit_date("completed-on").is_some()
    }

    pub fn explicit_date(&self, field: &str) -> Option<String> {
        let value = self.note.fields.get(field)?.as_string()?;
        schema::is_date(value).then(|| value.to_string())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Date {
    pub source: Id,
    pub value: String,
}

/// A deterministic forest derived from the stored links, without repairing synced content.
#[derive(Clone, Debug, Default)]
pub struct Tree {
    pub items: BTreeMap<Id, Item>,
    order: BTreeMap<Id, usize>,
}

impl Tree {
    pub fn read(space: &Space) -> Self {
        let mut tree = Self { order: order::positions(space), ..Self::default() };
        for id in space.note_ids() {
            let Ok(note) = space.note(id) else { continue };
            if !is_work(&note.type_name) {
                continue;
            }
            let parent = note
                .fields
                .get("parent-id")
                .and_then(|v| v.as_string())
                .and_then(|v| Id::parse(v).ok());
            let title = space.title(id).map_or_else(|_| "Untitled".into(), |t| t.plain);
            tree.items.insert(id, Item { note, parent, title });
        }
        let invalid: Vec<_> = tree
            .items
            .iter()
            .filter_map(|(id, item)| {
                let parent = item.parent?;
                let valid = tree.items.get(&parent).is_some_and(|p| {
                    item.note.type_name == schema::TASK || p.note.type_name == schema::PROJECT
                });
                (!valid).then_some(*id)
            })
            .collect();
        for id in invalid {
            tree.items.get_mut(&id).unwrap().parent = None;
        }
        // Cutting the largest identifier in each cycle gives every replica the same forest.
        let mut visited = BTreeSet::new();
        for start in tree.items.keys().copied().collect::<Vec<_>>() {
            let mut path = Vec::new();
            let mut positions = BTreeMap::new();
            let mut cursor = Some(start);
            while let Some(id) = cursor {
                if visited.contains(&id) {
                    break;
                }
                if let Some(at) = positions.get(&id).copied() {
                    let cut = path[at..].iter().max().copied().unwrap();
                    tree.items.get_mut(&cut).unwrap().parent = None;
                    break;
                }
                positions.insert(id, path.len());
                path.push(id);
                cursor = tree.items[&id].parent;
            }
            visited.extend(path);
        }
        tree
    }

    /// Nearest parent first; the item itself isn't included.
    pub fn ancestors(&self, id: Id) -> Vec<Id> {
        let mut ancestors = Vec::new();
        let mut parent = self.items.get(&id).and_then(|item| item.parent);
        while let Some(id) = parent {
            ancestors.push(id);
            parent = self.items[&id].parent;
        }
        ancestors
    }

    pub fn context(&self, id: Id) -> Id {
        let path = std::iter::once(id).chain(self.ancestors(id)).collect::<Vec<_>>();
        path.iter()
            .find(|id| self.items.get(id).is_some_and(|i| i.note.type_name == schema::PROJECT))
            .copied()
            .unwrap_or_else(|| *path.last().unwrap())
    }

    pub fn date(&self, id: Id, field: &str) -> Option<Date> {
        if field == "completed-on" {
            return Some(Date { source: id, value: self.items.get(&id)?.explicit_date(field)? });
        }
        std::iter::once(id).chain(self.ancestors(id)).find_map(|source| {
            Some(Date { source, value: self.items.get(&source)?.explicit_date(field)? })
        })
    }

    pub fn projects(&self, id: Id) -> Vec<Id> {
        self.ancestors(id)
            .into_iter()
            .filter(|id| self.items[id].note.type_name == schema::PROJECT)
            .collect()
    }

    pub fn path(&self, id: Id) -> String {
        let mut ancestors = self.ancestors(id);
        ancestors.reverse();
        ancestors.iter().map(|id| self.items[id].title.as_str()).collect::<Vec<_>>().join(" › ")
    }

    pub fn children(&self, id: Id) -> Vec<Id> {
        let mut children: Vec<_> = self
            .items
            .iter()
            .filter_map(|(child, item)| (item.parent == Some(id)).then_some(*child))
            .collect();
        children.sort_by(|a, b| {
            self.order
                .get(a)
                .unwrap_or(&usize::MAX)
                .cmp(self.order.get(b).unwrap_or(&usize::MAX))
                .then_with(|| {
                    self.items[a].title.to_lowercase().cmp(&self.items[b].title.to_lowercase())
                })
                .then_with(|| a.cmp(b))
        });
        children
    }

    pub fn unfinished_below(&self, id: Id) -> bool {
        self.items.iter().any(|(child, item)| {
            item.note.type_name == schema::TASK
                && !item.completed()
                && self.ancestors(*child).contains(&id)
        })
    }
}

/// Local writes reject invalid parents; merged links use the deterministic read rules above.
pub(crate) fn validate(space: &Space, id: Id, field: &str, value: Option<&Value>) -> Result<()> {
    let note = space.note(id)?;
    if !is_work(&note.type_name) {
        return Ok(());
    }
    let Some(value) = value else { return Ok(()) };
    let valid = match (field, value) {
        ("completed-on", Value::Date(date)) => {
            note.type_name == schema::TASK && schema::is_date(date)
        }
        ("due-on" | "scheduled-on", Value::Date(date)) => schema::is_date(date),
        ("parent-id", Value::Link(parent)) => {
            let tree = Tree::read(space);
            tree.items.get(parent).is_some_and(|p| {
                *parent != id
                    && !tree.ancestors(*parent).contains(&id)
                    && (note.type_name == schema::TASK || p.note.type_name == schema::PROJECT)
            })
        }
        ("status", Value::Choice(value)) => Status::valid(value),
        ("waiting-for", Value::Text(_)) => true,
        (
            "completed-on" | "due-on" | "parent-id" | "scheduled-on" | "status" | "waiting-for",
            _,
        ) => false,
        _ => true,
    };
    if valid {
        Ok(())
    } else {
        Err(Error::Space(format!("Invalid {field} for {}", note.type_name)))
    }
}

impl Space {
    /// Changes only the task's title line, preserving other body edits made since it was loaded.
    pub fn rename_task(&mut self, id: Id, expected: &str, title: &str) -> Result<String> {
        let note = self.note(id)?;
        let title = title.trim();
        if note.type_name != schema::TASK || title.is_empty() || title.contains(['\n', '\r']) {
            return Err(Error::Space("Enter a task title on one line".into()));
        }
        let range = title_range(&note.body);
        let current = &note.body[range.clone()];
        if current != expected {
            return Err(Error::NoteChanged(id));
        }
        // Escape punctuation so the plain title entry doesn't introduce markdown or links.
        let mut replacement = String::from("# ");
        for character in title.chars() {
            if character.is_ascii_punctuation() {
                replacement.push('\\');
            }
            replacement.push(character);
        }
        let at = note.body[..range.start].chars().count();
        if !current.is_empty() {
            self.delete_text(id, at, current.chars().count())?;
        }
        self.insert_text(id, at, &replacement)?;
        self.commit();
        Ok(replacement)
    }

    pub fn create_work(&mut self, type_name: &str, body: &str, parent: Option<Id>) -> Result<Id> {
        if !is_work(type_name) {
            return Err(Error::Space("Choose a project or task".into()));
        }
        if let Some(parent) = parent {
            let target = self.note(parent)?;
            if !is_work(&target.type_name)
                || (type_name == schema::PROJECT && target.type_name != schema::PROJECT)
            {
                return Err(Error::Space("Choose a valid parent in this space".into()));
            }
        }
        let order = order::prepare(self)?;
        let id = Id::generate();
        let fields: Vec<_> = parent.map(|id| ("parent-id", Value::Link(id))).into_iter().collect();
        self.insert_note(id, type_name, body, &fields)?;
        order.push(id.to_string())?;
        crate::space::commit(self.doc());
        Ok(id)
    }
}

/// The raw first nonblank line, including its markdown but excluding its line ending.
pub fn raw_title(body: &str) -> &str {
    &body[title_range(body)]
}

fn title_range(body: &str) -> std::ops::Range<usize> {
    let mut start = 0;
    for line in body.split_inclusive('\n') {
        let text = line.trim_end_matches(['\n', '\r']);
        if !text.trim().is_empty() {
            return start..start + text.len();
        }
        start += line.len();
    }
    body.len()..body.len()
}

/// The local calendar date used by relative date searches and task urgency.
pub fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// A label and a semantic CSS class. Dates remain visible alongside color.
pub fn urgency(due: Option<&str>) -> (String, &'static str) {
    let today = chrono::Local::now().date_naive();
    let Some(due) = due.and_then(|v| chrono::NaiveDate::parse_from_str(v, "%Y-%m-%d").ok()) else {
        return ("No due date".into(), "dim-label");
    };
    let days = (due - today).num_days();
    if days < 0 {
        (format!("Overdue · {due}"), "error")
    } else if days == 0 {
        ("Due today".into(), "warning")
    } else if days <= 3 {
        (format!("Due soon · {due}"), "warning")
    } else {
        (format!("Due {due}"), "dim-label")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_is_a_local_date_that_does_not_inherit_or_change_with_edits() {
        let mut space = Space::create(Id::generate()).unwrap();
        let project = space.create_work(schema::PROJECT, "Project", None).unwrap();
        let parent = space.create_work(schema::TASK, "Parent", Some(project)).unwrap();
        let child = space.create_work(schema::TASK, "Subtask", Some(parent)).unwrap();
        let date = Value::Date("2026-11-18".into());
        assert!(space.set_field(project, "completed-on", &date).is_err());
        assert!(space.set_field(parent, "completed-on", &Value::Boolean(true)).is_err());
        assert!(
            space.set_field(parent, "completed-on", &Value::Date("2026-02-30".into())).is_err()
        );
        space.set_field(parent, "completed-on", &date).unwrap();
        space.set_body(parent, "# Revised\n\nLater notes").unwrap();
        let reopened = Space::open(&space.snapshot().unwrap(), Id::generate()).unwrap();
        let tree = Tree::read(&reopened);
        assert!(tree.items[&parent].completed());
        assert_eq!(
            tree.items[&parent].explicit_date("completed-on").as_deref(),
            Some("2026-11-18")
        );
        assert!(!tree.items[&child].completed());
        assert!(tree.date(child, "completed-on").is_none());
        space.clear_field(parent, "completed-on").unwrap();
        assert!(!Tree::read(&space).items[&parent].completed());
        space.set_field(parent, "completed-on", &Value::Date("2026-11-19".into())).unwrap();
        assert_eq!(
            Tree::read(&space).items[&parent].explicit_date("completed-on").as_deref(),
            Some("2026-11-19")
        );
    }

    #[test]
    fn renaming_a_task_preserves_notes_and_checks_only_the_title_line() {
        let mut space = Space::create(Id::generate()).unwrap();
        let task = space.create_work(schema::TASK, "\n# Original\n\nDetails α\n", None).unwrap();
        let expected = raw_title(&space.note(task).unwrap().body).to_owned();
        space.set_body(task, "\n# Original\n\nOther device's details β\n").unwrap();
        space.rename_task(task, &expected, "New *literal* [title]").unwrap();
        let note = space.note(task).unwrap();
        assert!(note.body.ends_with("\n\nOther device's details β\n"));
        assert_eq!(space.title(task).unwrap().plain, "New *literal* [title]");
        assert!(matches!(
            space.rename_task(task, &expected, "Stale edit"),
            Err(Error::NoteChanged(_))
        ));
        let title = raw_title(&note.body).to_owned();
        assert!(space.rename_task(task, &title, "").is_err());
        assert!(space.rename_task(task, &title, "One\nTwo").is_err());
        let ordinary = space.create_note(schema::NOTE, "Ordinary note").unwrap();
        assert!(space.rename_task(ordinary, "Ordinary note", "Task").is_err());
    }

    #[test]
    fn inheritance_overrides_moves_and_deleted_parents() {
        let mut space = Space::create(Id::generate()).unwrap();
        let project = space.create_work(schema::PROJECT, "Project", None).unwrap();
        let child = space.create_work(schema::PROJECT, "Subproject", Some(project)).unwrap();
        let task = space.create_work(schema::TASK, "Task", Some(child)).unwrap();
        let subtask = space.create_work(schema::TASK, "Subtask", Some(task)).unwrap();
        space.set_field(project, "due-on", &Value::Date("2026-11-30".into())).unwrap();
        space.set_field(child, "scheduled-on", &Value::Date("2026-11-20".into())).unwrap();
        space.set_field(task, "due-on", &Value::Date("2026-12-02".into())).unwrap();
        let tree = Tree::read(&space);
        assert_eq!(tree.context(subtask), child);
        assert_eq!(tree.projects(subtask), [child, project]);
        assert_eq!(tree.date(subtask, "due-on").unwrap().source, task);
        assert_eq!(tree.date(subtask, "scheduled-on").unwrap().source, child);
        assert!(space.set_field(project, "parent-id", &Value::Link(task)).is_err());
        assert!(space.set_field(project, "parent-id", &Value::Link(child)).is_err());
        assert!(space.set_field(task, "parent-id", &Value::Link(subtask)).is_err());
        assert!(space.set_field(task, "due-on", &Value::Date("2026-02-30".into())).is_err());
        space.clear_field(task, "due-on").unwrap();
        assert_eq!(Tree::read(&space).date(subtask, "due-on").unwrap().source, project);
        space.clear_field(task, "parent-id").unwrap();
        assert_eq!(Tree::read(&space).context(subtask), task);
        assert!(Tree::read(&space).date(subtask, "due-on").is_none());
        space.delete_note(task).unwrap();
        assert!(space.note(subtask).is_err());
        assert!(space.tombstone(subtask).is_some());
    }

    // Checks the projects and tasks specification's subtree deletion rules.
    #[test]
    fn deleting_work_removes_its_entire_branch_and_preserves_other_notes() {
        let mut space = Space::create(Id::generate()).unwrap();
        let project = space.create_work(schema::PROJECT, "Project", None).unwrap();
        let child = space.create_work(schema::PROJECT, "Subproject", Some(project)).unwrap();
        let nested = space.create_work(schema::PROJECT, "Nested", Some(child)).unwrap();
        let task = space.create_work(schema::TASK, "Task", Some(child)).unwrap();
        let subtask = space.create_work(schema::TASK, "Subtask", Some(task)).unwrap();
        let leaf = space.create_work(schema::TASK, "Completed", Some(subtask)).unwrap();
        space.set_field(leaf, "completed-on", &Value::Date(today())).unwrap();
        let sibling = space.create_work(schema::TASK, "Sibling", Some(project)).unwrap();
        let standalone = space.create_work(schema::TASK, "Standalone", None).unwrap();
        let note = space.create_note(schema::NOTE, &format!("Link [[{task}]]")).unwrap();
        let mut replica = Space::open(&space.snapshot().unwrap(), Id::generate()).unwrap();
        let deleted: BTreeSet<_> = space.delete_note(task).unwrap().into_iter().collect();
        assert_eq!(deleted, BTreeSet::from([leaf, subtask, task]));
        for id in &deleted {
            assert!(space.note(*id).is_err());
            let tombstone = space.tombstone(*id).unwrap();
            assert_eq!(tombstone.deleted_by, Some(space.machine()));
            assert!(!tombstone.title.is_empty());
        }
        assert_eq!(space.tombstone(leaf).unwrap().title, "Completed");
        assert_eq!(space.title(note).unwrap().plain, "Link Task");
        let deleted: BTreeSet<_> = space.delete_note(project).unwrap().into_iter().collect();
        assert_eq!(deleted, BTreeSet::from([child, nested, project, sibling]));
        replica.import_checked(&space.snapshot().unwrap()).unwrap();
        for id in [child, leaf, nested, project, sibling, subtask, task] {
            assert!(replica.note(id).is_err());
            assert_eq!(replica.tombstone(id), space.tombstone(id));
        }
        assert!(space.note(standalone).is_ok());
        assert!(space.note(note).is_ok());
        assert_eq!(space.title(note).unwrap().plain, "Link Task");
        let child =
            space.create_work(schema::TASK, "Standalone subtask", Some(standalone)).unwrap();
        assert_eq!(
            space.delete_note(standalone).unwrap().into_iter().collect::<BTreeSet<_>>(),
            BTreeSet::from([child, standalone])
        );
        assert_eq!(space.delete_note(note).unwrap(), [note]);
    }

    #[test]
    fn deleting_work_uses_the_visible_forest_without_deleting_later_sync_additions() {
        let mut first = Space::create(Id::generate()).unwrap();
        let project = first.create_work(schema::PROJECT, "Project", None).unwrap();
        let task = first.create_work(schema::TASK, "Task", Some(project)).unwrap();
        let mut second = Space::open(&first.snapshot().unwrap(), Id::generate()).unwrap();
        let later = second.create_work(schema::TASK, "Offline addition", Some(project)).unwrap();
        first.delete_note(project).unwrap();
        first.import_checked(&second.snapshot().unwrap()).unwrap();
        second.import_checked(&first.snapshot().unwrap()).unwrap();
        for space in [&first, &second] {
            assert!(space.note(project).is_err());
            assert!(space.note(task).is_err());
            assert!(space.note(later).is_ok());
            assert_eq!(Tree::read(space).items[&later].parent, None);
        }
    }

    #[test]
    fn concurrent_moves_converge_without_changing_the_stored_links() {
        let mut first = Space::create(Id::generate()).unwrap();
        let a = first.create_work(schema::PROJECT, "A", None).unwrap();
        let b = first.create_work(schema::PROJECT, "B", None).unwrap();
        let mut second = Space::open(&first.snapshot().unwrap(), Id::generate()).unwrap();
        first.set_field(a, "parent-id", &Value::Link(b)).unwrap();
        second.set_field(b, "parent-id", &Value::Link(a)).unwrap();
        first.doc().import(&second.snapshot().unwrap()).unwrap();
        second.doc().import(&first.snapshot().unwrap()).unwrap();
        let one = Tree::read(&first);
        let two = Tree::read(&second);
        for id in [a, b] {
            assert_eq!(one.items[&id].parent, two.items[&id].parent);
            assert_eq!(one.context(id), two.context(id));
            assert!(first.note(id).unwrap().fields.contains_key("parent-id"));
        }
        let roots: BTreeSet<_> =
            one.items.values().filter(|i| i.parent.is_none()).map(|i| i.note.id).collect();
        assert_eq!(roots, [a.max(b)].into());
    }
}
