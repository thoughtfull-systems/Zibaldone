# ZIB0028 Give task occurrences their own notes

## Context

A recurring task needs one reusable body and an addressable copy for each occurrence. The owner can
check items in the copy, attach notes about completing or skipping it, and revisit it through search
and backlinks. Two machines can create the same occurrence while offline.

## Decision

Give each realized occurrence a `task` note under a stable identifier derived from the series, rule
revision, and calendar date or cycle. The `series-id` link marks it as an occurrence. Initialize its
note and shared merge base with identical Loro operations on each replica. Both replicas can then
edit the same body and publish links to the same address.

Use independent records for rule revisions and occurrence actions. Keep the first advancement when
someone reopens an occurrence. Merge competing first advances by choosing the earlier successor
date. Preserve date overrides and body edits independently of that choice.

The occurrence's identity supplies its series membership and search fields. Seed its body from the
series' current body with unchecked checklist items. Seed changes from the shared merge base with a
writer derived from the occurrence identifier and body digest. Replicas with the same starter text
share text operations. Distinct seed writers let replicas merge different starter text and edits
without reusing operation identifiers. Store completion and skip dates in their outcome records and
write `superseded-on` on replaced occurrences. The work search filters by these dates and excludes
series templates by the presence of `recurrence`.

See the [recurring tasks specification][recurrence] for the format and date rules.

## Rejected alternatives

- **Embed each body in the series.** Each copy would need a separate way to open it, link to it, and
  index it for search.
- **Generate random identifiers for occurrence notes.** Concurrent creation would produce two
  addresses for one occurrence. Resolving that conflict would require aliases and a text merge
  across independently initialized bodies.

## Occurrence navigation and subtasks

The series editor's history provides access to every occurrence. The occurrence editor links to its
series in the parent field.

Allow subtasks to belong to an occurrence. Each subtask keeps its own state and inherits dates and
status through that occurrence. Later occurrences start with their own hierarchy. A subtask search
result opens the nearest occurrence or project that can display its row. Resolve concurrent cycles
by cutting explicit parent links so the occurrence keeps its series association.

[recurrence]: ../specs/recurring-tasks.md
