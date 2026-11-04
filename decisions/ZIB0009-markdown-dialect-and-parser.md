# ZIB0009 Markdown dialect and parser

## Context

Every note's body is markdown, in a Loro text container, as
[decision ZIB0001](ZIB0001-storage-format-for-spaces.md) defines.
[Decision ZIB0005](ZIB0005-note-identifiers-and-links.md) adds wikilinks and embeds to it, and takes
each note's title from the first line of its body.
[Decision ZIB0007](ZIB0007-programming-languages-and-technologies.md) has the Rust core parse bodies
into a tree that every app renders the same way, and apply the owner's edits to the text.

Markdown has many dialects, and they disagree on details such as tables, task lists, and how
emphasis nests. A body has to mean the same thing on every machine, and an export has to read well
in other tools.

Plain markdown also falls short in three ways the owner cares about. Its tables hold one line of
text per cell, so a cell can't hold paragraphs or a list. It has no diagrams, and no math.

## Criteria

- **One meaning.** A precise specification defines how every body parses, so every implementation
  and every release reads it the same way.
- **Familiar.** The syntax is what the owner already writes elsewhere, and an exported note reads
  well in other markdown tools.
- **Rich enough for notes.** The dialect covers headings, lists, task lists, links, emphasis, code,
  quotes, and tables whose cells hold more than one paragraph or a list, plus diagrams, math, and
  Zib's wikilinks and embeds.
- **Editable.** The parser reports where in the text each element starts and ends, so the core can
  map the owner's edits back to the Loro text and draw decorations over it.
- **Native rendering.** Diagrams and math render inside the core, on Linux and Android alike,
  without a web view or a JavaScript engine.
- **Small toolbox.** The parser and renderers are Rust crates, fast enough to update a long note as
  the owner types on a phone, and small enough not to swell the Android app.

## Decision

Write note bodies in [CommonMark] with the [GitHub Flavored Markdown][gfm] (GFM) extensions for
tables, task list items, strikethrough, and autolinks, plus Zib's own additions. Parse them in the
core with the [pulldown-cmark] crate, and render diagrams and math in the core as Scalable Vector
Graphics (SVG). A later specification defines the additions' exact syntax.

- **Wikilinks and embeds.** `[[id]]`, `[[id|label]]`, and `![[id]]`, as decision ZIB0005 defines
  them, with backslashes that escape a backslash, a `]`, or a `|` inside a label.

- **Highlights.** A pair of `==` marks highlights text, as in `==this==`. The marks pair within one
  block the way `~~` does for strikethrough, and only a run of exactly two `=` counts.

- **List tables.** A fenced code block whose language is `table` holds a table as a list of rows.
  Each top-level item is a row, and each item nested under a row is a cell, which can hold any
  markdown: paragraphs, a list, code, or math. Options after the language set which rows and columns
  are headers: `header-rows` counts header rows from the top, and `header-columns` counts header
  columns from the left. By default the first row is a header and no column is, so a plain `table`
  block has one header row. The specification can add options later, such as column widths or
  alignment, as additive changes. GFM's pipe tables stay the usual choice. The apps' table editor
  writes a pipe table while every cell fits on one line and only the first row is a header, and
  switches to a list table when a cell needs more, so the owner rarely types a list table by hand.
  This table has a header row and a header column:

  ````markdown
  ```table header-columns=1
  - - Step
    - Notes
  - - Install
    - Download the package.

      Then check its signature:

      - on Linux, with `gpg`
      - on Android, in the store
  ```
  ````

- **Diagrams.** A fenced code block whose language is `mermaid` holds a [Mermaid] diagram, as on
  GitHub and in Obsidian. The core renders it with [merman], a Rust port of Mermaid that needs no
  JavaScript.

- **Math.** `$…$` holds inline math and `$$…$$` holds display math, in the LaTeX syntax that KaTeX
  and MathJax accept. pulldown-cmark parses both, and the core renders them with [RaTeX], a Rust
  port of KaTeX, which embeds the outlines of its glyphs in the SVG.

- **Raw HTML.** Bodies may hold HTML, which parses as CommonMark says, but apps show it as plain
  text and never render it.

- **Parsing.** The core parses a body with pulldown-cmark's offset iterator, which gives each
  element its range in the text. It finds wikilinks, embeds, and highlights in the text events
  itself, outside code. pulldown-cmark doesn't detect bare web addresses, so the core also finds
  them, following GFM's rules for extended autolinks. It parses the inside of a list table as
  markdown of its own, and builds the tree that every app renders from those ranges.

