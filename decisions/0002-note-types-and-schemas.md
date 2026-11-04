# 0002 Note types and their schemas

## Context

[Decision 0001](0001-storage-format-for-spaces.md) gives every note a `type`, which never changes,
and a `fields` map that holds the attributes its type defines, such as a book's author. Search and
filters use these fields, so each implementation needs to know what kind of value each field holds.
Decision 0001 also reserves a `schemas` key in each space for the definitions of the types the space
uses, and leaves their form to this decision.

Types come built into the app. Nobody creates types through the interface. The set of types and
their fields still grows from one release to the next, and machines don't update in step. A phone
can run an older release than a laptop for weeks, and both sync the same space in the meantime. Each
release has to read what a newer one wrote, and the reverse, without losing anything when it edits a
note.

## Criteria

- **Backward compatible.** A newer release reads, shows, and searches every note that an older
  release wrote.
- **Forward compatible.** An older release reads, shows, and searches the notes and fields that a
  newer release wrote, and editing such a note never loses data.
- **Merge-safe.** Concurrent edits from two releases merge as cleanly as edits from one.
- **Searchable.** Each field's definition says how search indexes and filters it.
- **Low ceremony.** Adding a type or a field to a release takes a change to its definitions, with no
  migration of existing notes.
- **Room for user-defined types.** The design doesn't rule out creating types in the app later.

## Decision

Each release defines its types in code. It also writes each type's definition, its schema, into
every space it opens, so a release that doesn't know a type or field learns about it from the space.
A later specification defines the exact encoding. In outline:

- **Type names.** Each type has a name of lowercase words joined by hyphens, such as `file` or
  `book`. A name never changes and never gets reused for a different type.
- **Schemas.** A schema lists the type's fields. Each field has a name, which follows the same rules
  as type names, a kind, and a search mode.
  - **Kinds:** text, integer, decimal, boolean, date, time of day, date and time, a link to a note,
    a file, as [decision 0004](0004-storage-and-sync-of-files.md) defines it, a choice from a fixed
    set of values, and a list of any of these. A time of day has no date or time zone, so a journal
    prompt set for 07:00 comes up at 07:00 wherever the owner is.
  - **Search modes:** `text` indexes the value for full-text search, `filter` indexes it for exact
    matches, ranges, and sorting, and `none` leaves it out of the index.
  - A schema can mark a field as deprecated. Releases stop offering it for new values but keep
    reading, showing, and indexing the values that exist.
- **Versions.** Each schema has a version number, starting at 1. A new version may only add fields,
  add values to a choice, and deprecate fields. It never removes a field, renames one, or changes
  its kind. A field that needs a new kind gets a new name. Once a release ships a version, its
  content never changes.
- **Storing schemas.** `schemas` maps each type name to a map from version number to schema. When a
  release opens a space, it writes each version it knows that the space lacks. Two machines that
  write the same version at once write identical content, so the merge makes no difference. Keying
  by version keeps an older release from overwriting a newer schema.
- **Reading notes.** A release reads every note against the highest version of its type in the
  space, which contains every field any version defined. Notes record no version. Every field is
  optional, so a note that lacks a field is valid in every version.
- **Unknown types and fields.** A release shows a note of a type it doesn't know as a generic note:
  its body, and its fields as the space's schema describes them. It indexes those fields by their
  search mode, and edits them with a generic editor for their kind. It doesn't create notes of a
  type it doesn't know.
- **Values that don't match.** A release that finds a value that doesn't match its field's kind,
  such as text in a date field, leaves it alone. It shows and indexes nothing for that field until
  someone sets a valid value.

Against the criteria:

- **Backward compatible:** fields never disappear or change kind, so every value an older release
  wrote still means what it did, and the newest schema still describes it.
- **Forward compatible:** an older release learns new types and fields from the space's schemas.
  Edits write single keys in the `fields` map, so editing one field never touches the fields the
  release doesn't know.
- **Merge-safe:** schema versions never change once written, so concurrent writes agree. Notes merge
  field by field as decision 0001 defines, whichever release made each edit.
- **Searchable:** every field carries a kind and a search mode, which together tell the index what
  to do with it, even for fields the release has never seen.
- **Low ceremony:** a new field is a new entry in the next schema version. No release rewrites
  existing notes.
- **Room for user-defined types:** schemas already live in the space as data. A later decision can
  let the owner write them, if the app grows an interface for it.

## Rejected alternatives

- **Types in code only, with no schemas in the space.** An older release would keep fields it
  doesn't know, because edits touch single keys, but it couldn't show, search, or filter them, and
  it couldn't show a note of a new type as anything but its body. It fails the forward compatible
  criterion.
- **Migrations that rewrite notes to a new schema.** Releases could rename fields and change kinds
  freely. But every machine that upgrades would run the migration on the same notes, and the
  rewrites would merge with edits from machines still on the old release, leaving a mix of old and
  new fields. It fails the merge-safe and low ceremony criteria, and an older release can't read
  migrated notes, which fails forward compatible.
- **A schema version on each note.** Readers would know which fields a note's writer understood. The
  additive rule makes this unnecessary, because the newest schema describes every note, and a
  version on each note would need a rule for merging two edits that write different versions.
- **JSON Schema.** It's a standard with validators in many languages. But it describes the shape of
  values, not how to search them, so Zib would extend it anyway. It allows changes this decision
  forbids, and every implementation would need a validator for a large language. It fails the low
  ceremony criterion and adds little to searchable.
- **Untyped fields, like front matter in markdown files.** Any key could hold any value, with no
  schema at all. Search couldn't tell a date from text, and a typo in a field name would create a
  new field. It fails the searchable criterion.
- **Types that the owner defines in the app now.** This is the most flexible option, but it needs an
  interface for editing types and rules for merging two machines' edits to the same type. The owner
  doesn't need it, and the schemas in the space leave room for it later.

## Consequences

- Every release carries the schemas of every type it knows, and writes any version a space lacks
  each time it opens it.
- Each implementation needs a generic view and editor for each kind, for types and fields it doesn't
  know.
- A field's mistakes stay in the schema forever. A field with the wrong kind gets deprecated and
  replaced, so naming and kinds deserve care before a release ships them.
- The `schemas` map grows by one entry for each version of each type, which stays small.
- Required fields can't exist, so every release handles a note that lacks any field.
- A later specification defines the encoding of schemas and of each kind's values, and lists the
  built-in types.
