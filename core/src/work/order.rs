//! A shared movable sequence orders siblings without replacing their hierarchy links.

use std::collections::{BTreeMap, BTreeSet};

use loro::{LoroMovableList, LoroValue, ValueOrContainer};

use super::{Tree, validate};
use crate::schema::{self, Value};
use crate::{Error, Id, Result, Space};

const ORDER: &str = "work-order";

fn entry(list: &LoroMovableList, index: usize) -> Option<Id> {
    match list.get(index)? {
        ValueOrContainer::Value(LoroValue::String(value)) => Id::parse(&value).ok(),
        _ => None,
    }
}

pub(super) fn positions(space: &Space) -> BTreeMap<Id, usize> {
    let list = space.doc().get_movable_list(ORDER);
    let mut positions = BTreeMap::new();
    for index in 0..list.len() {
        if let Some(id) = entry(&list, index) {
            positions.entry(id).or_insert(index);
        }
    }
    positions
}

/// Keep the first occurrence of live items after concurrent initialization, then append omissions.
pub(super) fn prepare(space: &Space) -> Result<LoroMovableList> {
    let tree = Tree::read(space);
    let list = space.doc().get_movable_list(ORDER);
    let mut seen = BTreeSet::new();
    let mut duplicates = Vec::new();
    for index in 0..list.len() {
        if let Some(id) = entry(&list, index)
            && tree.items.contains_key(&id)
            && !seen.insert(id)
        {
            duplicates.push(index);
        }
    }
    // Delete backwards so earlier indices keep referring to the same entries.
    for index in duplicates.into_iter().rev() {
        list.delete(index, 1)?;
    }
    let mut missing: Vec<_> = tree
        .items
        .values()
        .filter(|i| !i.note.is_task_occurrence() && !seen.contains(&i.note.id))
        .collect();
    missing.sort_by(|a, b| {
        a.title.to_lowercase().cmp(&b.title.to_lowercase()).then_with(|| a.note.id.cmp(&b.note.id))
    });
    for item in missing {
        list.push(item.note.id.to_string())?;
    }
    Ok(list)
}

impl Space {
    /// Reorders a project among its project siblings without changing its parent.
    pub fn reorder_project(&mut self, id: Id, before: Option<Id>) -> Result<()> {
        let tree = Tree::read(self);
        let Some(item) = tree.items.get(&id).filter(|i| i.note.type_name == schema::PROJECT) else {
            return Err(Error::Space("Choose a project to reorder".into()));
        };
        if let Some(before) = before {
            if tree
                .items
                .get(&before)
                .is_none_or(|i| i.note.type_name != schema::PROJECT || i.parent != item.parent)
            {
                return Err(Error::Space("Choose a project with the same parent".into()));
            }
            if before == id {
                return Ok(());
            }
        }
        place(self, id, before)?;
        crate::space::commit(self.doc());
        Ok(())
    }

    /// Move a task and its subtree. `before` names a destination sibling; `None` appends it.
    pub fn move_task(&mut self, id: Id, parent: Option<Id>, before: Option<Id>) -> Result<()> {
        self.move_work(schema::TASK, id, parent, before)
    }

    /// Move a project and its subtree. `before` names a destination project; `None` appends it.
    pub fn move_project(&mut self, id: Id, parent: Option<Id>, before: Option<Id>) -> Result<()> {
        self.move_work(schema::PROJECT, id, parent, before)
    }

    fn move_work(
        &mut self,
        kind: &str,
        id: Id,
        parent: Option<Id>,
        before: Option<Id>,
    ) -> Result<()> {
        let tree = Tree::read(self);
        let Some(item) = tree.items.get(&id).filter(|i| i.note.type_name == kind) else {
            return Err(Error::Space(format!("Choose a {kind} to move")));
        };
        validate(self, id, "parent-id", parent.map(Value::Link).as_ref())?;
        if let Some(before) = before {
            if tree.items.get(&before).is_none_or(|i| {
                i.parent != parent
                    || i.note.is_task_occurrence()
                    || kind == schema::PROJECT && i.note.type_name != kind
            }) {
                return Err(Error::Space("Choose a sibling at the destination".into()));
            }
            if before == id {
                return Ok(());
            }
        }
        place(self, id, before)?;
        // The field setter commits both the move and the parent change together.
        if item.parent == parent {
            crate::space::commit(self.doc());
        } else if let Some(parent) = parent {
            self.set_field(id, "parent-id", &Value::Link(parent))?;
        } else {
            self.clear_field(id, "parent-id")?;
        }
        Ok(())
    }
}

