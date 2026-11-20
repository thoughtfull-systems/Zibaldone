# Recurring tasks

This specification follows [ZIB0028][decision].

## Rules

A task's optional mergeable `recurrence-revisions` map uses revision identifiers as keys. Each value
is a JSON string with `after`, `at`, `author`, `cutoff`, `previous`, `rule`, and `template`. The
timestamp `at` is in milliseconds. `previous` names the preceding revision or is null. Start at the
revision with no predecessor. At each fork choose the greatest identifier. Follow its descendants to
the active revision. The cutoff is a date. A null rule ends the series. Retain all revisions and the
occurrences they've produced.

Rules hold `due` and `scheduled` offsets in calendar days or null, a `mode`, a `pattern`, and a
`start` date. The first matching date is on or after start. The pattern accepts this subset of
calendar recurrence rules.

- `FREQ` is `DAILY`, `WEEKLY`, `MONTHLY`, or `YEARLY`.
- `BYDAY` accepts weekday lists and monthly ordinals from first through fifth or last. Yearly
  ordinals require `BYMONTH`.
- `BYMONTH` and `BYMONTHDAY` accept single positive values.
- `INTERVAL` defaults to one. `WKST` defaults to Monday.
- `COUNT` or inclusive `UNTIL` bounds the result. Missing calendar dates don't consume a count.

In `keep` mode, realize all matching dates through today and dates brought forward by a scheduled
offset. If none of the keep occurrences are open and the active rule uses `keep` mode, realize the
earliest remaining future occurrence. This also allows the first occurrence to start in the future.
Reopening an earlier occurrence preserves any future occurrence the rule already realized.

In `jump` mode, the next date must match the calendar rule, follow the current date, and fall on or
after the action date. Passed dates consume their original count positions. In `interval` mode, add
the interval to the action date, clamping a missing day to the end of its month. Each distinct cycle
consumes one count position. Interval mode uses frequency and interval without date selectors.
Completion and explicit skip each qualify to advance once.

Writers store the series body at rule creation in `template` and set `after` to null. A saved
`after` object in a jump or interval revision retains the referenced occurrence. Its first close
advances the series. The object holds `cycle`, `day`, `id`, `key`, `revision`, and `series`.

## Identity and body

The occurrence key consists of the task identifier, the revision identifier, and the slot. A
calendar slot in keep mode uses `YYYY-MM-DD`. A jump or interval slot uses a cycle number in decimal
starting at one. Hash the concatenated 16-byte task ID, 16-byte revision ID, and UTF-8 slot with the
BLAKE3 context `Zibaldone task occurrence identifier`. Keep the first 16 bytes and set the version
to eight and the variant to the standard Universally Unique Identifier variant.

Each occurrence uses the `task` type. It stores `series-id`, `rule-id`, `occurrence-slot`, and
`original-on` as strings. The presence of `series-id` marks the task as an occurrence. Writers
reserve the occurrence identity fields for the recurrence engine. A calendar slot stores its date in
`original-on`. A sequential slot stores the rule start there and separately stores its initial
proposed date in `realized-on`. Readers derive the current sequential date from the winning
advancement.

Initialize the note through a Loro update from an otherwise empty document. Take the first eight
bytes of BLAKE3 with context `Zibaldone occurrence seed writer` and the 16-byte occurrence ID as
input. Interpret the bytes as an unsigned big-endian integer, shift right one bit, and set bit 63.
Cap the result at one less than the largest unsigned 64-bit integer. Use it as the seed writer.
Write the note in the regular note structure with the revision's author and timestamp. Use the
revision's `template` value as the shared starting point for merges. Clear its Markdown task-list
checkboxes.

