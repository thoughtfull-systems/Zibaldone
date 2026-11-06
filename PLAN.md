# Plan: an editor of Zib's own

Zib's Linux editor is a GTK text view with workarounds layered on top: tags that hide syntax,
overlay widgets for drawings, oversized characters that hold room for images, and fixes for the ways
GTK aborts when those tricks meet its own assumptions. Inline images can't work well that way, and
none of the work carries over to Android or the web.

This plan replaces the text view with an editor component of Zib's own, built so that the core holds
as much of it as it can. Each platform still lays out and draws text with its own engine, so the
platform layer is thick, but it only draws and measures. The core decides what the text means, what
shows, where the cursor goes, and what each key does.

Zib is a project for one person, so the plan favors the smallest step the owner can try by hand, and
a point after each step to stop if the approach isn't working.

## Goals

- **Selection, movement, copy, and paste** from the keyboard and the mouse.
- **Inline images**, aligned on the baseline at first. Other alignments and text that flows around
  an image can come later.
- **A live editor with hidden syntax** that shows as the cursor moves into it, with lists, task
  boxes, block quotes, tables, math, and diagrams, as the editor has now.
- **Precise editing of nested structure**, such as a table or a code block inside a list item, with
  the navigation and editing rules the current editor has.
- **One implementation of behavior**: the core holds the rules, with tests, so the Android and web
  apps reuse them.

## Not goals

- Compose keys and dead keys. The editor takes the text that GTK's input method commits, but shows
  nothing while a sequence is in progress.
- Bidirectional text. The editor lays out every line left to right.
- Accessibility.
- Rich text editing apart from markdown. The note's body stays the markdown it's stored as.

## Architecture

### What the core holds

A new `editor` module in `zib-core` holds the editor's state and rules, with no user interface code,
as [decision ZIB0007](decisions/ZIB0007-programming-languages-and-technologies.md) asks:

- **The text** the editor shows: the stored body with the labels it fills in for links, and the
  mapping between the two, which `body::fill_labels_mapped` and `body::strip_labels` already do.
- **The selection**: an anchor and a head, as byte offsets in that text, and the column that moves
  up and down keep.
- **What shows**: which syntax hides, from `editing::spans` and `editing::drawings`, and which shows
  because the selection touches it.
- **A display model** of the text, described below, that holds what the platform lays out.
- **Commands**: typing, Backspace and Delete by character and by word, Enter and Shift+Enter, Tab
  and Shift+Tab, wrapping a selection in delimiters, toggling a task box, cut, copy, and paste. Many
  of their rules exist already in `lists`, `pipe_tables`, and `editing`.
- **Movement** that doesn't depend on layout: by character or word, to either end of the text, and
  past hidden syntax and images as single steps.
- **Undo and redo**, grouped into steps the way typing groups them now.
- **Committing**: turning the text back into the stored body, and the edits that reach Loro, which
  the window does now.
- **Edits from other machines**: applying them to the text while keeping the selection in place, and
  placing other machines' cursors.

The core works in byte offsets of UTF-8 text. A platform whose text engine counts differently, such
as Android's UTF-16, converts at its boundary.

### The display model

The core describes the text as a list of **blocks**, each one either a run of lines to lay out or a
drawn block:

- **Lines.** Each line of the source becomes a paragraph to lay out, as it's a line in the text view
  now. A paragraph is a list of **runs**, each with its source range and a style: plain text, a
  heading level, emphasis, strong, code, a link, dimmed syntax, and so on. Hidden syntax isn't in
  any run. An **atom** takes one place in a run: an inline image, inline math, a bullet, or a task
  box, each with its source range and what the platform needs to draw it.
- **Decorations** on each paragraph: its indentation in a list, its block quote depth, and a code
  block's shade.
- **Drawn blocks** take the place of the lines of a table, display math, or a diagram while the
  selection is outside them, with the source range they cover.

Each run knows which source bytes it covers, so the platform can turn a place in its layout into a
source offset and back, without knowing any markdown.

### What a platform does

- Lays out each paragraph with its own text engine, Pango on Linux, from the runs and atoms, and
  caches the layout until the core says the paragraph changed.
- Draws text, atoms, decorations, drawn blocks, the selection, the cursor, and other machines'
  cursors.
- Answers the questions that need layout: the source offset at a point, the rectangle of an offset,
  and the offset one line up or down, or at the start or end of a visual line.
- Turns keys, pointer events, and the input method's committed text into core commands.
- Connects the clipboard, scrolling, and the window's menus.

On Linux, the component is a GTK widget in Rust that draws with Pango and GTK's snapshot API, as the
prototype in the `editor-prototype` stash does.

### Where it lives

- `core/src/editor/`: the state, display model, commands, movement, and undo, with unit tests.
- `linux/src/editor_widget.rs` and modules beside it: the GTK widget.
- The current editor stays, and the new one sits beside it behind the `ZIB_EDITOR=new` environment
  variable in development builds, until the new one does everything the old one does.

