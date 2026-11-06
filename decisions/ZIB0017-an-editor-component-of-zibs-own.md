# ZIB0017 An editor component of Zib's own

## Context

The Linux app edits a note's body in a GTK text view, with the markdown shown as written and its
syntax hidden while the cursor is away. Getting there took layers of workarounds: tags that hide
syntax, overlay widgets for math, diagrams, and tables, and space reserved for each drawing with
oversized characters and paragraph spacing. Each workaround has edge cases, and some of them make
GTK crash.

Inline images show the limits. An embedded image should sit in a line of text like a word, but the
text view can only place an image as an object in its own buffer, which then holds a character the
note doesn't. Every other approach approximates the image's size with text. Aligning an image other
than on the baseline, or letting text flow around it, isn't possible in the text view at all.

The editor's behavior also lives only in the Linux app. How the cursor steps over hidden syntax,
what Enter and Tab do in lists and tables, and how a click picks a table's cell all work around the
text view's quirks, so the Android and web apps from
[decision ZIB0007](ZIB0007-programming-languages-and-technologies.md) would each have to work them
out again.

Zib is a project for one person, so an approach also has to be small enough to build, and to stop if
it doesn't work out.

## Criteria

- **Inline images.** An image can sit inside a line of text at its own size, on the baseline at
  least, with room to add other alignments and text that flows around it.
- **Live, precise editing.** Syntax hides and shows as the cursor moves, and the owner can edit
  nested structure, such as a table in a list item, precisely.
- **Shared behavior.** The rules of editing and moving live in the core once, with tests, for every
  platform to reuse.
- **Native and light.** Each app draws with its platform's toolkit, as decision ZIB0007 asks, and no
  app pulls in a large runtime such as a browser engine.
- **Proportionate.** One person can build it in steps, try each one, and stop early if it doesn't
  work out.

## Decision

Build an editor component of Zib's own. The core holds its state and rules, and each platform lays
out and draws its text with the platform's own text engine. The platform layer is thick, since it
lays out text, but it only lays out, draws, and measures. The core decides what the text means, what
shows, where the cursor goes, and what each key does.

### What the core holds

An `editor` module in the core holds the editor's state and rules, with no user interface code:

- **The text**: the note's body in the space's Loro document, which the editor reads and edits
  directly, with no copy of its own.
- **The selection**: an anchor and a head, and the column that moves up and down keep.
- **What shows**: which syntax hides, and which shows because the selection touches it.
- **A display model** of the text, described below, that holds what the platform lays out.
- **Commands**: typing, Backspace and Delete by character and by word, Enter and Shift+Enter, Tab
  and Shift+Tab, wrapping a selection in delimiters, toggling a task box, cut, copy, and paste.
- **Movement** that doesn't depend on how lines wrap: by character or word, to either end of the
  text, and past hidden syntax and images as single steps.
- **Undo and redo** of the owner's edits, described below.
- **Other machines' edits and cursors**: their edits arrive in the Loro text, and the selection
  keeps its place among them.

The core works in byte offsets of UTF-8 text. A platform whose text engine counts differently, such
as Android's UTF-16, converts at its boundary. The text stays the markdown stored in the note.

### Edits and undo

Each command becomes an edit of the note's Loro text as it happens, so the stored body is always
what the editor shows. Nothing waits for a pause in typing, and nothing compares texts to find what
changed.

The selection and other machines' cursors hold Loro cursors, which keep their place in the text
while edits from anywhere arrive.

Undo and redo use Loro's undo manager. It undoes only this machine's edits, and leaves other
machines' edits in place, even when they arrived later. It groups typing into steps with a merge
interval and a checkpoint at each command that should be a step of its own, such as Enter. Each step
remembers the selection before it, so undo puts the selection back too.

### Labels for links

A link without a label shows its target's title. The editor draws the title from the display model,
as a styled run that stands for the link's source, and the text never holds it. When the selection
touches the link, its source shows as written. The editor fills in no labels, and strips none when
it saves.

### The display model

The core describes the text as a list of blocks, each one either lines to lay out or a drawn block:

- **Lines.** Each line of the source becomes a paragraph to lay out. A paragraph is a list of
  **runs**, each with its source range and a style: plain text, a heading level, emphasis, strong,
  code, a link, dimmed syntax, and so on. Hidden syntax isn't in any run. An **atom** takes one
  place in a run: an inline image, inline math, a bullet, or a task box, each with its source range
  and what the platform needs to draw it.
- **Decorations** on each paragraph: its indentation in a list, its block quote depth, and a code
  block's shade.
- **Drawn blocks** take the place of the lines of a table, display math, or a diagram while the
  selection is outside them, with the source range they cover.

Each run knows which source bytes it covers, so the platform can turn a place in its layout into a
source offset and back, without knowing any markdown.

### What a platform does

- Lays out each paragraph with its own text engine, Pango on Linux, from the runs and atoms, and
  keeps each layout until the core says the paragraph changed.
- Draws text, atoms, decorations, drawn blocks, the selection, the cursor, and other machines'
  cursors.
- Answers the questions that need layout: the source offset at a point, the rectangle of an offset,
  and the offset one line up or down, or at the start or end of a line as it wraps.
- Turns keys, pointer events, and the text the input method commits into core commands.
- Connects the clipboard, scrolling, and the window's menus.

On Linux, the component is a GTK widget in Rust that draws with Pango and GTK's snapshot API.

