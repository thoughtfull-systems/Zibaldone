# Editor behaviors to keep

Everything the current editor does, gathered from `linux/src/editor.rs`, `view.rs`, `drawings.rs`,
`tables.rs`, `remote.rs`, and `window.rs`, and from the rules in the core that they call. Step 0 of
`PLAN.md` explains its use: each entry becomes a core test or a check by hand, and the new editor
replaces the old one when every box here has a check. Entries marked "core" already have their rule
and tests in the core, which the new editor calls the same way.

Like `PLAN.md`, this list goes away once the work ends.

## Layout and look

- [ ] Lines wrap at word boundaries, and inside a word too long for a line.
- [ ] Margins of 24 pixels at the left, right, and bottom, and 16 at the top.
- [ ] Colors follow the system's light or dark style, and change with it while the editor is open.
- [ ] Headings show in bold at 1.6, 1.4, 1.2, and 1 times the text size for levels 1 to 4 and
  deeper, with 6 pixels of space before them. Each of the first four levels has a color: purple,
  blue, green, then orange, in a darker shade on a light background, and a lighter one on a dark
  background.
- [ ] Emphasis shows in italics, strong in bold, and strikethrough struck through.
- [ ] `=text=` underlines, and `==text==` highlights in yellow.
- [ ] Code spans show in a monospace font on a light shade.
- [ ] Links, wikilinks, and embeds that stay links show in blue and underlined.
- [ ] Syntax that shows, such as `**`, `[[`, or a heading's `#` markers, shows dimmed. This changes
  the current editor, which leaves a heading's `#` markers in the text color.
- [ ] Code blocks show in a monospace font on a shade that spans the editor's width, empty lines
  included, and starts at the line's start for a block nested in a list item.
- [ ] Block quotes show in italics and dimmed, moved right by one step for each level of nesting,
  with a bar beside their lines for each level. Bars of lines in a row join up. A quote in a list
  item starts past the item's marker or indentation, with its bars there, and moves only its own
  lines.
- [ ] A pipe table's source shows like a code block, in a monospace font on a shade across the
  editor's width, so its columns line up. This changes the current editor, which shows it in a
  monospace font with no shade.
- [ ] Math source shows like code: display math like a code block, in a monospace font on a shade
  across the editor's width, and inline math like a code span, in a monospace font on a light shade.
  This changes the current editor, which shows math source in an italic serif font.

## Lists and task boxes

- [ ] A list item's indentation, marker, and box show in a monospace font, so continuation lines and
  nested items line up under the text on the line before.
- [ ] While the cursor is away, `*` shows as a bullet, `-` as a hyphen bullet, and `+` as a bold
  plus, each as wide as the inside of a task box on its line.
- [ ] Numbered items with one-digit numbers move right by up to four columns, so a list's numbers
  line up on the right until they reach three digits. Nested levels add up, and lists in block
  quotes keep their columns. (core: `editing::list_pads`)
- [ ] While the cursor is away, a task box draws as a rounded square a little smaller than its line:
  an outline when unchecked, and filled with the accent color with a white check when checked. In a
  bulleted list, the box takes the marker's place, and the item's text starts at its content column.
- [ ] A checked item's text shows dimmed.
- [ ] Clicking a drawn box toggles it without moving the cursor, as one undo step. The pointer shows
  as an arrow over a box, and as a text cursor elsewhere. A box showing its source edits as text.
  (core: `editing::toggle_checkbox`)
- [ ] Toggling an item without a box gives it an unchecked one. (core)

## Keys in lists

The core's `lists::list_edit` holds these, with tests.

- [ ] Enter in an item starts a new item, at the same level, with the same kind of marker.
- [ ] Enter on a blank item moves it out a level, and on a blank top-level item ends the list.
- [ ] Enter before the marker of a list's first item inserts a blank line before the list.
- [ ] Shift+Enter starts a continuation line in the same item.
- [ ] Tab moves the item, with its children, under the previous sibling, and Shift+Tab moves it out
  a level with its children. In a list, Tab and Shift+Tab never insert a tab, even when they change
  nothing.
- [ ] The keys find the item past blank lines between items.
- [ ] Enter after a code block in a list item starts a new item, and text after the block belongs to
  no item of its code.
- [ ] In a code block, Enter and Shift+Enter keep the line's indentation, and Tab and Shift+Tab move
  the list item that holds the block.
