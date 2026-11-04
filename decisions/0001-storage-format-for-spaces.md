# 0001 Storage format for spaces

## Context

Zib holds linked markdown notes and binary files: PDFs, images, and other documents. It's
local-first: each machine keeps its own copy of the data, works fully offline, and syncs changes
with the other machines that share it. One person uses Zib, but on more than one machine, so the
same note can change on a laptop and a phone before either one syncs.

Notes and files belong to spaces that keep them apart by purpose: personal notes in one space, and
work notes in another. A machine holds only the spaces it shares, so a work machine can hold the
work space without the personal one.

A note has more than its text:

- **Metadata** about when and where someone created and last changed it.
- **Fields** that depend on the note's type, such as a due date or an author, which search and
  filters use. A later decision defines types and their fields.
- **Settings** that sync with the note, such as how the app shows it.

Each space also has settings that sync to every machine that shares it, and each machine has
settings of its own that never sync. Later decisions cover the machine's settings and the bytes of
binary files. This decision covers how a space stores its notes, their attributes, and its settings,
and how concurrent changes merge. It's the format every implementation reads and writes, so it's the
hardest choice in Zib to reverse.

The sibling `vibes` repository built Commonplace, an earlier personal knowledge management app, on
the same principles. Its decisions 0002, 0005, and 0007 chose a [Loro] document per library, a
`settings` map per note, and one library per space. This decision weighs the options afresh, using
Commonplace's reasoning and experience as evidence.

## Criteria

- **Lossless merging.** Concurrent edits on two offline machines merge without losing either side,
  down to edits in the same note's text.
- **Attributes beside the text.** Metadata, fields, and settings live outside the markdown body, and
  each merges on its own, so a change to a field on one machine and to the text on another both
  survive.
- **Every platform.** Linux, Android, and a web app in a browser can all read and write the format,
  offline, and the choice doesn't rule out Apple or Windows platforms later.
- **No server.** Sync needs nothing more than a way to move bytes between machines.
- **Scales to a lifetime of notes.** A space with tens of thousands of notes and years of history
  opens quickly and stays a manageable size on a phone.
- **Durable.** The data outlives the app: the format is open and stable, and a space can export to
  plain markdown files.

## Decision

Store each space as a single Loro document, a Conflict-free Replicated Data Type (CRDT). A later
specification defines the exact containers and keys. In outline:

- **`notes`** maps each note's identifier to a map with these keys:
  - **`type`:** the note's type. It never changes.
  - **`body`:** the note's markdown, in a Loro text container that merges concurrent edits character
    by character.
  - **`fields`:** a map from field name to value, holding the attributes that the note's type
    defines. The last write to each field wins.
  - **`meta`:** when and on which machine someone created the note, and when and on which machine
    someone last changed its body or fields. Timestamps are the writing machine's clock, in
    milliseconds since the Unix epoch, in Coordinated Universal Time (UTC).
  - **`settings`:** a map of preferences that sync with the note. Changing one doesn't update
    `meta`, so lists sorted by modification don't reorder.
- **`settings`** holds the space's synced settings. The last write to each key wins.
- **`schemas`** holds the definitions of the note types that the space uses.
- **`meta`** holds the space's identifier, as
  [decision 0003](0003-space-identity-and-machine-settings.md) defines it.

Each key in either `settings` map starts with the name of the implementation that reads it and a
period, such as `linux.` or `android.`, or with `zib.` for settings every implementation shares.

Each implementation decides how to keep the document on its machine, as long as it can load it, save
it, and exchange updates in Loro's encoding. A later decision defines identifiers for notes, how
notes link to each other, and the sync transport.

Against the criteria:

- **Lossless merging:** Loro merges concurrent edits to text, maps, and lists, and keeps the history
  that makes it possible.
- **Attributes beside the text:** each field and setting is its own map key, so concurrent changes
  to different keys never conflict, and none of them touch the body.
