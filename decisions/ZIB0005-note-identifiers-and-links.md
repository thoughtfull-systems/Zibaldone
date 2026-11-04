# ZIB0005 Note identifiers and links

## Context

[Decision ZIB0001](ZIB0001-storage-format-for-spaces.md) keys each space's `notes` map by a note
identifier and leaves its form to this decision. Notes link to each other inside their markdown
bodies, and other software links to them too: a browser bookmark, a calendar entry, or a launcher on
the desktop. A link has to keep working while the note it points to changes.

Machines create notes offline, so identifiers can't come from a shared counter. A note's title comes
from its text, so it changes whenever the text does. A note can also move from one space to another,
and a link from a note in one space can point to a note in another, on a machine that holds both.

[Decision ZIB0004](ZIB0004-storage-and-sync-of-files.md) keeps every file in a field of a note, such
as a book's PDF or an image's picture. A note's body shows an image or links to a PDF through the
note that holds it.

## Criteria

- **Stable.** A link keeps pointing to the same note through edits, renames, and moves between
  spaces.
- **Offline creation.** Any machine can create a note and its identifier without coordination.
- **Current titles.** A link shows its target's current title, unless the owner gave it a label of
  their own.
- **Merge-safe.** Renaming or deleting a note edits no other note's text, so two machines can't make
  conflicting copies of the same change.
- **Quick to write.** Linking while writing takes a few keystrokes.
- **Readable text.** Links clutter a note's raw markdown as little as they can.
- **Reachable from outside.** The operating system, the browser, and other apps can open a note from
  a link.

## Decision

Give every note a version 7 Universally Unique Identifier (UUID) when someone creates it, written as
26 lowercase characters of Crockford's Base32. The identifier never changes, never gets reused, and
stays the same when the note moves to another space. A space's identifier from
[decision ZIB0003](ZIB0003-space-identity-and-machine-settings.md) uses the same text form. A later
specification defines the exact syntax.

- **Wikilinks.** Inside a note's body, link to a note with `[[id]]` or `[[id|label]]`. Typing `[[`
  in an editor opens a search, and picking a note inserts a link to its identifier. If the owner
  selected text first, the selection becomes the label.
- **Labels only by choice.** A link stores a label only when the owner writes one. Every
  implementation shows a link without a label by its target's current title. A label can hold span
  formatting too, which shows when the link does.
- **Raw editors.** An editor that shows a note's raw markdown fills in and removes titles itself:
  - When the owner starts editing a note, the editor gives each link without a label its target's
    title, as markdown with its span formatting, and remembers each title it showed for each target.
  - While the note is open, it updates the filled-in labels of links whose target's title changes,
    and remembers those titles too.
  - When it saves, it removes each label that matches its target's current title or any title it
    showed for that target, and keeps every other label as one the owner chose. A label the owner
    edits no longer matches, so it stays from then on.
  - It saves by applying only the differences between the result and the stored text, so links the
    owner didn't touch cause no edits.
- **Titles.** Every note has a title: the first line of its body that isn't blank, without its
  leading white space or the marks that make it a heading. The title keeps the line's span
  formatting, such as bold, italic, underlines, highlights, strikethrough, inline code, and inline
  math, and shows it wherever the app lists the note or shows a link to it. Each link in the line
  becomes its text, its label or the title of its target when it has none, so a title never holds a
  link of its own. A cycle of such links ends at the first note it would repeat, which shows by its
  identifier. A note whose body is blank shows as untitled. Places that can show only plain text,
  such as a window's title or a filename, strip the formatting too.
- **Embeds.** `![[id]]` shows the target in place, as its type defines: an `image` note shows its
  picture, and other types show their title and the preview their schema names, such as a book's
  cover. Body links to files always go through the note that holds them, never a hash, so a link
  keeps working when the file's content changes.
- **Fragments.** The specification reserves `[[id#fragment]]` for links to a part of a note, such as
  a heading or a page of a PDF, and leaves the fragment's meaning for a later version.
- **Missing and deleted targets.** A link whose target the machine doesn't hold, because it lives in
  a space the machine lacks, shows as a link to a missing note. When the owner deletes a note for
  good, its space keeps a tombstone with its identifier and last title, so links to it still say
  what they pointed to.
