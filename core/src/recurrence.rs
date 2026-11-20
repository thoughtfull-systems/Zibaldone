//! Recurring task definitions, stable occurrence notes, and once-only advancement.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{Duration, NaiveDate};
use loro::{ExportMode, LoroDoc, LoroMap, LoroValue, UpdateOptions, ValueOrContainer};
use pulldown_cmark::{Event, Parser};
use serde::{Deserialize, Serialize};

use crate::schema::{self, Value, child_map};
use crate::space::{Note, NoteMeta, commit, insert_note, note_body, note_map};
use crate::{Error, Id, Result, Space, body, work};

mod checklist;
mod rule;
pub use chrono::Weekday;
pub use rule::{Frequency, Mode, Pattern, Rule};

pub fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

const ACTIONS: &str = "recurrence-actions";
const BODY_SEEDS: &str = "recurrence-body-seeds";
const OFFSETS: &str = "recurrence-offsets";
const REVISIONS: &str = "recurrence-revisions";

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Revision {
    previous: Option<Id>,
    cutoff: NaiveDate,
    changed_on: Option<NaiveDate>,
    after: Option<Slot>,
    rule: Option<Rule>,
    template: String,
    author: Id,
    at: i64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    Completed,
    Skipped,
    Open,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Action {
    #[serde(rename = "completed-on", skip_serializing_if = "Option::is_none")]
    completed_on: Option<NaiveDate>,
    #[serde(rename = "skipped-on", skip_serializing_if = "Option::is_none")]
    skipped_on: Option<NaiveDate>,
    #[serde(skip_serializing)]
    outcome: Option<Outcome>,
    #[serde(skip_serializing)]
    day: Option<NaiveDate>,
    due: Option<String>,
    scheduled: Option<String>,
    next: Option<NaiveDate>,
    seen: Vec<Id>,
}

impl Action {
    fn outcome(&self) -> Outcome {
        if self.completed_on.is_some() {
            Outcome::Completed
        } else if self.skipped_on.is_some() {
            Outcome::Skipped
        } else {
            self.outcome.unwrap_or(Outcome::Open)
        }
    }

    fn closed_on(&self) -> Option<NaiveDate> {
        match self.outcome() {
            Outcome::Completed => self.completed_on.or(self.day),
            Outcome::Skipped => self.skipped_on.or(self.day),
            Outcome::Open => None,
        }
    }
}

#[derive(Default)]
struct Actions {
    entries: BTreeMap<Id, Action>,
}

impl Actions {
    fn first(&self) -> Option<&Action> {
        self.entries
            .iter()
            .filter(|(_, event)| event.seen.is_empty() && event.outcome() != Outcome::Open)
            .min_by_key(|(id, event)| (event.next.is_none(), event.next, *id))
            .map(|(_, event)| event)
    }

    fn heads(&self) -> impl Iterator<Item = (Id, &Action)> {
        let observed: BTreeSet<_> =
            self.entries.values().flat_map(|a| a.seen.iter().copied()).collect();
        self.entries.iter().filter(move |(id, _)| !observed.contains(id)).map(|(id, a)| (*id, a))
    }

    fn current(&self, mode: Mode) -> Option<&Action> {
        self.heads()
            .filter(|(_, a)| mode == Mode::Keep || !a.seen.is_empty())
            .max_by_key(|(id, event)| (event.outcome() == Outcome::Open, *id))
            .map(|(_, event)| event)
            .or_else(|| self.first())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Slot {
    id: Id,
    series: Id,
    revision: Id,
    key: String,
    day: NaiveDate,
    cycle: Option<u32>,
}

impl Slot {
    fn new(series: Id, revision: Id, day: NaiveDate, cycle: Option<u32>) -> Self {
        let key = cycle.map_or_else(|| day.to_string(), |number| number.to_string());
        let mut hash = blake3::Hasher::new_derive_key("Zibaldone task occurrence identifier");
        hash.update(series.as_bytes());
        hash.update(revision.as_bytes());
        hash.update(key.as_bytes());
        let mut bytes = [0; 16];
        bytes.copy_from_slice(&hash.finalize().as_bytes()[..16]);
        bytes[6] = (bytes[6] & 0x0f) | 0x80;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Self { id: Id::from_bytes(bytes), series, revision, key, day, cycle }
    }

    fn stored(note: &Note) -> Option<Self> {
        if !note.is_task_occurrence() {
            return None;
        }
        let field = |key| note.fields.get(key)?.as_string().map(|value| value.to_string());
        let day = field("original-on")?.parse().ok()?;
        let key = field("occurrence-slot")?;
        let slot = Self::new(
            field("series-id")?.parse().ok()?,
            field("rule-id")?.parse().ok()?,
            day,
            key.parse().ok(),
        );
        (note.id == slot.id).then_some(slot)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Occurrence {
    pub series: Id,
    pub day: NaiveDate,
    pub outcome: Outcome,
    pub superseded_on: Option<NaiveDate>,
    pub scheduled: Option<String>,
    pub due: Option<String>,
    pub closed: Option<String>,
}

struct Series {
    id: Id,
    revisions: BTreeMap<Id, Revision>,
    chain: Vec<Id>,
}

impl Series {
    fn read(space: &Space, id: Id) -> Self {
        let mut revisions: BTreeMap<Id, Revision> = records(space, id, REVISIONS);
        for (id, [scheduled, due]) in records::<[Option<i32>; 2]>(space, id, OFFSETS) {
            if let Some(rule) = revisions.get_mut(&id).and_then(|revision| revision.rule.as_mut()) {
                rule.scheduled = scheduled;
                rule.due = due;
            }
        }
        let chain = Self::chain(&revisions);
        Self { id, revisions, chain }
    }

    fn head(&self) -> Option<Id> {
        self.chain.last().copied()
    }

    fn chain(revisions: &BTreeMap<Id, Revision>) -> Vec<Id> {
        let mut result = Vec::new();
        while let Some((&id, _)) = revisions.iter().rev().find(|(id, revision)| {
            revision.previous == result.last().copied() && !result.contains(id)
        }) {
            result.push(id);
        }
        result
    }

    fn replacement(&self, mut id: Id) -> Option<&Revision> {
        let mut seen = BTreeSet::new();
        while seen.insert(id) {
            if let Some(index) = self.chain.iter().position(|&candidate| candidate == id) {
                return self.chain.get(index + 1).map(|next| &self.revisions[next]);
            }
            let Some(previous) = self.revisions.get(&id).and_then(|r| r.previous) else {
                return self.chain.first().map(|first| &self.revisions[first]);
            };
            id = previous;
        }
        None
    }

    fn supersession_date(&self, slot: &Slot) -> Option<NaiveDate> {
        if self.head().is_some_and(|head| {
            let revision = &self.revisions[&head];
            revision.rule.as_ref().is_some_and(|rule| rule.mode != Mode::Keep)
                && revision.after.as_ref().is_some_and(|after| after.id == slot.id)
        }) {
            return None;
        }
        let replacement = self.replacement(slot.revision)?;
        Some(replacement.changed_on.unwrap_or_else(|| {
            chrono::DateTime::from_timestamp_millis(replacement.at)
                .map_or(replacement.cutoff, |at| at.date_naive())
        }))
    }

    fn slots(&self, space: &Space, today: NaiveDate) -> Vec<Slot> {
        let Some(id) = self.head() else { return Vec::new() };
        let revision = &self.revisions[&id];
        let Some(rule) = &revision.rule else { return Vec::new() };
        let Ok(pattern) = rule.compile() else { return Vec::new() };
        let mut slots = Vec::new();
        if rule.mode == Mode::Keep {
            let lead = i64::from(rule.scheduled.unwrap_or(0)).min(0);
            let horizon = today.checked_sub_signed(Duration::days(lead)).unwrap_or(NaiveDate::MAX);
            let mut open = false;
            for day in pattern.dates(rule.start).skip_while(|day| *day < revision.cutoff) {
                let slot = Slot::new(self.id, id, day, None);
                // Keep notes already created by an earlier close, date, or offset.
                if day > horizon && space.note_map(slot.id).is_err() {
                    if !open {
                        slots.push(slot);
                    }
                    break;
                }
                open = open
                    || Actions { entries: records(space, slot.id, ACTIONS) }
                        .current(Mode::Keep)
                        .is_none_or(|event| event.outcome() == Outcome::Open);
                slots.push(slot);
            }
        } else {
            let successor = |slot: &Slot| {
                Actions { entries: records(space, slot.id, ACTIONS) }
                    .first()
                    .and_then(|event| event.next)
                    .map(|day| (slot.cycle.unwrap() + 1, day))
            };
            let mut next = if let Some(slot) = &revision.after {
                slots.push(slot.clone());
                successor(slot)
            } else {
                rule.first().ok().map(|day| (1, day))
            };
            while let Some((cycle, day)) =
                next.filter(|(number, day)| pattern.admits(*number, *day))
            {
                let slot = Slot::new(self.id, id, day, Some(cycle));
                next = successor(&slot);
                slots.push(slot);
            }
        }
        slots
    }
}

fn records<T: serde::de::DeserializeOwned>(space: &Space, id: Id, name: &str) -> BTreeMap<Id, T> {
    let Ok(note) = space.note_map(id) else { return BTreeMap::new() };
    let Some(ValueOrContainer::Container(container)) = note.get(name) else {
        return BTreeMap::new();
    };
    let Ok(map) = container.into_map() else { return BTreeMap::new() };
    map.keys()
        .filter_map(|key| {
            let id = key.parse().ok()?;
            let value = map.get(&key)?.get_deep_value();
            Some((id, serde_json::from_str(value.as_string()?).ok()?))
        })
        .collect()
}

fn record<T: Serialize>(map: &LoroMap, id: Id, value: &T) -> Result<()> {
    let json = serde_json::to_string(value).map_err(|error| Error::Space(error.to_string()))?;
    map.insert(&id.to_string(), json)?;
    Ok(())
}

fn template(source: &str) -> String {
    let mut result = source.to_owned();
    for (event, range) in Parser::new_ext(source, body::options())
        .into_offset_iter()
        .filter(|(event, _)| matches!(event, Event::TaskListMarker(true)))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        let _ = event;
        result.replace_range(range, "[ ]");
    }
    result
}

fn mark_series(fields: &LoroMap) -> Result<bool> {
    if matches!(fields.get("recurrence"), Some(ValueOrContainer::Value(LoroValue::Bool(true)))) {
        return Ok(false);
    }
    fields.insert("recurrence", true)?;
    Ok(true)
}

fn seed_peer(hash: [u8; 32]) -> u64 {
    let peer = u64::from_be_bytes(hash[..8].try_into().unwrap()) >> 1 | 1 << 63;
    peer.min(u64::MAX - 1)
}

fn insert_occurrence(space: &Space, series: &Series, slot: &Slot) -> Result<bool> {
    if space.note_map(slot.id).is_ok() || space.tombstone(slot.id).is_some() {
        return Ok(false);
    }
    let revision = &series.revisions[&slot.revision];
    // Replicas share the note containers and the rule revision's baseline text operations.
    let seed = LoroDoc::new();
    seed.set_peer_id(seed_peer(blake3::derive_key(
        "Zibaldone occurrence seed writer",
        slot.id.as_bytes(),
    )))?;
    let baseline = template(&revision.template);
    insert_note(
        &seed,
        slot.id,
        schema::TASK,
        &baseline,
        &[
            ("occurrence-slot", Value::Text(slot.key.clone())),
            // A sequential occurrence can have different provisional dates on different devices.
            (
                "original-on",
                Value::Date(
                    slot.cycle
                        .map_or(slot.day, |_| revision.rule.as_ref().unwrap().start)
                        .to_string(),
                ),
            ),
            ("rule-id", Value::Text(slot.revision.to_string())),
            ("series-id", Value::Link(series.id)),
        ],
        &NoteMeta {
            created_at: revision.at,
            created_by: Some(revision.author),
            updated_at: revision.at,
            updated_by: Some(revision.author),
        },
    )?;
    commit(&seed);
    let current = template(&space.body(series.id)?.to_string());
    if current != baseline {
        // Identical initial bodies share operation IDs. Different bodies use different writers
        // and merge as concurrent edits against the same baseline.
        let digest = blake3::hash(current.as_bytes());
        let mut writer = blake3::Hasher::new_derive_key("Zibaldone occurrence body seed writer");
        writer.update(slot.id.as_bytes());
        writer.update(digest.as_bytes());
        seed.set_peer_id(seed_peer(*writer.finalize().as_bytes()))?;
        note_body(&seed, slot.id)?
            .update(&current, UpdateOptions::default())
            .map_err(|error| Error::Loro(format!("{error:?}")))?;
        note_map(&seed, slot.id)?
            .ensure_mergeable_map(BODY_SEEDS)?
            .insert(digest.to_hex().as_str(), true)?;
        commit(&seed);
    }
    space.doc().import(&seed.export(ExportMode::all_updates())?)?;
    Ok(true)
}

pub(crate) fn project(space: &Space, tree: &mut work::Tree, today: NaiveDate) {
    let series: BTreeMap<_, _> = tree
        .items
        .iter()
        .filter(|(_, item)| item.note.type_name == schema::TASK && !item.note.is_task_occurrence())
        .map(|(id, _)| (*id, Series::read(space, *id)))
        .filter(|(_, series)| series.head().is_some())
        .collect();
    for (&id, series) in &series {
        let head = series.head().unwrap();
        tree.series.insert(id);
        if let Some(rule) = &series.revisions[&head].rule {
            tree.recurrence_rules.insert(id, rule.clone());
        }
    }
    let slots: BTreeMap<_, _> =
        series.values().flat_map(|s| s.slots(space, today)).map(|slot| (slot.id, slot)).collect();
    read_occurrences(space, tree, &series, &slots);
    tree.finish_hierarchy();
    // Resolve inherited dates once the full hierarchy includes occurrence parents.
    let dates: Vec<_> = tree
        .occurrences
        .keys()
        .map(|&id| {
            (
                id,
                tree.date(id, "due-on").map(|date| date.value),
                tree.date(id, "scheduled-on").map(|date| date.value),
            )
        })
        .collect();
    for (id, due, scheduled) in dates {
        let occurrence = tree.occurrences.get_mut(&id).unwrap();
        occurrence.due = due;
        occurrence.scheduled = scheduled;
    }
    tree.current_occurrences = current_occurrences(&tree.occurrences, &series, &slots, today);
}

fn read_occurrences(
    space: &Space,
    tree: &mut work::Tree,
    series: &BTreeMap<Id, Series>,
    slots: &BTreeMap<Id, Slot>,
) {
    for (&id, item) in &tree.items {
        let Some(stored) = Slot::stored(&item.note) else { continue };
        let Some(series) = series.get(&stored.series) else { continue };
        let Some(revision) = series.revisions.get(&stored.revision) else { continue };
        let Some(rule) = &revision.rule else { continue };
        let actions = Actions { entries: records(space, id, ACTIONS) };
        let current = actions.current(rule.mode);
        let slot = slots.get(&id).unwrap_or(&stored);
        // A detached sequential row keeps the date its creating proposal assigned.
        let detached_day = item.explicit_date("realized-on").and_then(|date| date.parse().ok());
        let day = if slots.contains_key(&id) { slot.day } else { detached_day.unwrap_or(slot.day) };
        let date = |field, offset: Option<i32>| {
            item.explicit_date(field).or_else(|| {
                offset.and_then(|days| {
                    day.checked_add_signed(Duration::days(i64::from(days)))
                        .map(|value| value.to_string())
                })
            })
        };
        let outcome = current.map_or(Outcome::Open, Action::outcome);
        let superseded_on = if outcome == Outcome::Open && !slots.contains_key(&id) {
            item.explicit_date("superseded-on").and_then(|day| day.parse().ok())
        } else {
            None
        };
        let closed = current.filter(|a| a.outcome() != Outcome::Open);
        tree.occurrences.insert(
            id,
            Occurrence {
                series: series.id,
                day,
                outcome,
                superseded_on,
                scheduled: closed
                    .map_or_else(|| date("scheduled-on", rule.scheduled), |a| a.scheduled.clone()),
                due: closed.map_or_else(|| date("due-on", rule.due), |a| a.due.clone()),
                closed: closed.and_then(Action::closed_on).map(|day| day.to_string()),
            },
        );
    }
}

fn current_occurrences(
    occurrences: &BTreeMap<Id, Occurrence>,
    series: &BTreeMap<Id, Series>,
    slots: &BTreeMap<Id, Slot>,
    today: NaiveDate,
) -> BTreeMap<Id, Id> {
    let mut best: BTreeMap<Id, &Slot> = BTreeMap::new();
    let today = today.to_string();
    let calendar_priority = |slot: &Slot| {
        let row = &occurrences[&slot.id];
        (row.scheduled.as_ref().is_some_and(|day| day > &today), slot.day, slot.id)
    };
    for slot in slots.values() {
        let book = &series[&slot.series];
        let Some(rule) = book.head().and_then(|head| book.revisions[&head].rule.as_ref()) else {
            continue;
        };
        let current = best.get(&slot.series).copied();
        let preferred = if rule.mode == Mode::Keep {
            slot.cycle.is_none()
                && occurrences.get(&slot.id).is_some_and(|row| row.outcome == Outcome::Open)
                && current.is_none_or(|other| calendar_priority(slot) < calendar_priority(other))
        } else {
            slot.cycle.is_some() && current.is_none_or(|other| slot.cycle > other.cycle)
        };
        if preferred {
            best.insert(slot.series, slot);
        }
    }
    best.into_iter()
        .filter_map(|(series, slot)| {
            let row = occurrences.get(&slot.id)?;
            (row.outcome == Outcome::Open).then_some((series, slot.id))
        })
        .collect()
}

impl Space {
    pub fn recurrence_rule(&self, series: Id) -> Option<Rule> {
        let series = Series::read(self, series);
        series.revisions.get(&series.head()?)?.rule.clone()
    }

    pub fn set_recurrence(&mut self, id: Id, rule: Option<Rule>, today: NaiveDate) -> Result<()> {
        let note = self.note(id)?;
        if note.type_name != schema::TASK || note.is_task_occurrence() {
            return Err(Error::Space("Choose an ordinary task or series to set recurrence".into()));
        }
        let series = Series::read(self, id);
        let cutoff = if let Some(rule) = &rule {
            rule.first()?;
            if let Some((id, old)) =
                series.head().and_then(|head| Some((head, series.revisions[&head].rule.as_ref()?)))
                && old.mode == rule.mode
                && old.start == rule.start
                && old.pattern == rule.pattern
            {
                let offsets = [rule.scheduled, rule.due];
                if offsets != [old.scheduled, old.due] {
                    let map = self.note_map(note.id)?.ensure_mergeable_map(OFFSETS)?;
                    record(&map, id, &offsets)?;
                    commit(self.doc());
                    self.realize_recurrences(today)?;
                }
                return Ok(());
            }
            if series.head().is_some() && rule.start < today {
                return Err(Error::Space("Choose today or a future date for the new rule".into()));
            }
            rule.start
        } else if self.recurrence_rule(id).is_some() {
            today
        } else {
            return Ok(());
        };
        let revision = Revision {
            previous: series.head(),
            cutoff,
            changed_on: Some(today),
            after: None,
            rule,
            template: note.body,
            author: self.machine(),
            at: crate::time::now(),
        };
        self.write_recurrence(id, &revision, today)
    }

    fn write_recurrence(&mut self, id: Id, revision: &Revision, today: NaiveDate) -> Result<()> {
        let note = self.note_map(id)?;
        let map = note.ensure_mergeable_map(REVISIONS)?;
        record(&map, Id::generate(), revision)?;
        mark_series(&child_map(&note, "fields")?)?;
        commit(self.doc());
        self.realize_recurrences(today)?;
        Ok(())
    }

    pub(crate) fn has_recurrence_history(&self, id: Id) -> bool {
        !Series::read(self, id).revisions.is_empty()
    }

    /// Catch up finite schedules after a clock change or a sync.
    pub fn realize_recurrences(&mut self, today: NaiveDate) -> Result<bool> {
        let mut changed = schema::upgrade_draft_work_schema(self.doc())?;
        let mut series_by_id = BTreeMap::new();
        let mut occurrences = Vec::new();
        for id in self.note_ids() {
            let Ok(note) = self.note_map(id) else { continue };
            let Some(ValueOrContainer::Value(LoroValue::String(kind))) = note.get("type") else {
                continue;
            };
            if kind.as_str() != schema::TASK {
                continue;
            }
            let Ok(fields) = child_map(&note, "fields") else { continue };
            if fields.get("series-id").is_some() {
                occurrences.push(id);
                continue;
            }
            let series = Series::read(self, id);
            if series.head().is_none() {
                continue;
            }
            changed |= mark_series(&fields)?;
            for slot in series.slots(self, today) {
                if insert_occurrence(self, &series, &slot)? {
                    if slot.cycle.is_some() {
                        let fields = child_map(&self.note_map(slot.id)?, "fields")?;
                        fields.insert("realized-on", slot.day.to_string())?;
                    }
                    changed = true;
                }
            }
            series_by_id.insert(id, series);
        }
        for id in occurrences {
            changed |= checklist::reconcile(self, id)?;
            let note = self.note(id)?;
            let Some(slot) = Slot::stored(&note) else { continue };
            let Some(series) = series_by_id.get(&slot.series) else { continue };
            let Some(rule) = series.revisions.get(&slot.revision).and_then(|r| r.rule.as_ref())
            else {
                continue;
            };
            let superseded = series.supersession_date(&slot).filter(|_| {
                let actions = Actions { entries: records(self, id, ACTIONS) };
                actions.current(rule.mode).is_none_or(|a| a.outcome() == Outcome::Open)
            });
            let desired = superseded.map(|date| LoroValue::from(date.to_string()));
            let stored = note.fields.get("superseded-on");
            if stored == desired.as_ref() {
                continue;
            }
            let fields = child_map(&self.note_map(id)?, "fields")?;
            if let Some(date) = desired {
                fields.insert("superseded-on", date)?;
            } else {
                fields.delete("superseded-on")?;
            }
            changed = true;
        }
        if changed {
            commit(self.doc());
        }
        Ok(changed)
    }

    pub fn record_occurrence(
        &mut self,
        id: Id,
        outcome: Outcome,
        day: NaiveDate,
        comment: &str,
    ) -> Result<()> {
        let note = self.note(id)?;
        let Some(stored) = Slot::stored(&note) else {
            return Err(Error::Space("Expected a recurring occurrence".into()));
        };
        let series = Series::read(self, stored.series);
        let Some(rule) = series.revisions.get(&stored.revision).and_then(|r| r.rule.as_ref())
        else {
            return Err(Error::Space("The recurrence is unavailable".into()));
        };
        let tree = work::Tree::read_on(self, day);
        let occurrence = tree
            .occurrences
            .get(&id)
            .filter(|item| {
                item.outcome != Outcome::Open || series.supersession_date(&stored).is_none()
            })
            .ok_or_else(|| Error::Space("This occurrence is no longer active".into()))?;
        if occurrence.outcome == outcome
            || occurrence.outcome != Outcome::Open && outcome != Outcome::Open
        {
            return Err(Error::Space("This occurrence's state has changed".into()));
        }
        let actions = Actions { entries: records(self, id, ACTIONS) };
        let mut next = None;
        if actions.entries.is_empty()
            && outcome != Outcome::Open
            && series.head().is_some_and(|head| {
                head == stored.revision
                    || series.revisions[&head].after.as_ref().is_some_and(|s| s.id == id)
            })
        {
            let pattern = rule.compile()?;
            next = match rule.mode {
                Mode::Keep => None,
                Mode::Jump => pattern.dates(rule.start).find(|d| *d > occurrence.day && *d >= day),
                Mode::Interval => pattern.after(day),
            };
        }
        self.insert_occurrence_comment(id, outcome, day, comment)?;
        record(
            &self.note_map(id)?.ensure_mergeable_map(ACTIONS)?,
            Id::generate(),
            &Action {
                completed_on: (outcome == Outcome::Completed).then_some(day),
                skipped_on: (outcome == Outcome::Skipped).then_some(day),
                outcome: None,
                day: None,
                due: occurrence.due.clone(),
                scheduled: occurrence.scheduled.clone(),
                next,
                seen: actions.heads().map(|(id, _)| id).collect(),
            },
        )?;
        self.note_edited(id)?;
        self.realize_recurrences(day)?;
        Ok(())
    }
    pub fn add_occurrence_comment(&mut self, id: Id, day: NaiveDate, comment: &str) -> Result<()> {
        if Slot::stored(&self.note(id)?).is_none() {
            return Err(Error::Space("Expected a recurring occurrence".into()));
        }
        self.insert_occurrence_comment(id, Outcome::Open, day, comment)?;
        self.note_edited(id)
    }

    fn insert_occurrence_comment(
        &self,
        id: Id,
        outcome: Outcome,
        day: NaiveDate,
        comment: &str,
    ) -> Result<()> {
        if !comment.trim().is_empty() {
            let label = match outcome {
                Outcome::Completed => "Completed",
                Outcome::Skipped => "Skipped",
                Outcome::Open => "Note",
            };
            let body = self.body(id)?;
            let text = format!("\n\n**{label} {day}** — {}\n", comment.trim());
            body.insert(body.len_unicode(), &text)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(day: &str) -> NaiveDate {
        day.parse().unwrap()
    }

    fn rule(mode: Mode, pattern: &str) -> Rule {
        Rule {
            mode,
            pattern: pattern.into(),
            start: date("2026-11-02"),
            scheduled: Some(0),
            due: Some(2),
        }
    }

    #[test]
    fn calendar_patterns_omit_invalid_dates_and_honor_bounds() {
        let mut rule = rule(Mode::Keep, "FREQ=MONTHLY;BYMONTHDAY=31;COUNT=3");
        assert_eq!(rule.preview().unwrap(), ["2026-12-31", "2027-01-31", "2027-03-31"].map(date));
        rule.pattern = "FREQ=YEARLY;BYMONTH=11;BYDAY=-1MO;UNTIL=20271129".into();
        assert_eq!(rule.preview().unwrap(), ["2026-11-30", "2027-11-29"].map(date));
    }

    #[test]
    fn keeping_monthly_occurrences_waits_to_create_the_next_open_note() {
        let mut space = Space::create(Id::generate()).unwrap();
        let series = space.create_work(schema::TASK, "Monthly report", None).unwrap();
        let first_day = date("2026-11-19");
        let second_day = date("2026-12-19");
        let rule = Rule { start: first_day, ..rule(Mode::Keep, "FREQ=MONTHLY;UNTIL=20261231") };
        assert_eq!(rule.preview().unwrap(), [first_day, second_day]);
        space.set_recurrence(series, Some(rule), first_day).unwrap();
        let tree = work::Tree::read_on(&space, first_day);
        assert_eq!(tree.occurrences.len(), 1);
        let first = tree.current_occurrences[&series];
        assert_eq!(tree.occurrences[&first].day, first_day);
        assert!(!space.realize_recurrences(date("2026-12-18")).unwrap());
        space.set_body(series, "Updated monthly report").unwrap();
        assert_eq!(work::Tree::read_on(&space, first_day).occurrences.len(), 1);

        space.record_occurrence(first, Outcome::Completed, first_day, "").unwrap();
        let tree = work::Tree::read_on(&space, first_day);
        let second = tree.current_occurrences[&series];
        assert_eq!(tree.occurrences[&second].day, second_day);
        assert_eq!(tree.occurrences[&second].scheduled.as_deref(), Some("2026-12-19"));
        assert_eq!(space.note(first).unwrap().body, "Monthly report");
        assert_eq!(space.note(second).unwrap().body, "Updated monthly report");
        space.record_occurrence(first, Outcome::Open, first_day, "").unwrap();
        let tree = work::Tree::read_on(&space, first_day);
        assert_eq!(tree.current_occurrences[&series], first);
        assert_eq!(tree.occurrences.len(), 2);
        assert!(tree.occurrences[&second].superseded_on.is_none());
    }

    #[test]
    fn keep_catches_up_unfinished_occurrences_when_their_scheduled_dates_arrive() {
        let mut space = Space::create(Id::generate()).unwrap();
        let series = space.create_work(schema::TASK, "Weekly report", None).unwrap();
        let first_day = date("2026-11-02");
        let rule = Rule { scheduled: Some(-2), ..rule(Mode::Keep, "FREQ=WEEKLY") };
        space.set_recurrence(series, Some(rule), first_day).unwrap();
        assert!(!space.realize_recurrences(date("2026-11-06")).unwrap());

        let return_day = date("2026-11-14");
        assert!(space.realize_recurrences(return_day).unwrap());
        let tree = work::Tree::read_on(&space, return_day);
        let dates: BTreeSet<_> = tree
            .occurrences
            .values()
            .filter(|row| row.outcome == Outcome::Open && row.superseded_on.is_none())
            .map(|row| row.day)
            .collect();
        assert_eq!(tree.occurrences.len(), 3);
        assert_eq!(dates, BTreeSet::from([first_day, date("2026-11-09"), date("2026-11-16")]));
    }

    #[test]
    fn jumping_reopens_without_moving_the_successor() {
        let mut space = Space::create(Id::generate()).unwrap();
        let series = space.create_work(schema::TASK, "# Report", None).unwrap();
        let rule = rule(Mode::Jump, "FREQ=WEEKLY;COUNT=3");
        space.set_recurrence(series, Some(rule), date("2026-11-02")).unwrap();
        let revision = Series::read(&space, series).head().unwrap();
        let first = Slot::new(series, revision, date("2026-11-02"), Some(1)).id;
        space.record_occurrence(first, Outcome::Completed, date("2026-11-12"), "Done").unwrap();
        let second = Slot::new(series, revision, date("2026-11-16"), Some(2)).id;
        space.record_occurrence(first, Outcome::Open, date("2026-11-13"), "").unwrap();
        let reopened = work::Tree::read_on(&space, date("2026-11-13"));
        assert_eq!(reopened.current_occurrences[&series], second);
        assert_eq!(reopened.children(series), []);
        space.record_occurrence(first, Outcome::Completed, date("2026-11-18"), "").unwrap();
        let tree = work::Tree::read_on(&space, date("2026-11-18"));
        assert_eq!(tree.occurrences[&second].day, date("2026-11-16"));
        assert!(tree.items[&first].note.body.contains("**Completed 2026-11-12** — Done"));
        assert_eq!(tree.occurrences.len(), 2);
    }

    #[test]
    fn replicas_seed_one_checklist_and_merge_distinct_edits() {
        let mut a = Space::create(Id::generate()).unwrap();
        let series = a.create_work(schema::TASK, "# Report", None).unwrap();
        a.set_recurrence(series, Some(rule(Mode::Keep, "FREQ=DAILY")), date("2026-11-02")).unwrap();
        a.set_body(series, "# Report\n\n- [x] One\n- [ ] Two").unwrap();
        a.note_edited(series).unwrap();
        let mut b = Space::open(&a.snapshot().unwrap(), Id::generate()).unwrap();
        let day = date("2026-11-04");
        a.realize_recurrences(day).unwrap();
        b.realize_recurrences(day).unwrap();
        let revision = Series::read(&a, series).head().unwrap();
        let slot = Slot::new(series, revision, day, None).id;
        assert_eq!(a.note(slot).unwrap().body, b.note(slot).unwrap().body);
        let clean = a.note(slot).unwrap().body;
        assert!(clean.contains("- [ ] One"));
        a.set_body(slot, &clean.replace("[ ] One", "[x] One")).unwrap();
        b.set_body(slot, &clean.replace("[ ] Two", "[x] Two")).unwrap();
        a.note_edited(slot).unwrap();
        b.note_edited(slot).unwrap();
        let a_snapshot = a.snapshot().unwrap();
        a.doc().import(&b.snapshot().unwrap()).unwrap();
        b.doc().import(&a_snapshot).unwrap();
        let expected = clean.replace("[ ] One", "[x] One").replace("[ ] Two", "[x] Two");
        assert_eq!(a.note(slot).unwrap().body, expected);
        assert_eq!(b.note(slot).unwrap().body, expected);
    }

    #[test]
    fn replicas_merge_distinct_starting_bodies_and_occurrence_edits() {
        let mut a = Space::create(Id::generate()).unwrap();
        let original = "# Report\n\n- [ ] One\n- [ ] Two";
        let series = a.create_work(schema::TASK, original, None).unwrap();
        let day = date("2026-11-02");
        a.set_recurrence(series, Some(rule(Mode::Keep, "FREQ=DAILY")), day).unwrap();
        let mut b = Space::open(&a.snapshot().unwrap(), Id::generate()).unwrap();
        let revised_title = original.replace("# Report", "# Weekly report");
        let revised_footer = format!("{original}\n\nPack up.");
        let tomorrow = day.succ_opt().unwrap();
        let revision = Series::read(&a, series).head().unwrap();
        let slot = Slot::new(series, revision, tomorrow, None).id;
        for (space, starting_body, label) in
            [(&mut a, revised_title.as_str(), "One"), (&mut b, revised_footer.as_str(), "Two")]
        {
            space.set_body(series, starting_body).unwrap();
            space.realize_recurrences(tomorrow).unwrap();
            assert_eq!(space.note(slot).unwrap().body, starting_body);
            let edited = starting_body.replace(&format!("[ ] {label}"), &format!("[x] {label}"));
            space.set_body(slot, &edited).unwrap();
            space.note_edited(slot).unwrap();
        }
        let snapshot = a.snapshot().unwrap();
        a.doc().import(&b.snapshot().unwrap()).unwrap();
        b.doc().import(&snapshot).unwrap();
        let expected = "# Weekly report\n\n- [x] One\n- [x] Two\n\nPack up.";
        assert_eq!(a.note(slot).unwrap().body, expected);
        assert_eq!(b.note(slot).unwrap().body, expected);
    }

    #[test]
    fn concurrent_closes_converge_on_the_earlier_successor_and_keep_both_notes() {
        let mut a = Space::create(Id::generate()).unwrap();
        let series = a.create_work(schema::TASK, "# Water plants", None).unwrap();
        let rule = rule(Mode::Interval, "FREQ=WEEKLY;COUNT=2");
        a.set_recurrence(series, Some(rule), date("2026-11-02")).unwrap();
        let mut b = Space::open(&a.snapshot().unwrap(), Id::generate()).unwrap();
        let revision = Series::read(&a, series).head().unwrap();
        let first = Slot::new(series, revision, date("2026-11-02"), Some(1)).id;
        a.record_occurrence(first, Outcome::Completed, date("2026-11-10"), "Watered upstairs")
            .unwrap();
        b.record_occurrence(first, Outcome::Skipped, date("2026-11-12"), "Rain outside").unwrap();
        let second = Slot::new(series, revision, date("2026-11-17"), Some(2)).id;
        b.set_field(second, "due-on", &Value::Date("2026-11-23".into())).unwrap();
        let snapshot = a.snapshot().unwrap();
        a.doc().import(&b.snapshot().unwrap()).unwrap();
        b.doc().import(&snapshot).unwrap();
        for space in [&a, &b] {
            let tree = work::Tree::read_on(space, date("2026-11-12"));
            assert_eq!(tree.occurrences[&first].outcome, Outcome::Completed);
            assert_eq!(tree.occurrences[&second].day, date("2026-11-17"));
            assert_eq!(tree.occurrences[&second].due.as_deref(), Some("2026-11-23"));
            assert_eq!(tree.occurrences.len(), 2);
            let body = space.note(first).unwrap().body;
            assert!(body.contains("Watered upstairs") && body.contains("Rain outside"));
        }
    }

    #[test]
    fn a_series_body_edit_automatically_seeds_the_successor() {
        let mut space = Space::create(Id::generate()).unwrap();
        let series = space.create_work(schema::TASK, "# First", None).unwrap();
        let day = date("2026-11-02");
        space.set_recurrence(series, Some(rule(Mode::Interval, "FREQ=DAILY")), day).unwrap();
        let first = *work::Tree::read_on(&space, day).occurrences.keys().next().unwrap();
        space.set_body(series, "# Revised").unwrap();
        assert_eq!(work::Tree::read_on(&space, day).occurrences[&first].outcome, Outcome::Open);
        space.record_occurrence(first, Outcome::Completed, day, "").unwrap();
        let tree = work::Tree::read_on(&space, day);
        let second = tree.occurrences.keys().copied().find(|id| *id != first).unwrap();
        assert!(space.note(first).unwrap().body.starts_with("# First"));
        assert!(space.note(second).unwrap().body.starts_with("# Revised"));
        assert_eq!(tree.occurrences[&second].day, date("2026-11-03"));
    }

    #[test]
    fn occurrence_history_is_searchable_and_manual_backlinks_work() {
        use crate::search::{Index, Sort, search_indexes};

        let mut space = Space::create(Id::generate()).unwrap();
        let series = space.create_work(schema::TASK, "# Réview", None).unwrap();
        let day = date("2026-11-02");
        space.set_recurrence(series, Some(rule(Mode::Keep, "FREQ=DAILY;COUNT=1")), day).unwrap();
        let id = *work::Tree::read_on(&space, day).occurrences.keys().next().unwrap();
        assert_eq!(space.note(id).unwrap().body, "# Réview");
        space.record_occurrence(id, Outcome::Skipped, day, "Missing invoice").unwrap();
        let mut index = Index::in_memory().unwrap();
        index.rebuild(&space, &|_| None).unwrap();
        assert_eq!(index.backlinks(series).unwrap(), []);
        let hits = search_indexes(
            [(space.id().unwrap(), &index)],
            &format!("@series-id={series} @skipped-on? invoice"),
            Sort::Recurrence,
            10,
        )
        .unwrap();
        assert_eq!(hits[0].1.id, id);
        assert_eq!(hits.len(), 1);
        space.set_body(id, &format!("# Missing invoice\n\n[[{series}]]")).unwrap();
        index.update_note(&space, id, &|_| None).unwrap();
        assert_eq!(index.backlinks(series).unwrap(), [id]);
        assert_eq!(space.delete_note(series).unwrap(), [series, id]);
        assert!(space.tombstone(id).is_some());
    }

    #[test]
    fn a_new_rule_supersedes_open_occurrences_and_preserves_closed_history() {
        use crate::search::{Index, Sort, search_indexes};

        let mut space = Space::create(Id::generate()).unwrap();
        let task = space.create_work(schema::TASK, "Ordinary task", None).unwrap();
        let series = space.create_work(schema::TASK, "Recurring task", None).unwrap();
        let day = date("2026-11-02");
        let change_day = date("2026-11-05");
        space.set_recurrence(series, Some(rule(Mode::Keep, "FREQ=DAILY")), day).unwrap();
        let tree = work::Tree::read_on(&space, day);
        let first = tree.current_occurrences[&series];
        assert!(space.note(first).unwrap().is_task_occurrence());
        assert!(space.clear_field(first, "series-id").is_err());
        assert!(space.set_field(task, "series-id", &Value::Link(series)).is_err());
        assert!(space.set_field(first, "superseded-on", &Value::Date(day.to_string())).is_err());
        assert!(space.set_work_status(first, Some("waiting"), "A reply").is_err());
        assert!(space.set_recurrence(first, Some(rule(Mode::Keep, "FREQ=DAILY")), day).is_err());
        assert!(space.delete_note(first).is_err());

        space.record_occurrence(first, Outcome::Completed, day, "").unwrap();
        let second_day = day.succ_opt().unwrap();
        let skipped = work::Tree::read_on(&space, day).current_occurrences[&series];
        space.record_occurrence(skipped, Outcome::Skipped, second_day, "").unwrap();
        space.realize_recurrences(change_day).unwrap();
        let old = work::Tree::read_on(&space, change_day);
        let backlog = old.current_occurrences[&series];
        let old_open: BTreeSet<_> = old
            .occurrences
            .iter()
            .filter(|(_, row)| row.outcome == Outcome::Open)
            .map(|(&id, _)| id)
            .collect();
        assert_eq!(old_open.len(), 2);
        space.set_body(backlog, "Edited backlog").unwrap();
        let start = change_day.succ_opt().unwrap();
        let next = Rule { start, scheduled: None, ..rule(Mode::Keep, "FREQ=DAILY;COUNT=1") };
        space.set_recurrence(series, Some(next), change_day).unwrap();
        let tree = work::Tree::read_on(&space, change_day);
        let current = tree.current_occurrences[&series];
        for &id in &old_open {
            assert_eq!(tree.occurrences[&id].superseded_on, Some(change_day));
            assert_eq!(space.note(id).unwrap().fields["superseded-on"], "2026-11-05".into());
        }
        assert_eq!(space.note(backlog).unwrap().body, "Edited backlog");
        assert!(space.record_occurrence(backlog, Outcome::Completed, change_day, "").is_err());
        assert_eq!(tree.occurrences[&first].outcome, Outcome::Completed);
        assert_eq!(tree.occurrences[&skipped].outcome, Outcome::Skipped);

        let mut index = Index::in_memory().unwrap();
        index.rebuild(&space, &|_| None).unwrap();
        let find = |query| {
            search_indexes([(space.id().unwrap(), &index)], query, Sort::Title, 10)
                .unwrap()
                .into_iter()
                .map(|(_, hit)| hit.id)
                .collect::<BTreeSet<_>>()
        };
        assert_eq!(find("@superseded-on=2026-11-05"), old_open);
        assert_eq!(find("@completed-on?"), BTreeSet::from([first]));
        assert_eq!(find("@skipped-on?"), BTreeSet::from([skipped]));
        assert_eq!(find("@recurrence?"), BTreeSet::from([series]));
        assert_eq!(find("@recurrence=true"), BTreeSet::from([series]));
        assert_eq!(find(crate::work::WORK_QUERY), BTreeSet::from([task, current]));
        let fields = crate::search::field_names([&index]);
        assert!(fields.iter().any(|field| field == "recurrence"));
        assert!(!fields.iter().any(|field| field == "actionable"));
        assert_eq!(
            find(&format!("@type=task @series-id={series}")),
            old_open.union(&BTreeSet::from([first, skipped, current])).copied().collect()
        );

        space.record_occurrence(first, Outcome::Open, change_day, "").unwrap();
        space.set_recurrence(series, None, start).unwrap();
        let ended = work::Tree::read_on(&space, start);
        assert_eq!(ended.occurrences[&first].superseded_on, Some(change_day));
        assert_eq!(ended.occurrences[&current].superseded_on, Some(start));
        assert_eq!(ended.occurrences[&skipped].superseded_on, None);
        index.rebuild(&space, &|_| None).unwrap();
        let hits =
            search_indexes([(space.id().unwrap(), &index)], "@recurrence=true", Sort::Title, 10)
                .unwrap();
        assert_eq!(hits.into_iter().map(|(_, hit)| hit.id).collect::<Vec<_>>(), [series]);
    }

    #[test]
    fn draft_schemas_drop_actionable_and_upgrade_recurrence() {
        let mut space = Space::create(Id::generate()).unwrap();
        let series = space.create_work(schema::TASK, "Report", None).unwrap();
        let day = date("2026-11-02");
        let rule = rule(Mode::Keep, "FREQ=DAILY;COUNT=1");
        space.set_recurrence(series, Some(rule.clone()), day).unwrap();
        let first = work::Tree::read_on(&space, day).current_occurrences[&series];
        let schema = child_map(&space.doc().get_map(schema::SCHEMAS), schema::TASK).unwrap();
        child_map(&schema, "recurrence").unwrap().insert("kind", "text").unwrap();
        let actionable = child_map(&schema, "actionable").unwrap();
        actionable.insert("kind", "boolean").unwrap();
        actionable.insert("search", "filter").unwrap();
        child_map(&space.note_map(series).unwrap(), "fields")
            .unwrap()
            .delete("recurrence")
            .unwrap();

        let mut reopened = Space::open(&space.snapshot().unwrap(), Id::generate()).unwrap();
        let schema = schema::read_type(reopened.doc(), schema::TASK).unwrap();
        assert!(!schema.fields.contains_key("actionable"));
        assert!(reopened.realize_recurrences(day).unwrap());
        let note = reopened.note(series).unwrap();
        assert_eq!(note.field(&reopened, "recurrence"), Some(Value::Boolean(true)));
        assert!(reopened.note(first).unwrap().field(&reopened, "recurrence").is_none());
        assert_eq!(reopened.recurrence_rule(series), Some(rule));
        assert!(!reopened.realize_recurrences(day).unwrap());
    }

    #[test]
    fn concurrent_replacements_agree_on_the_supersession_date() {
        let mut a = Space::create(Id::generate()).unwrap();
        let series = a.create_work(schema::TASK, "Report", None).unwrap();
        let day = date("2026-11-02");
        a.set_recurrence(series, Some(rule(Mode::Keep, "FREQ=DAILY")), day).unwrap();
        let original = work::Tree::read_on(&a, day).current_occurrences[&series];
        let mut b = Space::open(&a.snapshot().unwrap(), Id::generate()).unwrap();
        let mut changes = Vec::new();
        for (space, day) in [(&mut a, date("2026-11-03")), (&mut b, date("2026-11-04"))] {
            let replacement = Rule { start: day, ..rule(Mode::Keep, "FREQ=DAILY;COUNT=1") };
            space.set_recurrence(series, Some(replacement), day).unwrap();
            let revision = Series::read(space, series).head().unwrap();
            changes.push((revision, day, Slot::new(series, revision, day, None).id));
        }
        changes.sort_unstable();
        let (_, _, loser) = changes[0];
        let (_, winning_day, winner) = changes[1];
        let snapshot = a.snapshot().unwrap();
        a.doc().import(&b.snapshot().unwrap()).unwrap();
        b.doc().import(&snapshot).unwrap();
        for space in [&mut a, &mut b] {
            let today = date("2026-11-05");
            space.realize_recurrences(today).unwrap();
            assert!(!space.realize_recurrences(today).unwrap());
            let tree = work::Tree::read_on(space, today);
            assert_eq!(tree.current_occurrences[&series], winner);
            assert_eq!(tree.occurrences[&loser].superseded_on, Some(winning_day));
            assert_eq!(tree.occurrences[&original].superseded_on, Some(winning_day));
        }
    }

    #[test]
    fn sync_reconciles_outcomes_for_occurrences_realized_offline() {
        let mut a = Space::create(Id::generate()).unwrap();
        let series = a.create_work(schema::TASK, "Report", None).unwrap();
        let first_day = date("2026-11-02");
        let replacement_day = date("2026-11-03");
        let return_day = date("2026-11-04");
        a.set_recurrence(series, Some(rule(Mode::Keep, "FREQ=DAILY")), first_day).unwrap();
        let first = work::Tree::read_on(&a, first_day).current_occurrences[&series];
        let mut b = Space::open(&a.snapshot().unwrap(), Id::generate()).unwrap();

        a.set_recurrence(series, None, replacement_day).unwrap();
        b.record_occurrence(first, Outcome::Skipped, return_day, "").unwrap();
        let late = work::Tree::read_on(&b, return_day).current_occurrences[&series];
        assert!(a.note(late).is_err());
        a.import_checked(&b.snapshot().unwrap()).unwrap();
        assert!(a.realize_recurrences(return_day).unwrap());
        assert_eq!(a.note(late).unwrap().fields["superseded-on"], "2026-11-03".into());
        assert!(!a.note(first).unwrap().fields.contains_key("superseded-on"));
        let tree = work::Tree::read_on(&a, return_day);
        assert_eq!(tree.date(first, "skipped-on").unwrap().value, "2026-11-04");
        let actions = records::<serde_json::Value>(&a, first, ACTIONS);
        let action = actions.values().next().unwrap();
        assert_eq!(action["skipped-on"], "2026-11-04");
        assert!(action.get("completed-on").is_none());
        assert!(!a.realize_recurrences(return_day).unwrap());
    }

    #[test]
    fn occurrence_subtasks_inherit_dates_and_stay_with_their_own_occurrence() {
        let mut space = Space::create(Id::generate()).unwrap();
        let project = space.create_work(schema::PROJECT, "# Project", None).unwrap();
        let series = space.create_work(schema::TASK, "# Series", Some(project)).unwrap();
        let day = date("2026-11-02");
        space.set_recurrence(series, Some(rule(Mode::Interval, "FREQ=DAILY")), day).unwrap();
        let first = work::Tree::read_on(&space, day).current_occurrences[&series];
        let subtask = space.create_work(schema::TASK, "# Subtask", Some(first)).unwrap();
        let recurring = space.create_work(schema::TASK, "# Repeat subtask", Some(subtask)).unwrap();
        let nested_rule = Rule { due: None, scheduled: None, ..rule(Mode::Interval, "FREQ=DAILY") };
        space.set_recurrence(recurring, Some(nested_rule), day).unwrap();
        assert!(space.create_work(schema::PROJECT, "# Bad parent", Some(first)).is_err());
        assert!(space.set_field(series, "parent-id", &Value::Link(first)).is_err());
        let tree = work::Tree::read_on(&space, day);
        assert_eq!(tree.children(series), []);
        assert_eq!(tree.children(first), [subtask]);
        assert_eq!(tree.context(subtask), first);
        assert_eq!(tree.projects(subtask), [project]);
        let due = tree.date(subtask, "due-on").unwrap();
        assert_eq!(due.source, first);
        assert_eq!(due.value, "2026-11-04");
        let nested = tree.current_occurrences[&recurring];
        assert_eq!(tree.occurrences[&nested].due.as_deref(), Some("2026-11-04"));

        space.record_occurrence(first, Outcome::Completed, day, "").unwrap();
        let tree = work::Tree::read_on(&space, day);
        let second = tree.current_occurrences[&series];
        assert!(tree.children(second).is_empty());
        assert_eq!(tree.children(first), [subtask]);
        assert!(!tree.items[&subtask].completed());
        let deleted = space.delete_note(project).unwrap();
        assert!(deleted.contains(&first) && deleted.contains(&subtask));
    }

    #[test]
    fn concurrent_checks_of_the_same_item_keep_a_valid_checkbox() {
        let mut a = Space::create(Id::generate()).unwrap();
        let series = a.create_work(schema::TASK, "# Routine\n\n- [ ] Item", None).unwrap();
        let day = date("2026-11-02");
        a.set_recurrence(series, Some(rule(Mode::Interval, "FREQ=DAILY")), day).unwrap();
        let id = *work::Tree::read_on(&a, day).occurrences.keys().next().unwrap();
        let mut b = Space::open(&a.snapshot().unwrap(), Id::generate()).unwrap();
        let checked = a.note(id).unwrap().body.replace("[ ] Item", "[x] Item");
        for space in [&mut a, &mut b] {
            space.set_body(id, &checked).unwrap();
            space.note_edited(id).unwrap();
        }
        let snapshot = a.snapshot().unwrap();
        a.doc().import(&b.snapshot().unwrap()).unwrap();
        b.doc().import(&snapshot).unwrap();
        for space in [&mut a, &mut b] {
            space.realize_recurrences(day).unwrap();
            assert_eq!(space.note(id).unwrap().body, checked);
        }
    }
}
