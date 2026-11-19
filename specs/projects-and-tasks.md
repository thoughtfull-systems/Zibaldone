# Projects and tasks

This specification follows [ZIB0025][decision]. The [built-in types specification][types] defines
the fields, and the [space document specification][document] defines their encoding.

## Stored values

Both `project` and `task` are note types with ordinary markdown bodies and immutable types.

- `due-on` and `scheduled-on` are optional date fields, encoded as `YYYY-MM-DD`.
- `parent-id` is an optional link to one note in the same space.
- `completed-on` is an optional date on `task`. A valid date means completed.

An unset scheduled or due date has no key and means inherit. Writers store only explicit values. No
value blocks inheritance. A completion date never inherits: an absent or invalid value means
incomplete. Unknown fields retain the normal preservation rules.

## Derived hierarchy

Read projects and tasks in the space. Ignore a parent link when its target is missing, isn't a
project or task, or is a task named as a project's parent. For each remaining directed cycle, ignore
the parent link from the item with the greatest identifier. Compare identifiers by their bytes.
Leave the stored links unchanged. These rules produce the same forest on every replica.

Local parent edits reject self-links, cycles, absent targets, targets in another space, and invalid
parent kinds. Projects can have only projects as parents. Tasks can have tasks or projects as
parents. An absent parent creates a root. Moving or deleting a parent doesn't delete its subtasks or
subprojects; they become roots when their target leaves the space.

Nesting doesn't impose dependencies or cascade completion. Resolve each date by walking from the
item toward the root and taking the first valid explicit date. Resolve the two dates independently.
Changing a parent or an ancestor's dates changes effective values without writing to descendants.

## Subtree deletion

This section replaces the rule that deleting a parent leaves its subtasks and subprojects as roots.
Deleting a task also deletes every descendant task. Deleting a project also deletes every descendant
project and task. Use the derived forest at the time of deletion. Include completed and hidden
items. Remove all affected notes and write each note's last-title tombstone in one commit. Give the
tombstones the same deletion timestamp and machine identifier. Preserve unrelated notes, ancestors,
siblings, and manual-order entries.

Deletion applies to the items present in the deleting replica's forest. Applying synced deletions
doesn't trigger another deletion walk. A subtask or subproject created concurrently on another
replica can survive as a root when its parent is missing. Moving a parent to another space still
doesn't delete its subtasks or subprojects. Ordinary notes delete only themselves, regardless of
links in their bodies or fields.

Both the desktop's main delete button and a task's overflow action use these rules. Confirmations
name the item kind and describe deleting its descendants. Remove the whole deleted set from search,
navigation history, and cached task drafts. Close an editor whose note belongs to that set; keep a
surviving ancestor project open when deleting one of its tasks.

## Search

For projects and tasks, `@scheduled-on` and `@due-on` compare effective dates, and `@parent-id`
compares the derived immediate parent. Their presence and missing operators use the same effective
values. `@completed-on!?` matches tasks with absent or invalid completion dates. `@completed-on?`
matches completed tasks, and date comparisons and ranges match their completion dates.

The virtual `@project` field matches ancestor project identifiers. It follows both task parents and
subprojects. For a project, it matches its ancestor projects, excluding itself. A standalone task
has no project value. It supports equality and presence or missing conditions. Its values don't live
in a note's fields map.

Date comparisons and range bounds accept `today`, resolving it to the local calendar date each time
the query runs. Other field kinds keep their existing input rules. The `due` sort orders effective
due dates ascending, undated items last, then case-insensitive title and stable identifiers. Apply
the same ordering across spaces before limiting results.

The initial work query is:

```text
@type=task AND @completed-on!? AND (@scheduled-on!? OR @scheduled-on<=today)
```

A future effective scheduled date hides even an overdue task. A parent scheduled date supplies a
default rather than an additional gate on a subtask with an earlier explicit scheduled date. Parent
tasks remain candidates alongside their unfinished subtasks.

## Desktop

The new-note menu offers projects and standalone tasks. A project shows its markdown, subproject
links, and nested tasks. Selecting a task result opens the nearest ancestor project, expands the
task's ancestor path, and opens its markdown editor within the hierarchy. Without a project, open
the root task. Reveal the selected descendant within that tree. Result definitions and result lists
stay open.

Task rows show completion and effective dates. Show completed controls completed rows; unfinished
descendants keep completed ancestors visible as context. A selected completed task remains open
until navigation chooses another task. Each item offers parent and date controls. Date controls
identify inherited sources and allow either an explicit date or inheritance.

