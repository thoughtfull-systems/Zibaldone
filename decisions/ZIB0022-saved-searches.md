# ZIB0022 Save searches in spaces

## Context

The desktop search box combines text and typed conditions. The owner wants to save a query, its
space scope, and its sort order, then browse its results beside the note editor. The query editor
shows the selected definition and offers Save and Save copy after edits.

[ZIB0003][machine] places content preferences in spaces and screen preferences on the machine. A
saved search can span spaces, whose pet names differ between machines. Its definition needs a stable
home and stable scope references. Concurrent edits must preserve a complete authored query, scope,
and ordering.

## Decision

Store saved searches in a `saved-searches` root map in a space. Each entry has its own identifier
and holds one plain map value containing the name, query, scope, and sort. The complete value uses
Loro's last-write-wins rule. An update preserves unknown keys in the value.

Save copy creates an identifier in the destination space the owner chooses. A new search defaults to
the currently searched space or the space that new notes go into. A saved search syncs with its home
space, even when its scope names another space. Scope uses a space identifier or all spaces, and a
machine reports an unavailable explicit scope rather than substituting another space.

Save checks that the stored value still matches the value the editor loaded. If sync changes it, the
editor keeps its draft and offers a copy. Concurrent offline saves still merge as complete values.
They don't combine one device's query with another device's scope or sort order.

Definitions describe searches, not result membership. Each machine runs the query against its local
indexes. The desktop keeps its open definition, unsaved drafts, selection, and scroll position in
the window's session. Closing and reopening the window within the running instance retains that
session. Restarting the instance opens a new search. Store the sidebar's width and visibility in the
machine configuration, apart from synced search definitions. Use a divider for resizing and hide the
sidebar at 100 logical pixels or less, retaining its last usable width for the toggle button.

## Rejected alternatives

- **A dedicated note type.** A saved search has no note body to edit or show in the note list. A
  separate collection keeps it outside ordinary note results and supports atomic definition edits.
- **Machine settings.** These keep screen preferences local but can't share saved queries between
  machines.
- **Separate registers for query, scope, and sort.** Concurrent edits could produce a definition
  neither device authored. One value preserves the chosen combination.

## Consequences

- Each saved search has one home space. Removing that space from a machine also removes access to
  its saved searches on that machine.
- Selecting a search doesn't change the machine's space visibility preferences for other features.
- The [saved searches specification][saved] defines the stored value and desktop behavior.

[machine]: ZIB0003-space-identity-and-machine-settings.md
[saved]: ../specs/saved-searches.md