### The core's interface

The Android app reaches the core through UniFFI bindings, as decision ZIB0007 says, so the editor's
interface uses only what those bindings carry well: plain values that the core hands over and owns
no part of afterward, such as the display model and offsets, and calls that take and return them.
The Linux widget uses the same interface, so it stays the one that every platform shares.

### Scope

Inline images sit on the baseline at first. The editor takes only the text that the input method
commits, so compose keys and dead keys don't work, and it supports neither bidirectional text nor
accessibility. Each of these can come later without changing the approach.

### Replacing the current editor

The new editor grows beside the current one, behind a setting of the development build, until it
does everything the current one does, and then replaces it. It starts with plain text, then styles,
then hidden syntax, so the riskiest parts come first, and each step works in the development build
before the next.

### Evaluation

- **Inline images:** an image is an atom in a run, which the platform lays out at its size. Pango's
  shape attribute reserves a box of any size at any offset from the baseline, so other alignments
  come later without a new approach.
- **Live, precise editing:** the core decides what shows from the selection, with the rules the
  current editor has, and the display model keeps each run's source range, so the platform maps
  every position back to the exact byte. Edits go straight into the Loro text, so other machines'
  edits merge as they arrive, and undo reverses only the owner's own.
- **Shared behavior:** the commands, the movement, what shows, and undo live in the core with unit
  tests. A platform only lays out, draws, and answers layout questions.
- **Native and light:** each platform draws with its own toolkit and text engine, and nothing adds a
  browser engine or a runtime beside the core.
- **Proportionate:** plain text editing comes first, then styles, then hidden syntax, and each step
  works in the development build. If the first three go badly, the old editor still works.

## Rejected alternatives

- **The text view with images as objects in its buffer.** It's the smallest change, and it gives
  inline images on the baseline. It keeps the editor's rules in the Linux app, written against the
  text view, so it fails the shared behavior criterion. It also allows no other alignment and no
  text flow, so it falls short of the inline images criterion.
- **A browser engine running CodeMirror.** WebKitGTK can run CodeMirror, which edits markdown as
  text and draws widgets over hidden syntax, and CSS gives images any alignment and text flow. The
  web app could reuse it. But it adds a browser engine to the Linux app and TypeScript beside the
  core, and its rules would live in JavaScript, not the core, so it fails the native and light
  criterion, and the shared behavior criterion for Android.
- **A rich-text editor such as ProseMirror or Tiptap.** These keep a document model of their own and
  treat markdown as an import and export format, so the stored text would change through
  conversions, and Zib's own syntax would need a schema and a serializer each. They fail the live,
  precise editing criterion, and the native and light criterion for the same reasons as CodeMirror.
- **Qt's text editor.** Qt's editor lets images float with text flowing around them, but it edits
  rich text, not markdown, its inline images don't align reliably, and it means leaving GTK. It
  fails the native and light criterion.
- **The editor of a Rust toolkit, such as iced, egui, or the one Zed uses.** None of them edits text
  with inline images, and none fits inside a GTK window. They fail the inline images and native and
  light criteria.
- **A copy of the text that the editor commits after a pause.** It's how the current editor works:
  it keeps its own text and saves the differences once typing stops. Other machines' edits that
  arrive before then need merging into the copy, and the undo history has to work around them. It
  does worse on the live, precise editing criterion than editing the Loro text directly.
- **An undo stack of the editor's own.** It would record the editor's edits and reverse them, but
  undoing one after another machine edited nearby means adjusting it for every later change, which
  Loro's undo manager already does. It does worse on the live, precise editing and proportionate
  criteria.
- **Labels in the text.** The current editor puts each link's title into its text as a label, and
  strips it when it saves, because a text view can only show text its buffer holds. A display model
  can show the title without it, so filling and stripping labels adds a mapping between two texts
  for nothing. It does worse on the proportionate criterion.
- **A core that lays out text too.** The core could lay out each note with a Rust text engine such
  as Parley, and each platform would only draw what it gets, which shares the most code. But no
  platform's toolkit draws text it didn't lay out with its own fonts, so each would have to draw
  glyphs itself or show a bitmap, and the text wouldn't render like the rest of the app. It also
  needs much more work before the first thing to try. It fails the native and light and
  proportionate criteria.

## Consequences

- The core gains an editor module, and the Linux app gains a widget that draws with Pango. The old
  editor, `drawings.rs`, and its workarounds for GTK go once the new editor replaces it.
- The navigation and editing rules in the Linux editor move into the core one at a time, each with a
  test. Until then, the two editors can differ.
- Every platform answers the same layout questions, and has to answer them the same way for the
  editor to behave alike.
- Dead keys and compose sequences don't work in the new editor until it shows text that an input
  method is still composing.
- The Android and web apps each need a widget that lays out the display model, but no editing rules
  of their own.
- Each command becomes a Loro change of its own, so a note's history holds one change for each
  keystroke, which Loro stores compactly. The commit timer, the comparison of texts, and the filling
  and stripping of labels in the window go away.
- Loro's undo manager works on the whole space's document, which holds every note. Every other
  change that this machine makes to the document, such as a derived thumbnail or a moved note,
  commits with an origin that the undo manager leaves out, so undo in the editor touches only the
  edits made in it.
- The [note bodies specification](../specs/note-bodies.md) gains a section for editors that draw a
  link's title without putting it in the text, beside its section on labels in raw editors.
