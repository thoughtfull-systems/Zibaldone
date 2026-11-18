# ZIB0026 Name date and timestamp fields

## Context

The owner wants field names to distinguish calendar dates from timestamps. The project and task
implementation remains uncommitted, and its earlier boolean completion values exist only in a test
database the owner can discard. A modification timestamp doesn't record when a task finished.

## Decision

Use `-on` for calendar dates and `-at` for timestamps. Tasks and projects use `scheduled-on` and
`due-on`. Tasks use a date named `completed-on`, with absence meaning incomplete. Set the local
calendar date when completing a task and clear it when reopening. Completion dates don't inherit,
and subsequent edits don't change them. Don't invent dates for experimental boolean completions.

Note metadata uses `created-at` and `updated-at`, and tombstones use `deleted-at`. New writes use
these names. Readers fall back to the earlier metadata keys for older documents, preserving those
keys without rewriting history. This changes what older readers see in new metadata; the owner
accepts resetting experimental data rather than requiring mixed-version writes or a migration.

Existing source fields already follow the convention: `captured-at` is a timestamp, and
`published-on` is a date. Keep their names and meanings.

Use `id` for a note's identifier in search and metadata controls. Use `parent-id` for a project or
task's parent link. The identifier stays the note's key in the space's map, rather than becoming an
additional stored field.

Expose `created-at` and `updated-at` in the metadata popup, search conditions, and field-name
completion for every note type. Index their metadata values rather than adding schema fields or
copying them into `fields`. Use the existing timestamp input rules. Hide `file`, `text`, and
`thumbnail` in the popup while preserving their stored values and search behavior.

The [field inventory][fields] lists built-in fields and metadata. The [space document][document]
defines metadata storage, and the [projects and tasks specification][work] defines completion.

[document]: ../specs/space-document.md
[fields]: ../specs/note-fields.md
[work]: ../specs/projects-and-tasks.md
