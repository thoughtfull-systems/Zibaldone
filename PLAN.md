# Plan: an editor of Zib's own

This plan carries out [decision ZIB0017](decisions/ZIB0017-an-editor-component-of-zibs-own.md),
which replaces the Linux app's GTK text view with an editor component whose rules live in the core.
The decision records what the editor is and why. This plan records the order of the work, and goes
away once the work ends.

Zib is a project for one person, so the plan favors the smallest step the owner can try by hand, and
a point after each step to stop if the approach isn't working.

## Where the work lives

- `core/src/editor/`: the state, display model, commands, movement, and undo, with unit tests.
- `linux/src/editor_widget.rs` and modules beside it: the GTK widget. The prototype in the stash on
  the `editor-prototype` branch, which lays out text and inline images with Pango, is its start.
- The current editor stays, and the new one sits beside it behind the `ZIB_EDITOR=new` environment
  variable in development builds, until the new one does everything the old one does.

## Steps

Each step ends with something to try in the development build, and core tests for what it adds. The
first three steps test the approach as a whole. If they go badly, the cost is a few weeks, and the
current editor still works.

### 0. The behaviors to keep

A list of everything the current editor does, gathered from `editor.rs`, `view.rs`, `window.rs`, and
`drawings.rs`: each key and what it does where, each click, how the cursor moves around hidden
syntax and drawings, and what shows when. It lives in `EDITOR-BEHAVIORS.md` beside this plan, and
each entry becomes a core test or a check by hand in a later step. The new editor replaces the old
one when every entry passes.

**Try it:** read the list, and add what's missing from memory.

### 1. Plain text

The core edits a note's Loro text directly and holds the selection as Loro cursors, with undo from
Loro's undo manager. The space's other local changes commit with an origin that the undo manager
leaves out. The widget lays out each line with Pango, with no styling, and supports:

- Typing, Backspace, Delete, and Enter.
- Moving by character, word, line, and page, Home and End, and the same with Shift to select.
- Clicking, double-clicking a word, triple-clicking a line, and dragging to select.
- Cut, copy, and paste of text, and the primary selection.
- Scrolling, with the cursor kept in view.
- Opening a note, with each edit stored as it happens.

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

A link without a label shows its target's title from the display model, and the text never holds it.
The note bodies specification gains a section for editors that work this way. The widget opens a
link on Ctrl+click and shows the link picker when the owner types `[[`.

**Try it:** link notes, rename a target, and check that labels follow.

### 6. Inline images

An embed becomes an image atom on the baseline, which the widget lays out with a Pango shape
attribute at the image's size, no wider than the text. Pasting an image works as it does now.

**Try it:** paste images between words, in list items, and alone on a line.

### 7. Block quotes and code blocks

Quote bars at each depth, the shade behind code blocks, and the keys that open and close them.

**Try it:** nest quotes and lists, compare Enter with Shift+Enter, and type `>` with whole lines
selected. Open a code block with a third backtick, add its language, and check blank lines and undo.

### 8. Math and diagrams

Inline math becomes an atom. Display math and diagrams become drawn blocks while the selection is
outside them, from the images that `render` draws now.

**Try it:** write inline math and a display formula, then add a `mermaid` fence. Click each drawing
to edit its source. Repeat inside lists and quotes. Check invalid input and undo.

### 9. Tables

Pipe tables and list tables become drawn blocks, with the current table widget's layout, and the
source shows when the selection enters. Tab and Shift+Tab move between cells, and a click in a cell
selects its source, as now.

**Try it:** create pipe and list tables, click cells to replace their source, and use Tab and
Shift+Tab. Nest a table inside a cell, resize the window, and check undo and linked titles.

### 10. Live editing

Edits from other machines arrive in the Loro text while the note is open, and the selection keeps
its place. Other machines' cursors and selections show in their colors.

**Try it:** open the same note on two machines. Type before and inside a selection, undo local
edits, and check that the other machine's edits stay. Move both cursors through styles, images, and
tables. Leave a cursor still, disconnect, and switch notes to check that stale cursors disappear.

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
