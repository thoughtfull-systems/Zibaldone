# Schemas

This specification defines how a space stores the schemas of note types in its `schemas` root
container, and the derivations that fill derived fields. It follows
[decision ZIB0002](../decisions/ZIB0002-note-types-and-schemas.md). The
[space document specification](space-document.md) defines the conventions it uses, and the
[built-in types specification](built-in-types.md) lists the types that releases define.

## Layout

`schemas` is a map. Each key is a type's name, and each value is a map from field name to the
field's definition. A type with no fields has an empty map.

Type names and field names are lowercase words of the letters `a` to `z` and digits, joined by
single hyphens, starting with a letter:

```abnf
name   = word *( "-" word )
word   = lower *( lower / DIGIT )
lower  = %x61-7A                 ; a to z
```

## Field definitions

A field's definition is a map with these keys:

- **`kind`:** string. One of `text`, `integer`, `decimal`, `boolean`, `date`, `time`, `timestamp`,
  `link`, `file`, `choice`, or `list`.
- **`item`:** string. For a `list`, the kind of each item, which is any kind but `list`. Other kinds
  have no `item`.
- **`search`:** string. `text`, `filter`, or `none`, as decision ZIB0002 defines them. For a `list`,
  the mode applies to each item.
- **`choices`:** map. For a `choice`, or a `list` of choices, each key is an allowed value, and each
  value is `true`. Other kinds have no `choices`.
- **`target`:** string. For a `link`, or a `list` of links, the name of the type its targets should
  have, such as `image` for a book's cover. Apps offer notes of that type first, but a link to a
  note of another type is still valid. Without `target`, a link may point to any note.
- **`accepts`:** map. For a `file`, or a `list` of files, each key is a media type the field
  accepts, such as `application/pdf`, or a family of them, such as `image/*`, and each value is
  `true`. Without `accepts`, the field takes any file.
- **`derivation`:** string. For a derived `file` field, the name of the derivation that fills it,
  from [Derivations](#derivations).
- **`derived-from`:** string. For a derived field, the name of the `file` field of the same type
  that it derives from.
- **`preview`:** boolean. `true` on the one field that previews notes of the type: a derived `file`
  field that holds an image, or a `link`. A type without such a field has no preview.
- **`deprecated`:** boolean. `true` once a release deprecates the field. Releases stop offering it
  for new values, and keep reading, showing, and indexing the values that exist.

A key that doesn't apply to a field's kind is absent.

## Writing schemas

- **When.** Each time a release opens a space, it compares the space's schemas with its own, and
  writes what the space lacks in one commit.
- **What it adds.** It adds each type the space lacks, and each field a type in the space lacks,
  with its whole definition. It adds each choice value and each accepted media type that a field's
  map lacks. It sets `deprecated` on each field it knows as deprecated.
- **What it never changes.** It never removes a type, a field, or a key of a definition, and never
  changes a value that the space already holds. Two releases that add the same field at once write
  identical definitions, so the merge makes no difference.
- **Additive releases.** A release never changes a field's `kind`, `item`, `target`, `derivation`,
  or `derived-from`, never removes a choice or an accepted media type, never clears `deprecated`,
  and never moves `preview` to another field. A field that needs any of those changes gets a new
  name, and the old one gets deprecated.

## Reading schemas

- A release reads every note against the space's schema for the note's type, which holds every field
  any release defined.
- A note may lack any field. A key in a note's `fields` that the schema doesn't define is a field
  from a release that hasn't written its schema here yet, and the release leaves it untouched.
- A value that doesn't match its field's definition, such as text in a `date` field or a value
  missing from `choices`, shows and indexes as nothing, and the release leaves it alone until
  someone sets a valid value.
- A release that doesn't know a type shows its notes as generic notes, with each field shown and
  edited by its kind, and doesn't create notes of that type.

## Derivations

A derivation turns the blob in one `file` field into a new blob for a derived field. The machine
that runs it stores the result as a blob, adds its entry to `files`, and sets the derived field, all
in one commit, as decision ZIB0002 describes.

- **`text`.** It takes any file whose text the machine can read, and makes a `text/plain` blob in
  UTF-8 that holds the file's full text, in reading order, with a blank line between pages or
  sections. A PDF's text comes from its text layer. Recognizing text in scanned images is a later
  addition.
- **`thumbnail`.** It takes an image, or a document the machine can draw, and makes an image of at
  most 512 pixels on its longer side: the image scaled down, or the document's first page. It uses
  `image/png` when the source has transparency, and `image/jpeg` otherwise.
- **Failure.** A machine that can't run a derivation on a file leaves the derived field empty for
  another machine. When a derivation fails, because the file has no text or a machine can't read it,
  the machine records nothing in the space. It remembers the failure in its cache directory, and
  tries again only after the source changes.

## Platforms

- **Derivations.** Each platform runs the derivations it has libraries for. The Linux app reads the
  text of PDFs and draws their first page. The Android app may leave PDFs to Linux, and draws
  thumbnails of images.