- [ ] In a `table` code block, the keys edit the list table's list as a note of its own, without the
  indentation of a table nested in a list item.
- [ ] Outside lists, the keys do what they do in ordinary text.
- [ ] With a selection, Enter and Tab replace it, except in a pipe table.

## Block quotes

In the core's `editing::quote_newline` and `editing::quote_selection`, with tests.

- [ ] Enter in a quote leaves the innermost quote: a line with one `>` fewer separates the quote
  from the next line, which starts with one `>` fewer. On a quote line with nothing before the
  cursor, that line becomes the separator.
- [ ] Shift+Enter starts the next line with the same `>` markers, in the same quote.
- [ ] A quote in a list item keeps the item's indentation, and the list keys handle a list inside a
  quote.
- [ ] A quote in a list item takes Enter before the item does.
- [ ] Typing `>` with whole lines selected makes each a quote line, blank lines included, and keeps
  them selected, so another `>` nests them. Lines in a list item keep the item's indentation before
  their `>`.

## Pipe tables

In the core's `pipe_tables::table_edit`, with tests.

- [ ] Tab in a pipe table lines up its columns and selects the text of the next cell, and Shift+Tab
  the previous one, even in a table in a list item, and with a selection.
- [ ] Tab past the last cell of a row adds a column, and rows that stray get lined up.
- [ ] Tab beside a table in a list item moves the item, and outside tables does nothing to them.
- [ ] A table that's already lined up doesn't change.

## Typing delimiters

In `attach_wrap_keys`, with the core's `editing::complete_block_opening` and
`editing::fence_selection`.

- [ ] Typing `*`, `_`, `` ` ``, `~`, `=`, or `$` with a selection wraps it in the delimiter on both
  sides, and keeps the text inside selected, so typing it again doubles it. `[` wraps it in `[` and
  `]`. Ctrl or Alt with the key types nothing special.
- [ ] Line breaks at either end of the selection stay outside the delimiters.
- [ ] A third backtick around a code span that fills its lines makes a code block, and a second `$`
  around inline math makes display math, with the content still selected.
- [ ] Without a selection, a third backtick at the start of a line opens a code block, with the
  cursor after the fence for its language, and a second `$` opens display math, with the cursor
  inside. The closing line follows at the opening's indentation, so a block in a list item stays in
  it.

## Hidden syntax

- [ ] Syntax hides while the cursor and selection are away from its element, and shows while they
  touch it, the cursor at either edge included. (core: `Span::touches`)
- [ ] Every piece of syntax in a pipe table shows while the selection touches the table, so its
  columns line up.
- [ ] A heading without text, such as `##` while it's typed, stays plain text until text follows its
  space. (core)
- [ ] A wikilink's target and `|` hide, and its label shows. (core)
- [ ] While a mouse button is down, syntax doesn't show or hide until the button comes up, so text
  doesn't move under the pointer and turn a click into a selection.
- [ ] Changes from other machines restyle the text like typing does.

## Moving the cursor

- [ ] Right onto hidden syntax goes past it, so the cursor lands inside the span from either side.
  Syntax that follows syntax, as `*` follows `**` in `***both***`, goes past both.
- [ ] Left onto hidden syntax lands inside the span too.
- [ ] Math is the exception: the cursor lands outside its drawing from either side.
- [ ] Bullets and drawn boxes keep their room, so a move steps through them.
- [ ] Left onto a task box that takes its marker's place lands before the item's text, with the box
  still drawn. The next Left lands just after the box's `]`, which shows its source. Up or Down from
  the start of another item's text lands before the text too.
- [ ] A move down into a hidden block, such as a table or a diagram, lands at the block's start, not
  its last line.
- [ ] A move that extends the selection shows a hidden block it heads into first, so the selection
  grows through its source a character or a line at a time.

## Clicking

- [ ] A click lands beside the shown character the owner clicked, never inside hidden syntax. A
  click before the first character after hidden syntax that opens a span, at a line's start or after
  white space, lands after the syntax. A click at the end of a word before closing syntax lands
  before the syntax.
- [ ] Clicking a drawing lands at the start of its source. For a block such as display math or a
  diagram, it lands at the start of the second line, inside the opening `$$` or fence, past the
  indentation a list item gives it.
- [ ] Clicking a cell of a drawn table selects that cell's source, nested tables included.
- [ ] Ctrl+clicking a wikilink or embed opens its note, and Ctrl+clicking a web link opens it in the
  default app, or says that no app opens it.
- [ ] A middle click pastes the primary selection.

## Drawings

- [ ] Inline math draws in the line in place of its source, at 1.21 times the text size. Math that
  fails to draw keeps its source.
- [ ] Display math and diagrams draw in place of their lines, centered, with 8 pixels of space
  around them. A failed one shows its source and the error in red, in a monospace font.
- [ ] While the cursor is in a block, its source shows and the drawing stays below it.
- [ ] A block in a list item draws where its source starts, past the item's indentation or marker,
  and fits the width left. Its first line's indentation hides with the source, unless the line holds
  the item's marker, which keeps its bullet.
- [ ] An embed of an image note draws its picture at most 480 pixels tall, within the text's width.
  Embeds of other notes stay links.
- [ ] A code block's shade doesn't show behind a drawing over it.
- [ ] No drawing is wider than the text, past a list item's indentation.

## Tables

- [ ] Pipe tables and list tables draw as grids with borders while the selection is outside them,
  and show their source while it's inside.
- [ ] Header rows and columns show in bold on a light shade.
- [ ] A column's text wraps at 40 characters. Columns follow their alignment: left, center, or
  right.
- [ ] Cells show inline styles, code, block quotes, the titles of linked notes, math, diagrams no
  wider than 480 pixels, and tables nested in cells.
- [ ] A cell whose math or diagram fails to draw shows its source.

## Links and their labels

- [ ] Opening a note shows each wikilink without a label with its target's title. (core:
  `body::fill_labels_mapped`)
