# Identifiers

This specification defines the identifiers of notes, spaces, and machines, their text form, and the
uniform resource identifiers (URIs) that name notes outside Zib. It follows
[decision ZIB0003](../decisions/ZIB0003-space-identity-and-machine-settings.md) for spaces and
machines and [decision ZIB0005](../decisions/ZIB0005-note-identifiers-and-links.md) for notes.

## Values

Every identifier that Zib generates is a version 7 Universally Unique Identifier (UUID), as
[Request for Comments (RFC) 9562][rfc 9562] defines it: a 48-bit timestamp in milliseconds since the
Unix epoch, the version and variant bits, and 74 more bits from a cryptographically secure random
generator. It's written and compared as 16 bytes in big-endian order. A machine that generates more
than one identifier in the same millisecond may use the counter that RFC 9562 allows, in place of
some of the random bits, so its identifiers keep their order.

- **Note identifiers.** A machine generates a note's identifier when someone creates the note. The
  note keeps it forever, through moves between spaces, and no other note ever takes it.
- **Space identifiers.** A machine generates a space's identifier when someone creates the space.
- **Machine identifiers.** A machine generates its own identifier the first time it runs.

Secrets aren't identifiers. The random values in pairing and invites stay fully random, as the
specifications for those protocols define them, because a timestamp would make them easier to guess.

A reader never relies on the inside of an identifier. It doesn't read the timestamp of an identifier
as the creation time of a note, space, or machine, which the note's `meta` records for notes.

## Text form

The text form of an identifier is 26 digits of [Crockford's Base32][crockford]. Zib always writes
one canonical form, and readers also accept the variations people make when they copy or type one.

- **Alphabet.** The 32 digits are `0123456789abcdefghjkmnpqrstvwxyz`, standing for the values 0 to
  31 in order. The alphabet leaves out `i`, `l`, `o`, and `u`.
- **Encoding.** A writer treats the 16 bytes as one 128-bit unsigned number, adds two zero bits
  beyond its highest bit to make 130 bits, and writes the 26 groups of 5 bits from the highest to
  the lowest, each as its digit. The first character is therefore always one of `0` to `7`.
- **Canonical form.** Zib writes the 26 digits in lowercase, with no hyphens, everywhere it writes
  an identifier: the space's document, wikilinks and embeds in note bodies, URIs, the `zib`
  command's output, and the apps.
- **Hyphens.** A person may add hyphens anywhere to make an identifier easier to read aloud or copy
  by hand, such as `01k97krzh1-fcx9rk9fhs-gt1de7`.
- **Reading.** A reader turns text into an identifier in this order:
  - First, it removes every hyphen and turns every letter to lowercase.
  - It replaces `i` and `l` with `1`, and `o` with `0`, since people confuse them.
  - Last, it rejects the text unless exactly 26 characters remain, each a digit of the alphabet,
    with the first one `7` or lower.
- **No check symbol.** Crockford's encoding offers an optional check symbol, but Zib neither writes
  nor accepts one. Four of its symbols, `*`, `~`, `$`, and `=`, are markdown's marks for emphasis,
  strikethrough, math, and highlights, and would complicate parsing identifiers in note bodies.
- **Order.** Sorting canonical forms as strings sorts the identifiers as numbers, so identifiers
  sort by the time a machine generated them.

```abnf
identifier    = first-digit 25digit
first-digit   = %x30-37                            ; 0 to 7
digit         = %x30-39 / %x61-68 / %x6A-6B        ; 0 to 9, a to h, j, k
              / %x6D-6E / %x70-74 / %x76-7A        ; m, n, p to t, v to z
```

The grammar describes the canonical form. Readers accept more, as the reading rules describe.

### Examples

| Bytes in hexadecimal               | Canonical form               |
| ---------------------------------- | ---------------------------- |
| `00000000000000000000000000000000` | `00000000000000000000000000` |
| `ffffffffffffffffffffffffffffffff` | `7zzzzzzzzzzzzzzzzzzzzzzzzz` |
| `017f22e279b07cc398c4dc0c0c07398f` | `01fwhe4ydgfk1shh6w1g60eecf` |
| `019a4f3c7e217b3a9c4d2f8e61a0b5c7` | `01k97krzh1fcx9rk9fhsgt1de7` |

The third row is the example version 7 UUID from RFC 9562. A reader also accepts
`01K97-KRZH1-FCX9R-K9FHS-GTIDE7` as the fourth identifier: it removes the hyphens, turns the letters
to lowercase, and reads the `I` as `1`.

## URIs

A URI names a note outside Zib, such as in a bookmark, a calendar entry, or another app.

```abnf
note-uri      = scheme ":note/" identifier [ "#" fragment ]
scheme        = "zib" / "web+zib"
fragment      = *( unreserved / pct-encoded / sub-delims / ":" / "@" / "/" / "?" )
```

`unreserved`, `pct-encoded`, and `sub-delims` are as [RFC 3986] defines them.

- **Identifier.** A reader accepts any form of the identifier that the reading rules accept.
- **Scheme.** Implementations write `zib`. A web app writes `web+zib`, because browsers let web
  pages handle only schemes that start with `web+`. Readers accept both.
- **No space.** A URI names only the note. The handler looks the identifier up in every space the
  machine holds, and shows a missing note if none holds it.
- **Fragments.** A URI may end in a fragment that names a part of the note, as wikilinks may. This
  specification reserves fragments and gives them no meaning yet. A reader that doesn't understand a
  fragment opens the whole note.
- **Web addresses.** A web app also serves each note at `https://<host>/note/<identifier>`, with the
  same identifier.

Examples:

- `zib:note/01k97krzh1fcx9rk9fhsgt1de7`
- `web+zib:note/01k97krzh1fcx9rk9fhsgt1de7`

## Platforms

- **Linux.** The app registers itself as the handler of the `zib` scheme through its desktop entry.
- **Android.** The app registers an intent filter for the `zib` scheme.
- **Browser.** A web app, when one exists, registers `web+zib` through the browser's protocol
  handler interface.

[crockford]: https://www.crockford.com/base32.html
[rfc 3986]: https://www.rfc-editor.org/rfc/rfc3986
[rfc 9562]: https://www.rfc-editor.org/rfc/rfc9562