At creation, read the series' current body and clear its task-list checkboxes. When the result
differs from the shared starting point, apply the Loro text update from that starting point to the
current body. Use a separate seed writer. Derive its hash with context
`Zibaldone occurrence body seed writer` from the 16-byte occurrence ID followed by the 32-byte
BLAKE3 digest of the updated UTF-8 body. Convert the writer hash to a peer identifier using the same
rule as for the first update. Record the updated body digest as lowercase hexadecimal with value
`true` in the occurrence's mergeable `recurrence-body-seeds` map. Commit the text update and digest
together. Matching bodies use identical operations. Different starting texts merge as concurrent
edits against the shared starting point.

An occurrence uses its own body once it exists. Editing the series body updates the starting text
for new notes as the rule creates them.

The app realizes notes on rule changes, completion or skip, startup, sync, and periodic upkeep.
Previews don't create notes. Occurrences stay addressable after a rule change. Deleting the series
removes its known occurrence notes with the task subtree. Series and occurrence notes stay in their
original space.

## Occurrence actions

The occurrence's optional mergeable `recurrence-actions` map uses an action identifier as each key.
Its values are JSON strings with `due`, `next`, `scheduled`, `seen`, and an optional outcome date. A
completion stores `completed-on` and a skip stores `skipped-on`. Each uses the local date of the
action. A reopen omits both. `next` is a proposed successor date or null. `seen` lists observed
action heads. For older action records, readers also accept `day` with `outcome` of `completed`,
`skipped`, or `open`.

For jumping or interval work, choose a first action with an earlier successor date over one with a
later date or no date. Break ties by the lowest action identifier. Apply the bounds to the successor
after selection. Later actions can update the outcome without changing the advancement. For
competing outcome corrections, prefer a causal successor. Among the remaining concurrent actions
choose a reopen first and use the greatest action identifier for ties. Keep mode uses this outcome
rule for all actions.

Capture scheduled and due dates on completion or skip, including absent dates. Closed occurrences
use the captured dates. Open occurrences use individual overrides, then revision offsets, then the
series' effective dates. Keep the Markdown body through all outcome changes. An optional comment
adds a dated paragraph in the same commit as the action.

## Superseded occurrences

A revision can include `changed_on`, the editing machine's local date when it saves the revision.
Replacing or ending a rule stops the old rule from creating occurrences. All open occurrences from
the old rule become superseded regardless of their dates or edits. Completed and skipped occurrences
keep their outcomes. Reopening an occurrence from a replaced rule makes it superseded. Superseded
occurrences stay in the series history with their bodies and date overrides. Changing only date
offsets updates the same revision.

Store `superseded-on` on the occurrence using the first revision on the winning chain that replaces
the occurrence's revision or diverges from its branch. Use `changed_on` when present. Otherwise use
the revision's timestamp as a calendar date in Coordinated Universal Time (UTC), or use its cutoff
if the timestamp is out of range. An occurrence carried forward by the active rule's `after` remains
part of that rule. On startup and sync, reconcile the stored date against the winning revision and
outcome, including for occurrences that arrive after a rule change. History and search use this
date.

## Search

Index `series-id` and the effective `recurrence-on`, `scheduled-on`, and `due-on` dates. An
occurrence inherits the series' project context and work status. Index `completed-on` or
`skipped-on` for a closed occurrence. Backlinks continue to use body links.

Store the boolean `recurrence` as true on a series template when the first revision arrives. Keep it
true when a revision ends the series. Ordinary tasks and occurrences omit `recurrence`.

The default work search uses `@recurrence!?` and requires absent `completed-on`, `skipped-on`, and
`superseded-on` dates. It also checks scheduled dates and deferral status. Upgrade the previous
default query in its own space when the name and sorting also match. All tasks uses `@type=task`.

## Offset changes

Saving the current pattern, mode, and start with new date offsets updates that revision. The
optional mergeable `recurrence-offsets` map holds a JSON string per revision ID. Its value is the
two-element array of scheduled offset followed by due offset. Each value is a whole number or null.
Open occurrences use the offsets from the map when present. Closed occurrences keep their captured
dates.

## Desktop

