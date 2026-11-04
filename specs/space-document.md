# Space document

This specification defines the Loro document that holds a space: its root containers, the keys in
each, and how each value's bytes look. It follows
[decision ZIB0001](../decisions/ZIB0001-storage-format-for-spaces.md) and the decisions it links to.
The [identifiers specification](identifiers.md) defines the text form of every identifier, and the
[schemas specification](schemas.md) defines the contents of `schemas`.

## Conventions

- **Containers.** Each root container has a fixed name and type. A value written as "map" below is a
  Loro map container, "text" is a Loro text container, and every other value is a plain Loro value:
  a string, a 64-bit signed integer, a boolean, or a list of plain values.
- **Keys.** Keys that Zib defines are lowercase words joined by hyphens.
- **Identifiers.** Identifiers are strings in their canonical text form, as keys and as values
  alike.
- **Timestamps.** A timestamp is an integer: milliseconds since the Unix epoch, in Coordinated
  Universal Time, with no time zone, from the writing machine's clock. Every time this specification
  records, such as when a machine created a note or removed another machine, is a timestamp.
- **Hashes.** A file's hash is its 32-byte [BLAKE3] hash, written as 64 lowercase hexadecimal
  digits.
- **Unknown keys.** A writer never removes or overwrites a key or root container that this
  specification doesn't define, and never rewrites a whole map from the keys it understands. A later
  addition to this specification may define it.

## Root containers

| Name         | Type | Holds                                                  |
| ------------ | ---- | ------------------------------------------------------ |
| `meta`       | map  | The space's own attributes                             |
| `notes`      | map  | Every note, keyed by its identifier                    |
| `tombstones` | map  | Notes deleted for good, keyed by their identifier      |
| `settings`   | map  | The space's synced settings                            |
| `schemas`    | map  | The definitions of note types, as `schemas.md` says    |
| `files`      | map  | An entry for each blob, keyed by its hash              |
| `machines`   | map  | The machines that share the space, keyed by identifier |

## `meta`

- **`id`:** string. The space's identifier. The machine that creates the space writes it in the
  space's first commit, and nothing changes it afterward.

## `notes`

Each key is a note's identifier, and each value is a map with these keys:

- **`type`:** string. The name of the note's type. A writer sets it when it creates the note and
  never changes it.
- **`body`:** text. The note's markdown, as the [note bodies specification](note-bodies.md) defines
  it.
- **`fields`:** map. Each key is a field's name, and each value is the field's value, encoded by its
  kind as [Field values](#field-values) describes. A field without a value has no key. The last
  write to each key wins.
- **`meta`:** map, with these keys:
  - **`created`:** timestamp. When a machine created the note.
  - **`created-by`:** string. The identifier of the machine that created it.
  - **`updated`:** timestamp. When its body or fields last changed.
  - **`updated-by`:** string. The identifier of the machine that last changed them.
- **`settings`:** map. The note's synced settings, as [Settings](#settings) describes. Changing a
  setting doesn't change `updated`.

### Field values

| Kind      | Value                                                                   |
| --------- | ----------------------------------------------------------------------- |
| text      | String                                                                  |
| integer   | Integer                                                                 |
| decimal   | String of an optional `-`, digits, and an optional `.` with more digits |
| boolean   | Boolean                                                                 |
| date      | String `YYYY-MM-DD`                                                     |
| time      | String `HH:MM` or `HH:MM:SS` on a 24-hour clock, and no time zone       |
| timestamp | Timestamp                                                               |
| link      | String: the target note's identifier                                    |
| file      | String: the blob's hash                                                 |
| choice    | String: one of the values the field's schema lists                      |
| list      | List of values of the list's item kind, each encoded as its kind is     |

A decimal keeps the digits the owner wrote, so `12.50` stays `12.50`, and readers compare decimals
by value. A list is one plain value, so the last write to the whole list wins.

### Writing notes

- **Creating.** A writer creates a note in one commit: its map with `type`, `body`, `fields`, `meta`
  with all four keys, and `settings`.
- **Editing.** An editor commits the owner's edits to a note 3 seconds after the owner pauses, and
  when the note closes. While the note is live, as the [sync specification](sync.md#live-editing)
  defines, it commits within 50 milliseconds instead.
- **Updated.** A writer sets `updated` and `updated-by` when it changes a note's body or fields, but
  while an editor has the note open, at most once a minute, and once more when the note closes. Each
  write stays in the document's history, so writing them on every commit would bloat it.
- **Deleting for good.** A writer deletes a note for good in one commit: it removes the note's key
  from `notes` and adds its entry to `tombstones`.
- **Moving.** Moving a note to another space removes it from this space with no tombstone, as
  [decision ZIB0005](../decisions/ZIB0005-note-identifiers-and-links.md) describes.

## `tombstones`

Each key is the identifier of a note deleted for good, and each value is a map:

- **`title`:** string. The note's last title, as markdown with its span formatting.
- **`deleted`:** timestamp. When a machine deleted the note.
- **`deleted-by`:** string. The identifier of the machine that deleted it.

A writer never removes a tombstone. A link to its identifier shows its title, styled as deleted.

## Settings

The space's `settings` and each note's `settings` map keys to plain values. Each key starts with the
name of the implementation that reads it and a period, such as `linux.` or `android.`, or with
`zib.` for a setting every implementation shares. The last write to each key wins. The specification
for each setting defines its key, its value, and its default, and a key that's absent takes its
default.

## `files`

Each key is a blob's hash, and each value is a map:

- **`size`:** integer. The blob's size in bytes.
- **`media-type`:** string. The blob's media type, such as `application/pdf`.
- **`holders`:** map. Each key is the identifier of a machine that holds the whole blob and has
  checked its hash, and each value is the timestamp when it began to hold it. A machine adds and
  removes only its own key.

A machine that adds a blob writes its entry, with itself as a holder, in the commit that first
refers to the blob. [Decision ZIB0004](../decisions/ZIB0004-storage-and-sync-of-files.md) defines
when a machine drops a blob and when an entry leaves the map.

## `machines`

Each key is a machine's identifier, and each value is a map:

- **`key`:** string. The machine's 32-byte Ed25519 public key, as 64 lowercase hexadecimal digits.
- **`name`:** string. The machine's name, which any member can change.
- **`policy`:** string. `all` or `on-demand`, as decision ZIB0004 defines them. It never changes.
- **`removed`:** timestamp. When a member removed the machine. A machine without this key is a
  member. No writer ever removes this key once it's set, and members treat the machine as removed
  even if a later write changes its other keys.

## Loro

- **Encoding.** Machines exchange updates and snapshots in Loro's binary encoding, as Loro 1.0 and
  later define it. Each implementation stores the document on its machine as it chooses.
- **Peers.** Each process that opens a space picks a new random Loro peer identifier each time, so
  no two writers ever share one.
- **Commits.** Each change in this specification that says "in one commit" happens in a single Loro
  commit, so other machines receive the whole change or none of it.

## Platforms

No platform differs.

[blake3]: https://github.com/BLAKE3-team/BLAKE3
