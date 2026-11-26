# ZIB0025 Organize work around projects and tasks

## Context

The owner wants project notes for design work, nested projects and tasks, and standalone tasks.
Search results need to open a task in its project. Scheduled and due dates inherit independently.
Explicit dates on subtasks and subprojects take precedence. Ordinary tasks should work before
recurrence arrives.

Note types never change, fields sync independently, and the search index derives from space data.
Concurrent offline moves can create cycles even when each machine rejects local cycles.

## Decision

Add dedicated `project` and `task` note types. Store explicit `scheduled-on` and `due-on` date
fields and a `parent-id` link on both. Both types have a `completed-on` date. A missing completion
date leaves the item incomplete. The owner replaces an ordinary note by creating a project and
deleting the old note.

Parents belong to the same space. A task can have a task or project parent; a project can have a
project parent. Each item has at most one parent. Reject local moves that create cycles or invalid
parent kinds. Derive a forest after sync: ignore missing or invalid targets, and ignore the outgoing
parent edge of the largest identifier in each cycle. Keep stored links intact. Every replica derives
the same forest without issuing repair writes that can conflict with another move.

An unset date inherits the nearest ancestor's valid value. Resolve scheduled and due dates
independently over the derived forest. Store no inherited copies. An explicit subtask or subproject
date can precede a parent's scheduled date or follow its due date. The initial design has no
inheritance opt-out.

Completion dates never inherit. Checking an item sets today's local date, and reopening it clears
the date. Later edits don't change it. The owner confirms that the earlier boolean completion model
exists only in disposable test data, so this implementation doesn't convert those booleans to dates.

Search indexes effective dates, normalized parents, and completion dates. A virtual `project`
condition matches any ancestor project, including through subprojects. Parent edits, deletions, and
moves refresh dependent index entries. The first implementation refreshes work items in the space
when hierarchy or effective dates change. Body edits and task completion update only the edited
item's entries. Narrower invalidation can follow without changing the format.

The work view selects unfinished tasks whose effective scheduled date is absent or has arrived. It
includes parent tasks alongside their subtasks. It sorts by effective due date with undated items
last. The owner can edit the saved query. Completing a parent doesn't complete or block its
subtasks. [ZIB0027](ZIB0027-work-status.md) adds inherited work status.

Store manual sibling order in a space-wide Loro movable list of work-item identifiers. Filter that
sequence by the derived parent when showing subtasks and subprojects. Keep hierarchy links
authoritative: concurrent parent moves select one parent, and sequence entries never create
membership. This lets independent reorders merge without replacing a whole sibling list. Legacy
items without positions fall back to title order after positioned items. Materialize missing
positions before a move or creation, and append newly created items. Ignore duplicate identifiers on
read and remove extra live occurrences before local sequence edits. Preserve entries for missing
notes so deletes and older writers don't require order repairs.

Dragging an item's title moves its existing sequence entry and can change its parent in the same
commit. Its descendants keep their links and positions. The desktop distinguishes row edges for
sibling placement from the row's center for nesting, and rejects self-drops and descendant targets.
Show a line for insertion and a highlighted row for nesting.

Use the same move operation and drop zones for subprojects and tasks. Validate project parents with
the existing hierarchy rules and use the movable sequence for sibling order. This keeps project
subtrees intact as they move and lets concurrent moves merge through the existing parent links. Keep
tasks and projects in separate lists and reject drops between their rows.

Show a Subprojects heading and inline additions beneath the subproject and task lists. Each addition
has a plain title text box aligned with the list's titles, even when the list is empty. Enter
submits without navigating away or showing a dialog. Omit Add buttons and the Add subtask menu
action. Retain focus and preserve drafts across list refreshes so the owner can add items in
sequence. Drag items to create subtasks and nested subprojects.

Show project controls, subprojects, and tasks in a permanent right sidebar beside the project note.
Give the note and sidebar separate scrolling. Use an equal split before the owner chooses a width,
keep the note at least 280 logical pixels wide and the sidebar at least 600, and cap the sidebar at
70% of their usable width. Keep task controls on one row to avoid wrapping at the minimum. Store the
chosen width in the machine configuration and reuse it across projects. Changing window size clamps
the presentation without rewriting that preference. Standalone tasks keep their inline presentation.
The [projects and tasks specification][work] defines the setting and bounds.

Selecting a task opens its nearest ancestor project, or its root task when no project exists. The
project editor expands its ancestor path and shows the selected task's inline title control.
Preserve the result list and the selected task when completion removes it from that list. Completed
ancestors remain visible when they contain unfinished tasks. The hierarchy offers a show-completed
control.

Use the local calendar date for `today` and urgency. Due dates in the next three days count as due
soon. Refresh open views after a date change. The first work item in a space creates editable All
tasks, Projects, and work saved searches, unless searches with those names already exist.

Task rows keep their parent picker in the overflow menu as Choose parent. Tasks can still change
parents through drag and drop. Reuse the searchable project picker within the same popover. Group
the drag handle, expander, and checkbox with reduced horizontal spacing. Projects keep their parent
controls beside their dates.

Hide a task date control when no explicit or inherited value exists. Offer Add schedule date and Add
due date in the overflow menu for missing values. Reuse the date picker within that menu. Setting a
date shows its inline control; clearing an override preserves an inherited control or removes it
when no effective date remains.

Deleting a task deletes its descendant tasks. Deleting a project deletes its descendant projects and
tasks, including completed items. Snapshot the derived subtree and write all removals and last-title
tombstones in one commit. Apply the same operation through every deletion entry point. Remove all
deleted identifiers from search, navigation history, and cached drafts. Moving a parent to another
space still leaves its subtasks and subprojects in the source space.

The deletion covers descendants present on the deleting replica. Sync imports those removals without
expanding them to subtasks or subprojects created concurrently elsewhere. This keeps merging
deterministic and avoids deleting unseen work. A concurrent subtask or subproject whose parent
disappears becomes a root.

## Rejected alternatives

- **A project role on ordinary notes.** The owner prefers dedicated types and accepts creating a new
  project to replace an existing note.
- **Copying inherited dates into subtasks or subprojects.** This loses the distinction between an
  override and an inherited value, and creates redundant sync writes when a parent changes.
- **Full note editors in task rows.** The owner wants compact title inputs and date pickers. Keep
  existing task body content while editing only the title within the project list.
- **Keeping descendants after deleting their parent.** The owner wants deletion to remove the
  complete branch. Move subtasks or subprojects elsewhere before deleting a parent to retain them.
- **Repair writes after concurrent cycles.** Independent repair writes can conflict with moves and
  erase an authored parent. Deterministic read rules converge without changing stored intent.
- **Waiting for recurrence.** Ordinary task organization provides a usable milestone and exercises
  the search and editor workflow before adding occurrence lifecycles.

## Consequences

The [projects and tasks specification][work] defines storage, derived hierarchy, and search
behavior. Moving an item to another space can detach its parent relationship. Deleting a work item
removes its known subtree; moving it keeps its subtasks and subprojects. Recurrence remains separate
work.

[work]: ../specs/projects-and-tasks.md