fn place(space: &Space, id: Id, before: Option<Id>) -> Result<()> {
    let list = prepare(space)?;
    let positions = positions(space);
    let from = positions[&id];
    let to = before.map_or(list.len() - 1, |before| {
        let index = positions[&before];
        if from < index { index - 1 } else { index }
    });
    if from != to {
        list.mov(from, to)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subproject_reorders_preserve_hierarchy_content_and_task_order_after_reload() {
        let mut space = Space::create(Id::generate()).unwrap();
        let root = space.create_work(schema::PROJECT, "Root", None).unwrap();
        let a = space.create_work(schema::PROJECT, "Alpha", Some(root)).unwrap();
        let task = space.create_work(schema::TASK, "Task", Some(root)).unwrap();
        let b = space.create_work(schema::PROJECT, "Beta", Some(root)).unwrap();
        let other_task = space.create_work(schema::TASK, "Other task", Some(root)).unwrap();
        let c = space.create_work(schema::PROJECT, "Charlie", Some(root)).unwrap();
        let child = space.create_work(schema::TASK, "Nested", Some(c)).unwrap();
        let other = space.create_work(schema::PROJECT, "Other", None).unwrap();
        let unrelated = space.create_work(schema::PROJECT, "Unrelated", Some(other)).unwrap();
        let original = space.note(c).unwrap();
        space.reorder_project(c, Some(a)).unwrap();
        space.reorder_project(a, None).unwrap();
        assert!(space.reorder_project(c, Some(unrelated)).is_err());
        assert!(space.reorder_project(c, Some(task)).is_err());
        assert!(space.reorder_project(task, Some(a)).is_err());
        let reopened = Space::open(&space.snapshot().unwrap(), Id::generate()).unwrap();
        let tree = Tree::read(&reopened);
        let children = |kind| {
            tree.children(root)
                .into_iter()
                .filter(|id| tree.items[id].note.type_name == kind)
                .collect::<Vec<_>>()
        };
        assert_eq!(children(schema::PROJECT), [c, b, a]);
        assert_eq!(children(schema::TASK), [task, other_task]);
        assert_eq!(tree.children(c), [child]);
        assert_eq!(reopened.note(c).unwrap(), original);

        space.move_project(c, Some(b), None).unwrap();
        assert!(space.move_project(b, Some(c), None).is_err());
        assert!(space.move_project(c, Some(task), None).is_err());
        assert!(space.move_project(c, Some(root), Some(task)).is_err());
        let reopened = Space::open(&space.snapshot().unwrap(), Id::generate()).unwrap();
        let tree = Tree::read(&reopened);
        assert_eq!(tree.projects(child), [c, b, root]);
        assert_eq!(tree.children(b), [c]);
        assert_eq!(tree.items[&task].parent, Some(root));
    }

    #[test]
    fn task_order_and_subtree_moves_survive_reload_and_reject_cycles() {
        let mut space = Space::create(Id::generate()).unwrap();
        let project = space.create_work(schema::PROJECT, "Project", None).unwrap();
        let other = space.create_work(schema::PROJECT, "Other", None).unwrap();
        space.set_field(project, "due-on", &Value::Date("2026-12-01".into())).unwrap();
        space.set_field(other, "due-on", &Value::Date("2026-12-31".into())).unwrap();
        let z = space.create_work(schema::TASK, "Zulu", Some(project)).unwrap();
        let a = space.create_work(schema::TASK, "Alpha", Some(project)).unwrap();
        let b = space.create_work(schema::TASK, "Beta", Some(project)).unwrap();
        let child = space.create_work(schema::TASK, "Subtask", Some(z)).unwrap();
        assert_eq!(Tree::read(&space).children(project), [z, a, b]);
        space.move_task(b, Some(project), Some(z)).unwrap();
        assert_eq!(Tree::read(&space).children(project), [b, z, a]);
        space.move_task(z, Some(b), None).unwrap();
        let tree = Tree::read(&space);
        assert_eq!(tree.children(project), [b, a]);
        assert_eq!(tree.children(b), [z]);
        assert_eq!(tree.children(z), [child]);
        space.move_task(b, Some(other), None).unwrap();
        let tree = Tree::read(&space);
        assert_eq!(tree.context(child), other);
        assert_eq!(tree.date(child, "due-on").unwrap().value, "2026-12-31");
        assert!(space.move_task(b, Some(child), None).is_err());
        assert!(space.move_task(b, Some(b), None).is_err());
        assert!(space.move_task(project, Some(other), None).is_err());
        assert!(space.move_task(a, Some(project), Some(child)).is_err());
        space.move_task(z, Some(project), Some(a)).unwrap();
        space.set_body(z, "# Renamed\n\nPreserve details").unwrap();
        let reopened = Space::open(&space.snapshot().unwrap(), Id::generate()).unwrap();
        let tree = Tree::read(&reopened);
        assert_eq!(tree.children(project), [z, a]);
        assert_eq!(tree.children(z), [child]);
        assert_eq!(tree.date(child, "due-on").unwrap().value, "2026-12-01");
        assert_eq!(tree.items[&z].note.body, "# Renamed\n\nPreserve details");
    }

    #[test]
    fn concurrent_reorders_keep_both_moves_and_converge() {
        let mut first = Space::create(Id::generate()).unwrap();
        let project = first.create_work(schema::PROJECT, "Project", None).unwrap();
        let a = first.create_work(schema::TASK, "A", Some(project)).unwrap();
        let b = first.create_work(schema::TASK, "B", Some(project)).unwrap();
        let c = first.create_work(schema::TASK, "C", Some(project)).unwrap();
        let d = first.create_work(schema::TASK, "D", Some(project)).unwrap();
        let mut second = Space::open(&first.snapshot().unwrap(), Id::generate()).unwrap();
        first.move_task(d, Some(project), Some(a)).unwrap();
        second.move_task(b, Some(project), Some(a)).unwrap();
        first.doc().import(&second.snapshot().unwrap()).unwrap();
        second.doc().import(&first.snapshot().unwrap()).unwrap();
        let one = Tree::read(&first).children(project);
        assert_eq!(one, Tree::read(&second).children(project));
        assert_eq!(one.len(), 4);
        assert_eq!(one.iter().copied().collect::<BTreeSet<_>>(), [a, b, c, d].into());
        let at = |id| one.iter().position(|item| *item == id).unwrap();
        assert!(at(b) < at(a));
        assert!(at(d) < at(a));
    }

    #[test]
    fn concurrent_parent_changes_show_a_task_once_in_the_winning_parent() {
        let mut first = Space::create(Id::generate()).unwrap();
        let a = first.create_work(schema::PROJECT, "A", None).unwrap();
        let b = first.create_work(schema::PROJECT, "B", None).unwrap();
        let c = first.create_work(schema::PROJECT, "C", None).unwrap();
        let task = first.create_work(schema::TASK, "Task", Some(a)).unwrap();
        let child = first.create_work(schema::TASK, "Subtask", Some(task)).unwrap();
        let mut second = Space::open(&first.snapshot().unwrap(), Id::generate()).unwrap();
        first.move_task(task, Some(b), None).unwrap();
        second.move_task(task, Some(c), None).unwrap();
        first.doc().import(&second.snapshot().unwrap()).unwrap();
        second.doc().import(&first.snapshot().unwrap()).unwrap();
        let one = Tree::read(&first);
        let two = Tree::read(&second);
        assert_eq!(one.items[&task].parent, two.items[&task].parent);
        assert_eq!(one.children(task), [child]);
        let occurrences: Vec<_> =
            [a, b, c].into_iter().flat_map(|parent| one.children(parent)).collect();
        assert_eq!(occurrences, [task]);
    }

    #[test]
    fn legacy_tasks_and_concurrent_order_initialization_never_hide_items() {
        let mut first = Space::create(Id::generate()).unwrap();
        let project = first.create_work(schema::PROJECT, "Project", None).unwrap();
        let mut ids = Vec::new();
        // Creation deliberately differs from the legacy alphabetical fallback.
        for title in ["Zulu", "Beta", "Alpha"] {
            let id = first.create_note(schema::TASK, title).unwrap();
            first.set_field(id, "parent-id", &Value::Link(project)).unwrap();
            ids.push(id);
        }
        let [z, b, a] = ids[..] else { unreachable!() };
        assert_eq!(Tree::read(&first).children(project), [a, b, z]);
        let mut second = Space::open(&first.snapshot().unwrap(), Id::generate()).unwrap();
        first.move_task(z, Some(project), Some(a)).unwrap();
        second.move_task(b, Some(project), Some(a)).unwrap();
        first.doc().import(&second.snapshot().unwrap()).unwrap();
        second.doc().import(&first.snapshot().unwrap()).unwrap();
        let one = Tree::read(&first).children(project);
        assert_eq!(one, Tree::read(&second).children(project));
        assert_eq!(one.len(), 3);
        first.move_task(b, Some(project), Some(z)).unwrap();
        let order = Tree::read(&first).children(project);
        assert!(order.iter().position(|id| *id == b) < order.iter().position(|id| *id == z));
        let new = first.create_work(schema::TASK, "A new task", Some(project)).unwrap();
        assert_eq!(Tree::read(&first).children(project).last(), Some(&new));
        first.delete_note(a).unwrap();
        let order = Tree::read(&first).children(project);
        assert_eq!(order.len(), 3);
        assert!(!order.contains(&a));
    }
}