The local calendar date determines overdue, due today, and due soon labels. Due soon covers the next
three calendar days. Pair urgency styling with text. Refresh relative-date searches and urgency
after the date changes, including a timezone change that changes the local date.

Checking a task's completion control sets `completed-on` to the machine's current local calendar
date. Unchecking it removes the field. Later edits preserve the completion date. Reopening and
completing again records the new completion date; this field doesn't hold completion history.

## Initial saved searches

When creating the first work item in a space, create All tasks with `@type=task` and due sort,
Projects with `@type=project` and title sort, and What should I work on? with the work query and due
sort. Scope each to that space. Keep an existing search with any of these names. These are ordinary
editable [saved searches][saved].

The space setting `zib.work-searches` records initialization with boolean true. Once it exists,
later work items don't recreate searches. Older releases ignore the setting. Concurrent first
creations can produce separate searches; each remains an independent editable definition.

## Platforms

The core defines the same hierarchy, dates, and search behavior on every platform. Linux provides
the desktop workflow. Recurrence, waiting states, and project inactivity remain outside this format.

## Compact desktop controls

This section replaces the task-row editor presentation in Desktop. A task row shows a completion
checkbox, an inline title text box, and scheduled and due date controls. Selecting a task reveals
its row in project context without expanding a markdown editor. Existing task body content remains
in storage. Adding subtasks and deleting tasks use the row's actions menu.

Title edits replace only the body's first line that isn't blank. Preserve every other line,
including edits from another machine. Escape punctuation in the plain title input when writing
markdown. Reject a write if the original title line changed in the meantime. Keep the draft visible
and offer Escape to reload the current title. Save after a typing pause, on Enter, and when focus
leaves the field. Flush pending title writes before quitting. Refreshes preserve dirty drafts,
focus, and the caret.

Each date control shows its effective date beside a calendar icon. The icon opens a popover with a
date text box, a calendar selector, Apply, and Clear date. Text and calendar selection stay in sync.
Invalid text disables Apply. Enter or Apply stores an explicit date. Clear date removes an explicit
value and resumes inheritance, so an inherited date can remain visible. Keep Clear date inactive
when the item already inherits. Identify the source of an inherited date within the popover.

The parent control uses a folder icon. Its popover lists candidate projects and a search text box.
Match typed words incrementally against project titles without case sensitivity. Exclude the item
itself and its descendants. Selecting a candidate sets the parent; No parent removes it. The model
still supports task parents, and Add subtask creates a task under a task.

Keep open popovers and their inputs stable while background updates arrive. Refresh their rows when
the popover closes. These controls apply to projects and tasks alike.

Mirror the parent icon horizontally. Date and parent controls don't highlight on hover. Search
results dim completed tasks. Incomplete tasks retain normal emphasis or their due-date urgency
style.

The project header puts the parent navigation link and its date and parent pickers on one row.
Project views omit a separate Project type label. Standalone task views omit a separate Standalone
task type label.

Task rows show their scheduled and due date pickers followed by an overflow menu. The Choose parent
action opens the searchable project picker within that menu, with a way back to task actions.
Reopening the menu starts on its actions page. Tasks can still change parents through drag and drop.
Projects keep their parent selector beside their dates. Group the task drag handle, expander, and
checkbox with 2-pixel spacing, and reduce the horizontal padding on the expander and checkbox.

Show a task's scheduled or due date control only when its effective value exists, including an
inherited value. Undated tasks show no empty date placeholders. For each missing effective date,
offer Add schedule date or Add due date in the task's overflow menu. Each action opens the usual
calendar and text input within the same popover. Saving adds the inline date control. Clearing an
override resumes inheritance. Keep the control visible when an inherited date remains. Otherwise,
remove it. Removing an ancestor's date updates affected task rows. Project date controls stay
visible even when unset.

Task overflow actions use the same native menu presentation as the new-note and app menus. Date and
parent pickers open as submenus with native back navigation. Create picker inputs when first opening
their submenu, and preserve them when returning to it while the menu stays open. Opening Choose
parent focuses its search text box after native menu navigation finishes, including return visits to
that submenu.

Reserve the addition text box's row-control width even when its list is empty. Use invisible,
inactive controls with the same theme sizing as populated rows. Keep these placeholders out of
keyboard navigation and accessibility output. Adding or removing the last item doesn't shift the
text box horizontally. Use subtask and subtasks for nested tasks, and subproject and subprojects for
nested projects throughout the interface and its documentation.

