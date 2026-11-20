# Saved searches

This specification defines saved queries and their desktop editor, following
[decision ZIB0022][decision]. The [space document specification][document] defines the underlying
containers, and the [identifier specification][identifiers] defines identifiers.

## Storage

The optional `saved-searches` root container is a Loro map. Each key is a saved search identifier.
Writers generate and encode it like other identifiers. Each value is a plain map with these keys:

- **`name`** holds the search's name as a string with at least one character other than whitespace.
- **`query`** holds the search expression as text, including its line breaks. An empty query lists
  every note in scope.
- **`scope`** holds null for every space the machine holds, or a canonical space identifier for
  exactly that space. It never holds a pet name.
- **`sort`** holds `modified`, `relevance`, or `title`. Modified sorts newest first. Title uses the
  search index's case-insensitive title order. Relevance ranks text matches, with modification time
  and identifier as tie breakers. A query without positive text matches sorts by modification time.

Each entry syncs as one value. Writers change only the keys they know and preserve unknown keys.
Readers leave malformed entries and unknown values untouched and omit them from the picker. Creating
or saving an entry commits its whole value in one Loro commit. A saved search has no note body, and
the note index doesn't include it.

Save checks the current value against the value loaded by the editor. A mismatch fails with a
message that the search changed. The draft remains available for Save copy. Two offline saves can
both succeed before exchanging updates. Loro resolves their complete values with its map register
ordering.

## Scope

The home space determines where a saved search syncs. Scope determines which notes it searches. An
explicit scope that this machine doesn't hold produces no results and shows an unavailable space. It
never expands to all spaces. A scope of all spaces includes new spaces as they arrive. Selecting a
saved scope doesn't alter machine visibility preferences for other features.

## Range conditions

Saved queries accept the bounds and endpoint rules in the [search ranges specification][ranges].

## Desktop sidebar

A button beside the note editor toggles the search sidebar. Hiding it preserves the definition,
results, and open note. The sidebar begins with a dropdown of saved searches from every space the
machine holds, sorted by name with stable identifiers as tie breakers. Space names distinguish
entries when the machine holds multiple spaces. New search opens an unsaved definition.

The definition uses a multiline text editor. Field completion applies at its cursor, including
within grouped expressions. The result list follows the definition, save controls, scope, and
sorting. Editing reruns the query and marks the draft as changed. Invalid queries show an error
instead of results. An invalid query can't replace a saved search.

Save updates the selected search and preserves its identity and home space. Save copy asks for a
name and destination and creates a separate search. Saving a new search also asks for a name and
destination. Scope or sort changes count as edits alongside query changes.

Switching searches keeps a draft for each edited definition during the window's session. Opening a
result changes the note editor and preserves the search. Refreshing results preserves the selected
note and scroll position when possible. If the open note stops matching, its editor stays open.

## Platforms

The Rust core reads and writes the same format for every platform. The Linux app provides the
sidebar described here.

## Due-date sort

The `sort` value also accepts `due` as the [projects and tasks specification][work] defines. It
orders effective due dates first and keeps undated items last. Older readers that don't recognize
this value preserve the saved search but omit it from their picker.

## Query wrapping

The search definition wraps without inserting display hyphens. Literal hyphens in field names and
values remain visible as part of the query.

## Sidebar layout on the machine

Resize the search sidebar by dragging its divider. Its width and visibility belong to this machine
and don't sync with saved searches. The optional `search-sidebar` object in `machine.json` holds
`visible`, a boolean, and `width`, a positive integer in logical pixels. Without the object, show
the sidebar at 320 logical pixels. A stored width of 100 or less, or greater than the largest signed
32-bit integer, falls back to 320.

Dragging the divider to 100 logical pixels or less hides the sidebar. Keep its last saved usable
width so the toggle button restores it. Save width changes 500 milliseconds after the latest
movement, and flush pending layout writes when closing the window or quitting. Hiding the sidebar
preserves the query, drafts, results, and open note.

## Result presentation

Every result shows its title with a small note-type pill beside it, including ordinary notes. The
next row shows applicable status, scheduled date, and due date. Breadcrumbs follow, including the
space when results span spaces. Show all matching passages beneath the breadcrumbs, indented from
the title. Preserve match highlighting, escape markup in source text, and avoid a fixed subtitle
line limit. Keep distant matches and matches from different fields or search clauses. Date and
status controls open their pickers directly without navigating away from results.

## Recurrence date sorting

The sort value `recurrence` orders occurrence dates newest first, followed by notes without an
occurrence date. Break ties by note identifier. See the [recurring tasks specification][recurring].

[decision]: ../decisions/ZIB0022-saved-searches.md
[document]: space-document.md
[identifiers]: identifiers.md
[ranges]: search-ranges.md
[recurring]: recurring-tasks.md
[work]: projects-and-tasks.md
