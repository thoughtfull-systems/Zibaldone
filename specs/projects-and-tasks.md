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

## Title rows and details

This section replaces the single-row compact controls. Subproject and subtask rows show a plain
clickable title on the first row, with the overflow menu at its right edge. Align the drag handle,
subtask expander, and completion checkbox vertically with that title. Completed tasks dim only the
title. Clicking a title opens its inline editor. Enter or leaving the editor saves and returns to
the title display; Escape restores the current stored title. Keep invalid or conflicting drafts
editable. Preserve the existing save, refresh, and title-line conflict rules. Apply these title
edits to projects too, preserving their other body content. A separate arrow opens a subproject.

Place effective status, scheduled date, and due date beneath the title, in that order. Align the
details' left edge with the title rather than the drag handle. Omit absent values. Clicking any part
of a date control opens its calendar picker. Clicking status opens its menu. Keep title and metadata
controls independent, including in search results. Preserve open popovers while results or work rows
refresh.

Status menus offer Inherited, Someday, Maybe, Waiting, and None in that order. This presentation
order replaces the earlier alphabetical order. Inherited removes an explicit override. Keep the
existing relevance rules: hide the current explicit fixed status, retain Waiting to edit its text,
and offer Inherited when an explicit or ancestor value exists.

Wrap status and date controls using their individual natural widths with 4-pixel gaps. Don't share
column widths between wrapped lines: a due date on the next line never widens a status control on
the first line. Apply this rule to search results and work-row details.

Date controls show text without a calendar icon; the text still opens the calendar picker. Work-row
titles use normal font weight, including during inline editing.

## Consistent task emphasis

Apply completion and due-date emphasis to each entire task row in search results and project task
lists. Completed tasks dim their title, status, dates, and controls together, with no additional
emphasis on the title alone. Completion takes precedence over urgency. Incomplete tasks due today or
within three days use warning emphasis throughout the row; overdue tasks use error emphasis. Use
effective inherited due dates, and keep other incomplete tasks at normal emphasis. Style each row
independently so a parent task's appearance doesn't affect its subtasks.

Due-soon and overdue rows also use bold text for their titles and details in both views. This
urgency rule replaces normal title weight while the task is urgent. Completed tasks keep normal
weight and dimming, even after their due date. Use the same bright amber-yellow warning color in
light and dark themes.

Only the title uses bold weight for due-soon and overdue tasks. This replaces the preceding rule for
bold details. Keep status, dates, type pills, and breadcrumbs at normal weight while preserving
their row's color and completion styling.

Use a slightly darker amber-yellow in light mode and keep the bright amber-yellow in dark mode. This
replaces the shared warning color across themes. Update existing rows when the theme changes.

## Picker activation and menu accents

Urgency changes row text color and title weight without changing menu accents, selection highlights,
or picker controls. Due-soon and overdue tasks use the same menu styling as ordinary tasks.

Clicking any calendar day saves that date and dismisses the picker. This includes the already
selected date and today's initial selection in an undated picker. Month and year navigation don't
save or close the picker. Typing a date keeps the picker open until Enter or Apply; Clear date keeps
its existing inheritance behavior.

Calendar pickers omit the Apply button. Save typed dates with Enter. Invalid input keeps the picker
open for correction. Keep calendar-click saving and Clear date behavior unchanged.

Place the date text box and Inherited button together on the picker's first row, with the button on
the right. Inherited replaces Clear date and keeps its behavior and sensitivity rules. Omit the
explanation that clearing resumes inheritance; retain the source name for an inherited date.

## Remember completed visibility

Show completed reads and writes the displayed note's `zib.show-completed` setting, as the
[space document specification][document] defines it. Restore it when opening or refreshing a project
or standalone task view. Preserve the choice after restart and sync. A subproject has its own
choice. Selecting a task search result uses the enclosing work view's setting. Preserve the existing
exception that reveals a selected task or an unfinished task's completed ancestors without changing
the checkbox.

## Task overflow order

Task overflow menus place status choices directly in the main menu. Order the actions as Add
schedule date, Add due date, status choices, Choose parent, and Delete task. Keep the existing
relevance rules for date and status actions. Within the status choices, keep Inherited, Someday,
Maybe, Waiting, and None in that order. Waiting opens its existing text and completion picker.

## Compact picker lists

Choose parent and Waiting use the same compact list spacing in menu subpages and standalone
popovers. Apply 2-pixel row padding without extra menu-specific row margins or minimum heights. Keep
labels inset by 8 pixels horizontally and 6 vertically.

Waiting omits the Apply button. Enter saves typed text, and activating a suggestion saves that
value. Both actions close the picker after a successful save. Tab only fills the entry.

## Clearing inherited status

Clear is an action, not a displayed status. It replaces the None menu item and appears only when an
ancestor supplies Someday, Maybe, or Waiting and the item hasn't already cleared that value.
Selecting Clear writes `none` to block inheritance, keeping the existing storage format. Retain
Inherited to remove an explicit declaration and resume inheritance.

An explicit or inherited clearing declaration displays as no status in task rows, search results,
and note metadata. Project controls use the same generic Status trigger as an unrestricted project.
Don't show a None chip, value label, or menu choice. Search continues to treat these items as having
no effective status. Keep the menu order Inherited, Someday, Maybe, Waiting, and Clear, omitting
irrelevant actions.

