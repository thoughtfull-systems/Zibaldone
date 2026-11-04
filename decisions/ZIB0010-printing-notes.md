# ZIB0010 Printing notes on paper and to files

## Context

The owner prints some notes, such as a checklist, a recipe, or a set of notes to read away from a
screen, and sometimes wants them as a PDF file instead. A printed note should look as good as a
typeset document: headings, tables that can break across pages, code, and the math and diagrams that
[decision ZIB0009](ZIB0009-markdown-dialect-and-parser.md) adds.

[Decision ZIB0007](ZIB0007-programming-languages-and-technologies.md) puts features in the Rust core
so every app offers them, and records platform differences in specifications. The owner prints only
from Linux, so the Android app has no need to print, and shouldn't carry the libraries and fonts
that typesetting takes.

## Criteria

- **Typeset quality.** Pages have good typography, tables and lists break across pages cleanly, and
  math and diagrams look as sharp as on screen.
- **Same content as on screen.** A printed note comes from the same parse tree and the same rendered
  math and diagrams as the note in the app, so nothing parses or renders twice in different ways.
- **PDF and paper.** The owner can print one note or a selection of notes to a printer, or save them
  as a PDF.
- **Light Android app.** Printing adds nothing to the Android build.
- **Small toolbox.** Printing runs inside the Linux app, offline, with no external program to
  install.

## Decision

Print with [Typst], a typesetting system written in Rust, built into the core behind a build feature
that only the Linux app turns on. A later specification defines the page styles.

- **From tree to pages.** The core turns a note's parse tree into Typst markup, escaping every piece
  of text, and compiles it to a PDF in memory with Typst's `typst` and `typst-pdf` crates. Math and
  diagrams go in as the SVG that the core already renders for the screen, and images go in from
  their blobs.
- **Zib's elements.** A wikilink prints as its title. An embed prints as the app shows it, such as
  an image or a note's title and preview. A list table prints as a table, with its header rows and
  columns repeated where Typst repeats headers across pages.
- **Selections.** The owner can print a selection of notes, or a note with the notes it links to, as
  one document, with each note starting on a new page.
- **Printing and saving.** The Linux app hands the PDF to GTK's print dialog, which sends it to a
  printer, or saves it as a file. The `zib` command asks the running instance for the PDF and writes
  it to a file.
- **Fonts.** The core bundles the fonts that its page styles use, so a note prints the same on every
  Linux machine. It falls back to the system's fonts, found through Fontconfig, for scripts the
  bundled fonts lack.
- **Settings.** Paper size and margins are machine settings, in the machine's store from
  [decision ZIB0003](ZIB0003-space-identity-and-machine-settings.md), since they depend on the
  printer at hand.
- **Android.** The Android app builds the core without the feature, offers no printing, and its
  specification records the difference.

Against the criteria:

- **Typeset quality:** Typst lays out text with professional line breaking and hyphenation, breaks
  tables across pages with repeated headers, and places SVG at any resolution.
- **Same content as on screen:** Typst receives the core's own tree and SVG, so a note prints as it
  parses and renders in the app.
- **PDF and paper:** Typst writes PDF, and GTK's print dialog covers printers and files alike.
- **Light Android app:** the build feature keeps Typst and its fonts out of the Android build.
- **Small toolbox:** Typst is a set of Rust crates that run in process, with no LaTeX installation,
  browser, or external converter.

## Rejected alternatives

- **Drawing pages with GTK's own printing with Cairo and Pango.** GTK already depends on them, so
  printing would add nothing to the build. But Zib would write its own page layout: where tables
  break, how headers repeat, and how lists and code wrap. That's a typesetting system of its own. It
  does worse on typeset quality and small toolbox.
- **Rendering HTML and printing it with a browser engine, such as WebKitGTK.** Browsers print well
  enough, and HTML is easy to generate. But it adds a browser engine to the Linux app, and browsers
  break tables across pages poorly. It fails small toolbox and does worse on typeset quality.
- **Exporting to LaTeX or through Pandoc.** Both typeset beautifully. But they need large external
  installations, and Pandoc would parse the markdown again with its own rules. They fail small
  toolbox and same content as on screen.
- **Printing on Android too.** Android's print framework would take the same PDF. But Typst and its
  fonts would add tens of megabytes to the Android app for a feature the owner doesn't use there. It
  fails light Android app.

## Consequences

- The Linux build depends on Typst's crates and carries its fonts, which adds tens of megabytes.
- The core keeps two paths from the parse tree: the screen models every app renders, and the Typst
  markup for print. The print path needs tests of its own.
- A new element in the dialect needs a printed form as well as an on-screen one.
- Typst releases can change layout, so printing pins Typst's version and updates it deliberately.
- An export to PDF, when the export decision comes, can reuse this path.
- Android can't print. A printout of a note written on the phone waits until the note syncs to a
  Linux machine.

[typst]: https://typst.app/