- [ ] A link the owner types gets its target's title once the cursor leaves it.
- [ ] A label equal to the target's title, or to any title the editor showed for it, isn't stored.
  Other labels stay as the owner's. (core: `body::strip_labels`)
- [ ] When a target's title changes while the note is open, its links show the new title.
- [ ] A target that's gone shows as a missing note, and one with a tombstone shows its last title,
  styled as deleted.
- [ ] Typing `[[` opens the link picker at the cursor, and wrapping a selection in `[[` and `]]`
  does too, and the selection becomes the link's label.
- [ ] The picker lists the 20 newest notes of the spaces that show while its search is empty, and
  search results from every space otherwise, each with its space's name, never the open note, and
  "Untitled" for a note without a title.
- [ ] In the picker, Up and Down move the selection, Enter or a click picks a note, and Escape
  closes it and leaves the typed `[[` alone.
- [ ] A picked note's link replaces the `[[` with the target's identifier and title, and the cursor
  goes after it.

## Files and images

- [ ] Pasting an image with no text on the clipboard makes an `image` note in the open note's space,
  named for the time of the paste, and an embed of it in place of the selection.
- [ ] Dropping files on the editor adds each as a note in the current space, and opens the last.

## Editing and undo

- [ ] Typing, Backspace, and Delete by character, Ctrl+Backspace and Ctrl+Delete by word.
- [ ] Ctrl+Z undoes and Ctrl+Shift+Z redoes, with each key that edits through the core, such as
  Enter in a list or Tab in a table, one step.
- [ ] Cut, copy, and paste of text with Ctrl+X, Ctrl+C, and Ctrl+V. Copying the hidden syntax along
  with the text it belongs to.
- [ ] Selecting text offers it as the primary selection without dropping it when another window
  selects text.
- [ ] Double-click selects a word, and triple-click a line.

## Saving and live editing

- [ ] Edits store after typing pauses for three seconds, at once while another machine edits the
  same note live, and when the note closes or moves.
- [ ] The window's title is the note's title, from its first line with text, or "Untitled".
- [ ] While another machine edits the note, the title bar says "Editing live".
- [ ] Edits from other machines arrive without moving the owner's cursor or selection off the text
  around them.
- [ ] Other machines' cursors show as carets with their machine's name, and their selections in the
  machine's color, from six colors that each machine keeps on every run.
- [ ] The owner's cursor goes to the other machines at once when a live session starts, and again
  while it stays still. Other machines' cursors that go quiet disappear.

## Around the editor

- [ ] Alt+Left and Alt+Right, and a mouse's back and forward buttons, walk the history of opened
  notes, even with the cursor in the editor.
- [ ] A new note takes the focus when it opens.
- [ ] A panic while styling text doesn't stop the app, and the next change styles the text from
  scratch.