Keep subtask rows close to the density of subproject rows. Remove extra padding around task date and
overflow controls and omit additional bottom margins on task rows. Keep the expander and completion
checkbox free of horizontal padding. Apply the same sizing to the addition placeholders so
empty-list alignment stays stable.

## Manual task order

The optional `work-order` root container is a Loro movable list of canonical work-item identifier
strings. Its sequence orders siblings: filter it by the derived parent, ignore missing notes and
invalid values, and use only the first occurrence of each identifier. Append unlisted siblings in
case-insensitive title order, breaking ties by identifier bytes. Order never determines parentage.
Completed tasks keep their positions when hidden, and title edits don't change a positioned item's
order. Search results keep the selected search sort.

Before a local move or creation, remove duplicate occurrences of live work items, keeping the first.
Append unlisted live work items in case-insensitive title order, breaking ties by identifier bytes.
Preserve other entries. New work items append their identifiers. Readers never hide an item because
its position is absent or invalid. Concurrent initializations can create duplicates; the first
occurrence rule still produces the same order after sync.

A task move names its new parent and an optional sibling before which to insert it. An absent
sibling means append. Validate the task, parent, cycle constraints, and destination sibling before
writing. Move the existing sequence element and update the parent link in one commit. Descendants
keep their parent links and order. Their effective dates and project membership follow the new
ancestor chain. A reorder within the same parent doesn't change note content or modification times.

The desktop shows a drag handle beside each task. Dropping on a row's top quarter inserts before
that task, and dropping on its bottom quarter inserts after it at the same nesting level. Dropping
in the middle half appends the dragged task as a subtask and expands the target. Use the complete
sibling sequence when resolving insertion, including hidden completed tasks. Show an insertion line
at the appropriate edge or highlight the nesting target. Reject self-drops, descendant targets,
non-task payloads, and tasks from another space. Preserve title drafts while dragging and defer
background row rebuilds until the drag ends.

## Manual subproject order

Subprojects use the same `work-order` sequence. A project reorder changes only the sequence position
of that project among sibling projects; its parent link, descendants, body, fields, and metadata
stay intact. Reject a destination sibling with a different parent or note type. An absent
destination sibling appends the project. Task order remains unchanged.

The desktop adds drag handles beside subproject links. Dropping in the upper half of a subproject
row inserts before it, and dropping in its lower half inserts after it. Show the corresponding
insertion line. Resolve the next sibling among projects, excluding task rows. Reject self-drops,
non-project payloads, and projects from another parent or space. Clicking the project link still
opens that project. Reuse the persistence, sync, and drag refresh rules for task ordering.

## Inline additions

A project shows a Subprojects heading before its subproject links. The section ends with a
single-line title text box and Add button. The Tasks section ends with its own title text box and
Add button. A standalone task has only the task addition. Each addition creates a subproject or
subtask under the open project or task, with no explicit dates. Titles contain plain text; escape
markdown punctuation when storing the heading.

Enter or Add submits a nonempty trimmed title. Keep Add inactive for an empty title or during a
submission, keeping the text box editable. Keep the current note open and focus the addition's text
box. On success, clear the submitted title unless the owner has typed a different draft in the
meantime. Preserve that newer draft. Failures keep the input and show an error. Hierarchy refreshes
preserve addition widgets, drafts, and focus. Creation appends the new item using the manual
ordering rules.

## Enter-only additions

This section replaces the Add buttons in Inline additions and the Add subtask action in task menus.
Each list ends with only its new-title text box. Enter submits a nonempty trimmed title. Ignore
empty submissions and repeated Enter presses while creation runs, keeping the text box editable.
Keep the focus, draft preservation, plain-title escaping, and project context rules from Inline
additions. Align the left edge of each addition text box with the titles of that list's direct
subtasks or subprojects. Reserve the same space as the row controls, using their current theme
sizes. Create subtasks by adding a task and dragging it under another task.

## Project details sidebar

For projects, place the parent navigation link, scheduled and due date controls, parent selector,
Subprojects section, and Tasks section in a right sidebar beside the project's markdown editor. The
note and sidebar scroll independently. Keep inline additions, compact title editing, drag ordering,
date inheritance, and completed-task filtering in the sidebar. Revealing a task scrolls the sidebar
without moving the project note. Standalone task views keep their inline hierarchy.

The divider resizes the sidebar and never collapses it while a project is open. Give the note at
least 280 logical pixels and the sidebar at least 600. Keep each task's date and parent controls on
one row. Limit the sidebar to 70% of their combined usable width, and leave the note at least its
minimum. Without a saved width, divide the usable width equally, subject to these limits.