### A decision first

Replacing the text view sets a guideline for every platform and is hard to reverse, so it gets a
decision record before the first step, as [decision ZIB0000](decisions/ZIB0000-record-decisions.md)
asks. The record compares this plan with images in the buffer of the current text view, a web engine
running CodeMirror, and a core that also lays out text.

## Steps

Each step ends with something to try in the development build, and core tests for what it adds. The
first three steps test the approach as a whole. If they go badly, the cost is a few weeks, and the
current editor still works.

### 1. Plain text

The core holds a note's text and selection, and makes edits by character and word, with undo. The
widget lays out each line with Pango, with no styling, and supports:

- Typing, Backspace, Delete, and Enter.
- Moving by character, word, line, and page, Home and End, and the same with Shift to select.
- Clicking, double-clicking a word, triple-clicking a line, and dragging to select.
- Cut, copy, and paste of text, and the primary selection.
- Scrolling, with the cursor kept in view.
- Opening and saving a note through the window's existing commit path.

**Try it:** edit a note's raw markdown in the new widget, and check that the note saves and syncs.

### 2. Styles

The core turns `editing::spans` into styled runs, with every piece of syntax shown. The widget draws
headings, emphasis, strong, strikethrough, underlines, highlights, code, links, and dimmed syntax in
the current theme's colors.

**Try it:** a note looks like the current editor does with the cursor on every line.

### 3. Hidden syntax

The core leaves hidden syntax out of the runs, and shows it as the selection touches it, with the
rules that `Preview::shows` has now. Movement steps over hidden syntax as the current editor does.
The navigation rules in `editor.rs` move into the core with tests, one at a time, written from the
current behavior.

**Try it:** move through a note with links, emphasis, and code, and compare it with the current
editor. This is the main point to decide whether to go on.

### 4. Lists and task boxes

The widget indents list items, draws bullets and task boxes in place of their markers, and lines up
numbers. The core gives Enter, Shift+Enter, Tab, and Shift+Tab the meanings they have in an outline,
through `lists::list_edit`, and toggles a task box on a click.

**Try it:** write and rearrange a nested task list.

### 5. Links and their labels

The core fills in labels for links without one and strips them on commit, as the window does now.
The widget opens a link on Ctrl+click and shows the link picker when the owner types `[[`.

**Try it:** link notes, rename a target, and check that labels follow.

### 6. Inline images

An embed becomes an image atom on the baseline, which the widget lays out with a Pango shape
attribute at the image's size, no wider than the text. Pasting an image works as it does now.

**Try it:** paste images between words, in list items, and alone on a line.

### 7. Block quotes and code blocks

Quote bars at each depth, the shade behind code blocks, and the keys that open and close them.

### 8. Math and diagrams

Inline math becomes an atom. Display math and diagrams become drawn blocks while the selection is
outside them, from the images that `render` draws now.

### 9. Tables

Pipe tables and list tables become drawn blocks, with the current table widget's layout, and the
source shows when the selection enters. Tab and Shift+Tab move between cells, and a click in a cell
selects its source, as now.

### 10. Live editing

Edits from other machines arrive in the core and keep the selection in place, and other machines'
cursors and selections show in their colors.

### 11. The switch

The new editor becomes the default once it does everything the old one does. Then the GTK text view,
`drawings.rs`, the view's tag drawing, and the workarounds for GTK's aborts go.

### Later

- Image alignment at the top, middle, and bottom, and text that flows around images.
- Laying out only the paragraphs in view, if long notes get slow.
- The Android and web editors, on the same core.

## Testing

- **The core:** unit tests for each command and movement rule, written as text with a cursor mark,
  such as `"- item|"` going to `"- item\n- |"` on Enter. Most of the editor's behavior lives here,
  so most of its tests do too.
- **Layout:** the widget renders a few sample notes to image files through GTK's Broadway backend
  without a display, which makes changes in layout visible without opening the app.
- **By hand:** each step ends with trying it in the development build.

## Risks

- **Rebuilding the navigation rules.** The current editor's rules took a lot of tuning, and they're
  written against GTK's behavior. Porting them one at a time, with a test for each, keeps any that
  get lost visible.
- **Layout questions the core can't answer.** Up, Down, Home, and End depend on wrapping, so the
  platform answers them, and every platform has to answer them the same way. Each of those questions
  stays small, and the core decides what happens with the answer.
- **Performance.** Pango lays out a paragraph in microseconds, so laying out every paragraph again
  after an edit works for notes of ordinary length. A cache of layouts by paragraph comes first if
  it doesn't.
- **Input methods.** Without compose and dead keys, the widget takes committed text only. Adding
  them later means showing the text in progress, which the prototype already does.
