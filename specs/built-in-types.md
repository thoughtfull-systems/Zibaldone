# Built-in types

This specification lists the note types that releases define and write into every space, with their
fields. The [schemas specification](schemas.md) defines how a space stores each schema, and
[decision ZIB0002](../decisions/ZIB0002-note-types-and-schemas.md) explains why releases define the
types. Types and fields join this list over time, and never leave it.

## `note`

An ordinary note: every note the owner writes from scratch is a `note`. It has no fields: its body
is all it holds, and its title comes from the body's first line that isn't blank, as for every type.
It has no preview.

## `file`

A file of any kind that has no more specific type, such as a PDF, a spreadsheet, or an archive.

Every field is of the file kind, so its value is the hash of a blob, as the
[space document specification](space-document.md#field-values) says. The note holds the file
directly, with no other note between.

- **`file`** holds the file itself. Its search mode is `none`.
- **`text`** holds the file's extracted text, so search finds the note by the file's contents. Its
  search mode is `text`, its `derivation` is `text`, and it's `derived-from` the `file` field.
- **`thumbnail`** holds a small image of the file's first page, for files a machine can draw. Its
  search mode is `none`, its `derivation` is `thumbnail`, it's `derived-from` the `file` field, and
  it's the type's `preview`.

## `image`

A picture, such as a photo, a scan, or a diagram from another tool.

Every field is of the file kind, so its value is the hash of a blob, as the
[space document specification](space-document.md#field-values) says. The note holds the file
directly, with no other note between.

- **`image`** holds the picture itself. Its search mode is `none`, and it `accepts` `image/*`.
- **`thumbnail`** holds a smaller copy, for lists and previews on machines that don't hold the
  picture. Its search mode is `none`, its `derivation` is `thumbnail`, it's `derived-from` the
  `image` field, and it's the type's `preview`.

An `image` note's body can describe the picture, which search indexes like any body.

## Adding files

When the owner adds a file to a space, by dropping, pasting, sharing, or picking it, the app creates
one note for it, in one commit with the blob's entry in `files`:

- A file whose media type starts with `image/` becomes an `image` note, with the blob in `image`.
- Any other file becomes a `file` note, with the blob in `file`.
- The note's body starts with the file's name, which becomes its title, followed by a blank line.

The derived fields stay empty until a machine that can run their derivations fills them.

## Embeds

`![[id]]` shows each type as [decision ZIB0005](../decisions/ZIB0005-note-identifiers-and-links.md)
describes:

- An `image` note shows its picture at the width of the text, from `image` on a machine that holds
  the blob, and from `thumbnail` until it arrives.
- A `file` note shows its title and its thumbnail, if it has one, and opens the file when the owner
  activates it.
- A `note` shows its title.

## Platforms

No platform differs in the types it defines. Which derivations each platform runs is in the
[schemas specification](schemas.md#platforms).