Waiting focuses its text box after the submenu opens, including when editing an existing value or
returning to the picker. Keep its current text and suggestion behavior.

## Recurring tasks

The [recurring tasks specification](recurring-tasks.md) extends the work view with occurrence notes.
An occurrence shares its series' project context and work status. Its body and outcome belong to
that occurrence. Ordinary subtasks retain their existing hierarchy and date inheritance.

## Editing task bodies

Choose **Edit task body** in a task row to open the full note editor for that task. This works
before and after setting up recurrence. Edit the series body before setting up recurrence to include
that text in its first occurrence. Use the parent link or Back to return to the project. Selecting a
task in search results continues to reveal its row in the project.

## Task details sidebar

An open task shows its body beside the same independently scrolling sidebar as a project. Share the
machine's sidebar width across both views. Show status, scheduled and due dates, and the parent
picker in the sidebar header. List subtasks with the same controls and completed filtering as
project tasks. The header has a completion checkbox for an ordinary task. For a recurring task,
complete the occurrence from its own note.

## Shared work editor controls

**New project** and **New standalone task** create and open blank notes with focus in the body
editor. Work editors show Status, Scheduled date, Due date, and Parent as rows with a label and a
control. An occurrence displays its inherited status and links to its series in the Parent row. Use
**Move…** to open the parent picker. In the task row menu, **Repeat…** follows the date actions. Use
the **Open task** or **Open subproject** arrow in a row to open its note.

Replace an empty Subprojects, Tasks, or Subtasks section with an add button for its item type. The
button creates a blank note and focuses its title in the new row. Once the section has an item, show
the title entry for adding more items below the list. Align both ends of that entry with the
corresponding title column. Omit Occurrence history when it has no entries.

## Work editor field layout

Place the overflow menu at the upper right of the sidebar. Use bold, right-aligned field labels and
normal-size labels and controls. Align the values at the start of the second column. Put the
completion checkbox first when the note has one. The parent link ends with a right-pointing arrow.

A task offers **Add recurrence…** in its menu and in the Recurrence field. When a rule is active,
the field summarizes its cadence and bounds. Select the summary to edit the rule.

## Work editor buttons

The editor offers actions through its fields and buttons. List rows keep their actions menus. Use
the same bold text, padding, and hover style for the editor's field buttons and its add buttons.
Keep the recurrence summary on one line and open occurrences from the history directly below. Open
the recurrence editor to apply a template to future occurrences. Occurrence editors offer skip,
reopen, and note actions as buttons where applicable.

## First additions and recurring search results

In an empty section, the add button reveals the section and focuses its title entry. Press Enter
with a title to create the first item. Keep the entry available for further additions.

Mark search results for tasks with an active recurrence. Selecting one opens the task's series
editor.

An occurrence's status button shows its inherited value. Use the recurring task's status picker to
change the series status.

## Subproject hierarchy and row titles

Projects use the same optional `completed-on` field, local completion date, and date search rules as
tasks. Completion applies to the project itself. Its descendants keep their own completion values.
Show a completion checkbox in project rows and in the open project's sidebar header.

Show nested projects in Subprojects and nested tasks in Tasks or Subtasks. Expand and collapse each
branch from its own row. A completed row stays visible when it has an unfinished descendant in its
section. When the open item has subprojects or tasks, place one right-aligned **Show completed**
checkbox on its own line before the lists. Apply its saved setting to both lists. Keep the expander
closed and unavailable when no children match the filter. Use separate columns for the expander,
completion checkbox, and title. Split the space between two columns equally into padding on either
side. Use half that spacing between the checkbox and title. Use less vertical padding than
horizontal padding. Put status and dates beneath the title and align them with its text. Hovering
over the checkbox or title highlights both with a shared background. The background starts halfway
between the expander arrow and checkbox. Align the leading edges of the **New subproject** and **New
task** text boxes with that background. Align the text inside each box with the leading edge of the
checkbox. Pressing either control darkens the background. Clicking the checkbox toggles completion.

Click a row title once to open its note. Double-click the title to rename it in place. Use the title
as the drag source and use the task drop zones for both sections. Dropping on the top or bottom
quarter inserts before or after the target under its parent. Dropping in the middle nests the item
under the target and expands the target. Validate project moves with the existing parent rules and
use the movable sequence for ordering. Move the parent link and sequence entry in one commit. The
dragged item's descendants keep their parent links and positions.

Right-click a project or task row to open its context menu. With focus in the row, use the context
menu key or Shift+F10 to open the same menu. Show status choices in the main project menu between
the date actions and **Move…**. Use the task menu's order and relevance rules. Offer **Rename…**
after **Move…** and before the delete action in both menus.

## Inline project status

Use a dropdown for the project status field. Selecting `Waiting...` sets the status to Waiting,
closes the dropdown, and focuses a text box to the right of **Waiting for:**. Press Enter or leave
the text box to save the reason. Tab completes text from earlier reasons. In search results and
other item details, show a Waiting reason as `Waiting for: <reason>`.

## Compact project details

Use the note editor's background for the project details sidebar. Replace the original 600-pixel
minimum with 360 logical pixels, or the width the controls need if they require more room. Let row
details wrap when the available width can't fit them on one line.

[decision]: ../decisions/ZIB0025-projects-and-tasks.md
[document]: space-document.md
[saved]: saved-searches.md
[types]: built-in-types.md