- **Rendering.** The core turns each diagram and each piece of math into SVG, and keeps the results
  in the cache directory, keyed by a hash of their source, so an unchanged diagram doesn't render
  again. The apps draw the SVG. A diagram or formula that fails to render shows its source and the
  error.

- **Titles.** A note's title is the plain text of its body's first line: heading marks, emphasis,
  and other formatting removed, and each link replaced by its text, as decision ZIB0005 describes.

- **Writing.** The core never rewrites a body to normalize its markdown. An edit changes only the
  text the owner changed.

Against the criteria:

- **One meaning:** CommonMark and GFM each have a precise specification with conformance tests, and
  pulldown-cmark passes CommonMark's. Mermaid and KaTeX define their own syntax. The additions are
  small and have a specification of their own.
- **Familiar:** CommonMark with GFM's extensions is what GitHub, most note apps, and most editors
  write, and `mermaid` blocks and `$` math are what GitHub and Obsidian render. Another tool shows a
  list table as a code block of nested lists, which still reads well.
- **Rich enough for notes:** list tables give cells any content, and Mermaid and KaTeX cover
  diagrams and math.
- **Editable:** pulldown-cmark reports a byte range for every element, so the core maps each node of
  the tree to its text, inside list tables too.
- **Native rendering:** merman and RaTeX produce SVG in Rust, with no web view.
- **Small toolbox:** pulldown-cmark is the parser behind `rustdoc` and `mdBook`, has few
  dependencies, and parses without building its own tree. RaTeX's fonts and merman add a few
  megabytes, far less than a web view or a typesetting system.

## Rejected alternatives

- **Another markup, such as Typst, AsciiDoc, Djot, or Org.** Typst and AsciiDoc have tables whose
  cells hold anything, and Typst has math and print layout built in. But Typst is a programming
  language laid out on pages, and AsciiDoc's Rust parser is young. Djot and Org still hold one line
  per table cell. Other tools read none of them as widely as markdown. They fail familiar, and most
  fail small toolbox.
- **Pandoc's grid tables or multiline tables.** They let cells span lines in plain markdown, but
  their columns have to line up character by character, which no one can type on a phone, and few
  tools besides Pandoc read them. They fail rich enough for notes in practice, and familiar.
- **HTML tables in the body.** Markdown allows them, and other tools render them. But typing HTML is
  tedious, and Zib never renders HTML, so it would need a second, safe HTML renderer. It fails
  native rendering and small toolbox.
- **Mermaid's own renderer in a web view.** It supports every diagram type the day Mermaid adds it.
  But it puts a browser engine beside each app's native widgets. It fails native rendering.
- **Typst as the renderer for math on every platform.** It typesets math beautifully, and could also
  print. But its library and fonts add tens of megabytes to the Android app, and it uses its own
  math syntax, so LaTeX math would need converting first. It fails small toolbox.
- **`comrak`.** It implements CommonMark and GFM fully, bare web addresses included, and builds a
  tree with source positions. But it reports positions as lines and columns rather than byte ranges,
  it allocates a tree the core would convert again, and it's slower. It does worse on editable and
  small toolbox, and its one gain, autolinks, is a small pass over the text events.
- **`markdown-rs`.** It follows the CommonMark specification closely and builds a syntax tree with
  positions. But it's younger and less used than pulldown-cmark, and its tree costs more to build on
  every keystroke. It does worse on small toolbox.
- **A structured document instead of markdown, such as a tree of blocks.** Rich editing and tables
  would map cleanly onto it. But it gives up plain text, which the owner can read and type anywhere,
  and decision ZIB0001 already stores bodies as text. It fails familiar.

## Consequences

- The core depends on pulldown-cmark, merman, and RaTeX, and a specification for bodies defines the
  additions, list tables, and the rule for titles.
- Every implementation renders the same tree and the same SVG, so a body looks alike on every
  platform, apart from each platform's fonts and widgets. Both apps need to draw SVG.
- merman and RaTeX are young. Neither covers everything Mermaid and KaTeX do, so some diagrams and
  formulas show their source and an error until the ports catch up. Swapping either for another
  renderer changes no stored note.
- HTML in a body never runs or renders, so pasted HTML shows as text.
- A new syntax, such as footnotes, is an additive change to the dialect. A body that uses it shows
  as plain text on releases that don't know it.
- A later decision on exports settles how list tables, wikilinks, embeds, and highlights appear
  outside Zib.
- Printing gets its own decision.

[commonmark]: https://spec.commonmark.org/
[gfm]: https://github.github.com/gfm/
[mermaid]: https://mermaid.js.org/
[merman]: https://github.com/Latias94/merman
[pulldown-cmark]: https://github.com/pulldown-cmark/pulldown-cmark
[ratex]: https://github.com/erweixin/RaTeX
