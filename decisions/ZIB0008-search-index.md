# ZIB0008 Library for the search index

## Context

Loro has no query language, so [decision ZIB0001](ZIB0001-storage-format-for-spaces.md) leaves
search to an index on each machine, built from the space's document.
[Decision ZIB0002](ZIB0002-note-types-and-schemas.md) gives each field a search mode: `text` for
full-text search, `filter` for exact matches, ranges, and sorting, and `none` to leave it out. A
file field with the `text` mode indexes its blob's contents, such as a PDF's extracted text.
[Decision ZIB0005](ZIB0005-note-identifiers-and-links.md) keeps backlinks in the same index.

[Decision ZIB0003](ZIB0003-space-identity-and-machine-settings.md) treats the index as derived data
in the machine's cache directory, which never syncs and can rebuild at any time. Only the running
instance of Zib writes it.

[Decision ZIB0007](ZIB0007-programming-languages-and-technologies.md) puts the index in the Rust
core, which builds for Linux, Android, and WebAssembly in a browser. The index's library has to
build for every one of those targets.

## Criteria

- **Every platform.** One library builds into the core for Linux and Android, so search behaves the
  same on both, and the choice doesn't rule out a browser later.
- **Full text.** Search ranks matches by relevance, matches word prefixes as the owner types, and
  handles text in any language that separates words with spaces.
- **Context.** Each result shows a snippet of the text around the matches, with the matching words
  highlighted, whether they're in a note's body or in a file's extracted text.
- **Fields.** The same index answers exact matches, ranges, and sorting on `filter` fields, and
  holds backlinks, so a query can combine text with conditions on fields.
- **Persistent and incremental.** The index survives a restart, and each change to a note updates
  only that note's entries, so opening a space with tens of thousands of notes doesn't rebuild
  anything.
- **Small toolbox.** The library is mature, needs no server, and adds little to each build.

## Decision

Keep each machine's index for each space in a SQLite database in the machine's cache directory, and
search text with SQLite's FTS5 extension. The core reaches SQLite through the `rusqlite` crate on
every platform. A later specification defines the tables.

- **Text.** One FTS5 table holds a row for each note, with one column each for its title and body.
  Another column holds its type name and fields with the `text` mode, including the extracted text
  of its files. Search ranks matches with FTS5's built-in BM25 ranking and matches prefixes of the
  words the owner types.
- **Snippets.** The text table keeps its own copy of the text, so FTS5's `snippet` function can
  return a few words around the matches in the column that matched, with the matching words marked
  for the app to highlight.
- **Fields.** Ordinary tables hold the values of fields with the `filter` mode, by kind, with
  indexes for exact matches, ranges, and sorting. A query joins them with the text table.
- **Backlinks.** A table holds each link from one note to another, which the core updates whenever
  it parses a body.
- **Keeping up.** The core updates a note's rows in the same pass that applies each change to the
  space, whether the change comes from the owner or from sync. If the database is missing or
  damaged, the core rebuilds it from the space's document.
- **Linux and Android.** `rusqlite` builds SQLite from source, with FTS5 enabled, into the core.
- **Browser, later.** Linux and Android come first. When a web app follows, the core uses
  `sqlite-wasm-rs`, a build of SQLite for `wasm32-unknown-unknown` with FTS5 enabled, which
  `rusqlite` supports. It stores the database in the Origin Private File System through a virtual
  file system that runs only in a dedicated worker, so the web app runs the core in a worker inside
  the tab that holds the spaces.

Against the criteria:

- **Every platform:** `rusqlite` with bundled SQLite builds for Linux and Android, so one set of
  queries runs on both, and `sqlite-wasm-rs` can bring the same SQLite and FTS5 to a browser later.
- **Full text:** FTS5 ranks with BM25, matches prefixes, and its default tokenizer splits text on
  Unicode word boundaries in any language that separates words with spaces.
- **Context:** FTS5's `snippet` function returns the words around each match from the stored text,
  bodies and files' text alike.
- **Fields:** SQLite's tables and indexes answer exact matches, ranges, and sorting, and a single
  query joins them with the text table.
- **Persistent and incremental:** the database lives on disk, and inserts, updates, and deletes
  touch single rows.
- **Small toolbox:** SQLite is among the most widely deployed libraries in the world, needs no
  server, and adds about a megabyte to each build.

## Rejected alternatives

- **Tantivy.** It's the leading full-text search library in Rust. It builds for Linux and Android,
  ranks better and answers faster than FTS5 on large indexes, and generates snippets too. But it has
  no supported build for a browser, its index writer holds a large memory buffer, and backlinks and
  joins with field conditions would need a second store beside it. It does worse on the every
  platform and small toolbox criteria, for gains a personal space of tens of thousands of notes
  doesn't need.
- **An in-memory index library, such as `elasticlunr-rs`.** It builds for every target and needs no
  storage. But it rebuilds from every note each time Zib starts, and holds the whole index in
  memory, which a phone with tens of thousands of notes and PDFs' text can't afford. It fails the
  persistent and incremental criterion.
- **A different search engine on each platform,** such as SQLite through Android's own interface and
  a JavaScript library in the browser. Each would use its platform's tools, but search would behave
  differently on each, and each would need its own code outside the core. It fails the every
  platform criterion and breaks the core's single implementation from decision ZIB0007.
- **Search over the Loro document directly, with no index.** It needs no extra storage, but every
  search would scan every note and every file's text. It fails the persistent and incremental
  criterion long before a space reaches tens of thousands of notes.

## Consequences

- The core depends on `rusqlite`, and on `sqlite-wasm-rs` for the browser. Both have to build with
  FTS5, which a check in the core's build verifies.
- The web app runs the core in a dedicated worker, and talks to it from the tab through messages.
- FTS5's default tokenizer doesn't split languages written without spaces, such as Chinese.
  Supporting them later takes another tokenizer, such as FTS5's trigram tokenizer, and a rebuild of
  the index.
- Search matches a whole note. Matches inside a part of a note, such as a page of a PDF, wait for
  the fragments that decision ZIB0005 reserves.
- Each machine's cache directory holds one database per space, which can grow to a large part of the
  size of its notes and files' text. Snippets need the text table to keep its own copy of every body
  and file's text, which roughly doubles that size.