- **Moving notes.** Moving a note to another space happens on one machine that holds both. It
  fetches any blobs that the note's file fields hold and it lacks, copies the note into the target
  with the same identifier as new changes, and deletes it from the source without a tombstone or a
  trip through the trash, since the note still exists. A machine that holds only the source shows
  links to it as links to a missing note. The app warns before a move that it discards any edits to
  the note that other machines haven't synced yet. The note's history stays behind, and the notes it
  links to stay in their own spaces, since links reach across spaces.
- **URIs.** Outside the app, a uniform resource identifier (URI) of the form `zib:note/<id>` names a
  note. Each implementation registers itself as the handler of the `zib` scheme. A web app also
  serves each note at `https://<host>/note/<id>` and registers `web+zib`, because browsers only let
  web pages handle schemes that start with `web+`. Standard markdown links with a `zib:` URI work
  inside bodies too.
- **Backlinks.** Each machine finds backlinks by parsing every body, and keeps them in the search
  index that decision ZIB0003 places on the machine and [decision ZIB0008](ZIB0008-search-index.md)
  defines.

Against the criteria:

- **Stable:** the identifier is the only part of a link that resolves, and nothing about a note's
  content or space changes it.
- **Offline creation:** a version 7 UUID combines a timestamp with 74 random bits, so two machines
  won't generate the same one.
- **Current titles:** links store no copied titles, so every implementation looks up the title each
  time it shows a link.
- **Merge-safe:** a rename changes only the renamed note, and a deletion adds a tombstone, which a
  map merges by key without touching any text.
- **Quick to write:** the editor's search turns `[[` and a few letters into a finished link.
- **Readable text:** an identifier takes 26 characters instead of the usual 36, and every
  implementation shows the title in its place.
- **Reachable from outside:** a custom scheme works on Linux through the desktop entry, on Android
  through an intent filter, and in a browser through the `web+zib` scheme.

## Rejected alternatives

- **Links by title.** They're the quickest to type and the most readable, but they break when a
  title changes, and two notes can share a title. They fail the stable criterion.
- **Titles copied into labels.** The raw text would read well, but a copied title goes stale when
  the target's title changes, and rewriting it on two machines at once doubles it. It fails the
  current titles and merge-safe criteria.
- **Version 4 UUIDs, which are all random.** They work as well, but version 7's leading timestamp
  sorts identifiers by creation and keeps indexes that use them as keys in order. The creation time
  is already in each note's `meta`, so version 7 reveals nothing new.
- **Standard UUID text.** The usual 36-character form with hyphens works everywhere, but adds 10
  characters of noise to every link. It fails the readable text criterion.
- **Short random identifiers.** Eight random characters read well, but each machine would need to
  check for collisions, which it can't do offline. They fail the offline creation criterion.
- **Standard markdown links as the only syntax.** One syntax keeps the text standard, but
  `[label](zib:note/<id>)` is longer to type and to read than `[[<id>|label]]`, and an empty label
  isn't valid markdown. It fails the quick to write and readable text criteria as the main syntax,
  so it stays as a second form.
- **Body links to a file's hash.** A link could name a blob directly, with no note between. But a
  new version of the file has a new hash, so every link to it would break or need rewriting. It
  fails the stable criterion.

## Consequences

- The core needs a version 7 UUID generator, a Crockford Base32 codec, and a wikilink parser, which
  every implementation shares through it.
- Decision ZIB0004 refers to this decision for the syntax of links to files, which goes through file
  notes.
- The specification for spaces needs a tombstone map for notes deleted for good.
- A move writes two documents. If the app stops between them, the note exists in both spaces, and
  the owner deletes one copy.
- Every implementation derives titles by the same rule, so the rule lives in shared code.
- A filled-in title has to come back from the text exactly, or the save can't recognize it, so the
  wikilink syntax lets a backslash escape a backslash, a `]`, or a `|` inside a label, and writers
  escape all three.
- Raw text shows identifiers where titles would be, so an export to markdown files rewrites links to
  file names and fills in titles.
- Search over raw text doesn't find a note by the titles of the notes it links to.
- Implementations register the `zib` scheme with their platform, and a web app registers `web+zib`.