- **Every platform:** Loro is a Rust library with official packages for JavaScript in the browser,
  through WebAssembly, and for Swift. Kotlin can reach it through Rust bindings.
- **No server:** a Loro update is a self-contained byte string that any channel can carry.
- **Scales to a lifetime of notes:** Loro loads snapshots lazily, so opening a large document
  doesn't decode every note, and shallow snapshots can drop old history once every machine has it.
- **Durable:** Loro's encoding has stayed stable since version 1.0, and its code is open source
  under the MIT license. An export writes each note as a markdown file with its attributes in front
  matter.

## Rejected alternatives

- **A folder of markdown files with front matter, synced by a file sync tool.** It scores best on
  the durable criterion, and other tools can read the notes directly. But when two machines edit the
  same file, a file sync tool keeps one copy and sets the other aside as a conflict, which fails
  lossless merging. Front matter mixes attributes into the text file, so a field change and a text
  edit on two machines conflict, which fails attributes beside the text. A browser also can't open a
  folder on the machine without a server or an interface that only Chromium offers, which fails
  every platform.
- **Markdown files as the source of truth, with a CRDT that tracks them.** The app would keep a CRDT
  for merging and write each note out as a file, so other tools could edit the files too. Edits made
  outside the app would then have to turn back into CRDT operations by comparing text, which loses
  intent and can't tell concurrent edits apart from sequential ones. It weakens lossless merging,
  and it doubles the storage. The export covers reading the notes without Zib.
- **Automerge.** Automerge merges text and maps as well as Loro does, has a longer track record, and
  passes the lossless merging, attributes beside the text, no server, and durable criteria. It loads
  a whole document into memory to open it and has no equivalent of shallow snapshots, so it does
  worse on scales to a lifetime of notes. It also lacks a movable tree, which closes off nested
  notes or outlines later without a custom structure on top.
- **Yjs or its Rust port, Yrs.** Yjs is mature and widely used, and it merges text well. It has no
  movable tree and no way to trim history, and its maps hold no typed values beyond JSON. It does
  worse than Loro on scales to a lifetime of notes for the same reasons as Automerge.
- **SQLite with a CRDT extension, such as cr-sqlite.** Tables and queries suit typed fields and
  search well. But cr-sqlite merges rows and columns, not the characters of a text value, so two
  edits to the same note's body keep only one of them, which fails lossless merging. Its development
  has also slowed, and its browser build depends on a WebAssembly port of SQLite.
- **A SQLite database with a custom sync protocol.** SQLite runs everywhere and queries fast, but
  merging rows that two machines changed needs a custom rule for every table, and merging text needs
  a CRDT anyway. It fails lossless merging unless Zib reinvents a CRDT.
- **One Loro document per note.** Small documents load and sync on their own, and a machine could
  hold a subset of notes. But a change that spans notes, such as renaming a type's field across a
  space, has no atomic merge, and sync has to compare thousands of version vectors. Nothing so far
  requires a machine to hold only some of a space's notes, and files, the large items, can stay
  outside the document. It does worse on scales to a lifetime of notes.

## Consequences

- Every implementation depends on Loro, through its Rust crate, its JavaScript package, or bindings.
  A decision on languages and technologies needs to account for that.
- Only one process at a time may write a space on a machine, so each implementation guards against a
  second instance.
- A space's document keeps its full history and grows over time. Trimming history with shallow
  snapshots needs a rule for when every machine has caught up.
- Search over fields and text needs an index on each machine, built from the document, because Loro
  has no query language. The index stays on the machine and never syncs.
- The last write to a field wins, and deleting a note while another machine edits it discards the
  edits. A later specification can soften this with a trash that keeps deleted notes for a while.
- Note types and their schemas, space identity and machine settings, file storage, note identifiers
  and links, the sync transport, and the export format each need a follow-on decision.

[loro]: https://loro.dev/
