# ZIB0027 Defer work with inherited status

## Context

Projects and tasks need Maybe, Someday, and Waiting states. Waiting includes free text with
completion from earlier values. These states exclude tasks from the work view. The owner wants
subtasks and subprojects to inherit status while allowing an explicit override or no status.

## Decision

Add an optional `status` choice and `waiting-for` text to projects and tasks. Store lowercase
`maybe`, `none`, `someday`, or `waiting` in `status`. An absent status means inherit, or available
when no ancestor has a status. Explicit `none` stops inheritance without deferring the item. Waiting
text is optional; an empty value means Waiting without detail.

Resolve status from the item toward the root, taking the nearest valid explicit status. Resolve
Waiting text at that same source. None stops the walk and applies to descendants that inherit it.
Inherit removes the explicit value and resumes the walk. Keep date and completion rules unchanged.

Store earlier Waiting text in a space's additive `waiting-values` root map. Each trimmed value is a
key with boolean true. Never remove entries when status changes or a note disappears. This retains
completion candidates after clearing or deleting work and merges independent additions. The desktop
combines candidates from all spaces held on the machine, matches without case sensitivity, and lets
the owner enter a new value. Keep status and Waiting text as separate schema fields, following the
existing independent-field sync model. Write a local status change and its history together in one
commit. After concurrent edits, ignore Waiting text unless the effective status is Waiting.

Index effective `status` and `waiting-for` values. Treat explicit or inherited None as missing
status in search, so it remains eligible for work. Add `@status!?` to the initial work query.
Upgrade an existing untouched default work search once per space, preserving customized definitions.

Expose status in the project controls and task overflow menu. Show an effective status beside task
dates. Identify inherited sources. Offer Someday, Maybe, and Waiting. Clear suppresses an inherited
status and appears only when such a status exists and the owner hasn't cleared it. Keep `none` as
the stored inheritance barrier, and display it as no status. Inherited removes an explicit
declaration and resumes inheritance. Show the menu in the order Inherited, Someday, Maybe, Waiting,
Clear. Omit the current explicit fixed choice, keeping Waiting available to edit its text.

## Rejected alternatives

- **Ancestor status always wins.** This prevents the explicit overrides and None choice the owner
  wants. The nearest declaration preserves inheritance while allowing both choices.
- **Only current values for completion.** Clearing the last use loses a value the owner wants to
  reuse. A synced additive history retains it without scanning document history.
- **Separate booleans for each state.** These permit conflicting states and make the associated
  Waiting text harder to interpret. One choice expresses the current state.

## Consequences

The [work specification][work] defines status inheritance and the [space document][document] defines
history storage. The field inventory includes the new names. Recurrence remains separate work.

[document]: ../specs/space-document.md
[work]: ../specs/projects-and-tasks.md