The machine's `machine.json` stores a chosen width in the optional `project-sidebar-width` key, a
positive integer in logical pixels. Share this preference across project views on that machine;
don't sync it or store it in notes. Save after a pointer drag or keyboard resize finishes, and flush
pending writes on navigation, window close, and quit. Restore the width within the current bounds.
Clamp previously saved narrow widths to the new minimum for display. Invalid stored widths fall back
to the equal split. Automatic layout changes don't replace the owner's saved width. Resizing a
window without a saved choice keeps the equal split.

## Work status

This section adds the status model in [ZIB0027](../decisions/ZIB0027-work-status.md). Projects and
tasks have optional `status` and `waiting-for` fields. The status is a choice: `maybe`, `someday`,
or `waiting`. Waiting text is a string with outer whitespace removed. An empty string means no
detail. Invalid status values have no effect on inheritance and remain stored.

Resolve status by walking the derived forest from the root toward the item and taking the first
valid explicit status. For Waiting, read text from that same source; don't inherit text separately.
An absent status inherits. Without any status in the path, the item is available. Projects and tasks
follow the same rule. Inherited values can't be overridden. Local status edits reject an item with
an inherited status. Preserve masked explicit values after parent edits or sync; they become
effective again when no ancestor supplies a status. Completion and dates remain independent.

A local status change writes both fields and any new Waiting history in one commit. Clearing status
removes both fields and resumes inheritance. Non-Waiting status removes Waiting text. Waiting
without text remains valid after concurrent field edits. Historical Waiting values remain stored.

Search indexes effective `status` and `waiting-for`. Index Waiting text only when effective status
is `waiting`. Refresh dependent rows after ancestor status, reason, or parent changes. Field-name
completion offers both names. The work query now includes `AND @status!?`, excluding all three
states even when a task is overdue. All tasks and Projects remain unrestricted saved searches.

On opening a space, upgrade saved searches named What should I work on? only when their query is the
earlier exact default, their scope is that space, and their sort is due. Add `AND @status!?` and
record `zib.work-status-search` as boolean true in space settings. Mark the setting even when no
search qualifies. Preserve customized queries, names, scope, and sort. New default searches include
the status condition from creation.

Projects offer a Status menu beside their date and parent controls. Task overflow menus offer Set
status, and rows with an effective status show a compact control. Unrestricted items offer Inherit,
Maybe, Someday, and Waiting. Waiting opens a focused text box with matching earlier values, Apply,
and Enter submission. Tab completes a matching value; selecting a suggestion fills the text box.
Match incrementally without case sensitivity. Retain earlier values after clearing or deleting the
last note that uses them. Combine values from all held spaces. Inherited controls name the source
and offer Open source, without status editing actions. Preserve open picker input during refreshes.

## Status overrides

This section replaces the ancestor-first status and read-only inherited controls in Work status. Add
`none` to the `status` choices. Resolve the nearest valid explicit status by walking from the item
toward the root. An absent field means Inherit. Maybe, Someday, and Waiting override an ancestor's
value. Explicit None stops inheritance and leaves the item available. Subtasks and subprojects
inherit that None unless they set their own status. Changing or moving a parent keeps explicit
overrides. Selecting Inherit removes the item's fields and resumes inheritance.

Search treats explicit or inherited None as missing effective `status` and `waiting-for`. The
existing work query therefore includes these available tasks with `@status!?`. Deferred overrides
still exclude tasks, and Waiting text comes from the source of the effective Waiting value.

All status menus offer Maybe, None, Someday, and Waiting. When a parent supplies a status, including
None, also offer Inherit. Keep Inherit available for an explicit None after the parent's status
clears, so the owner can remove the inheritance barrier. Retain inherited-source tooltips and
Waiting completion. Show a compact None control for explicit and inherited None values.

Offer Inherit for any explicit status, including Maybe, Someday, and Waiting, even when no ancestor
currently supplies a status. Selecting it directly removes the override. Omit the current explicit
Maybe, None, or Someday option from the menu. Keep Waiting available so its text can still change.
Inherited values remain selectable as explicit overrides.

Clicking or activating a Waiting suggestion saves that value and closes the status menu after the
write succeeds. Selection doesn't require an Apply click. Enter on a highlighted suggestion also
saves it. Tab only fills the text box, allowing further edits; typed values still support Enter and
Apply.

[decision]: ../decisions/ZIB0025-projects-and-tasks.md
[document]: space-document.md
[saved]: saved-searches.md
[types]: built-in-types.md