Open a task's menu and choose **Repeat…**. Choose a mode and start date. Enter a calendar rule such
as `FREQ=WEEKLY;BYDAY=MO,WE` or an interval such as `FREQ=MONTHLY;INTERVAL=2`. The preview shows
matching dates. Select **Use offset** to enter the offset in days. Enter zero to schedule or make an
occurrence due on its recurrence day. Clear the checkbox to use the task's effective date.

Open **Occurrence history** from the series menu. Selecting a row opens its occurrence note and
keeps the history pane available. The open occurrence's completion row has a checkbox followed by
**Complete with note…**, **Skip**, and **Skip with note…**. The actions with a note append to the
occurrence's body. Edit the recurring task's body to change the starting text for new occurrences.

## Graphical recurrence controls

These controls replace the rule text entry in Desktop. **Repeat…** offers the mode, interval,
frequency, start date, and date offsets. Calendar rules also offer months, days of the month, and
weekday checkboxes, with ordinal weekdays where the rule supports them. Choose whether the series
runs indefinitely, through a date, or for a fixed count. The preview shows the resulting dates.

The series row has a repeat icon and buttons for the current occurrence and occurrence history. The
expander shows only actual subtasks. For a calendar series that keeps every occurrence, the current
link opens the earliest open occurrence that can start today. When none can start, it opens the
earliest open future occurrence. For jump and completion-relative rules, it opens the open
occurrence that can advance the series.

## Options by frequency

The desktop shows weekday buttons for weekly rules, a day or ordinal weekday for monthly rules, and
a month plus a day or ordinal weekday for yearly rules. Daily and completion-interval rules need
only a frequency and interval. Weekly buttons start on Sunday. New or changed weekly patterns
specify `WKST=SU`. Opening an existing rule or editing only its date offsets preserves its stored
week start. Older rules that combine calendar filters can keep their saved pattern or switch to the
options for their frequency.

## History and occurrence notes

The recurring task's sidebar shows **Occurrence history** before **Subtasks**. The history shortcut
opens the recurring task. Select a history row to open that occurrence's note. Its parent link
returns to the recurring task.

**Complete with note…** and **Skip with note…** focus the text box when the dialog opens.

## Occurrence subtasks and navigation

A task can use an occurrence's identifier in `parent-id`. A subtask belongs to that occurrence and
can inherit its effective dates and status. Completing, skipping, or repeating the occurrence leaves
its subtasks with their existing parent and completion state. A series or project deletion includes
its occurrence subtasks.

When deriving the hierarchy, include each occurrence's link to its series before resolving cycles.
In each cycle, ignore the explicit parent link of the item with the greatest identifier among those
with explicit parent links. Preserve the link from the occurrence to its series.

The desktop lists occurrence subtasks in the occurrence's sidebar. A search result for an occurrence
opens that occurrence. A search result for its subtask reveals the subtask in the occurrence's
sidebar. A series result in a project opens the project and reveals the series row. Use **Edit task
body** on that row to find the current occurrence and history in the series editor.

## Previewing a rule change

Before replacing or ending an active rule, the desktop opens a read-only preview. It lists every
open occurrence that the change supersedes, in recurrence date order. Confirm to apply the change.
If the rule or previewed set changes during review, ask the owner to review the change again.
Changes that only adjust date offsets skip the replacement preview.

When both rules use a count limit, suggest the previous count minus the calendar slots before the
new start date. For a completion interval, subtract the cycles before that date instead. Offer a
suggestion when at least one remains. Keep the count chosen in the editor, which starts with the new
rule's first occurrence. The editor carries forward the previous end date by default.

## Late arrivals for a deleted series

During recurrence upkeep, remove any occurrence whose series has a tombstone in the same space. Also
remove tasks still attached under that occurrence through `parent-id` or `series-id` links. Write
tombstones for the removed notes with their own titles and the deletion timestamp and machine from
the series tombstone. Preserve any existing tombstone. This reconciliation covers occurrences that
another machine creates before it receives the series deletion.

[decision]: ../decisions/ZIB0028-recurring-tasks.md
