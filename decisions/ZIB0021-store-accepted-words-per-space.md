# ZIB0021 Store accepted words per space

## Context

The note editor needs a way to remember words its base dictionary doesn't recognize. A name or
technical term can belong to one space's vocabulary even when it would be a typo in another.
Machines that share a space need to agree on which words the owner accepts there. The owner needs to
be able to edit or remove accepted words in Zib's note editor.

## Decision

Store accepted words in a regular note in the space, one word per line. A space setting identifies
the note so renaming it doesn't change its role. The editor appends a word when the owner accepts it
and reads the note again after edits. The owner can edit the note the same way as any other note.

Two machines can create the dictionary before either receives the other's change. Each registers its
own note and the editor checks both, so the words from each machine survive sync. New additions go
to the first note by identifier. The [space document specification][dictionary] defines the settings
and how to read the words.

Keep the base dictionary separate from the space document. The core checks prose and suggests
replacements locally with the `spellbook` library, which reads dictionaries in the Hunspell format.
Linux ships an American English dictionary with the app. The editor checks the note's source and
applies each replacement through its existing undo history.

## Rejected alternatives

- **A list in a space setting.** A write replaces the whole list, so concurrent additions can lose a
  word.
- **A map of accepted words.** The owner needs a separate editor to remove or change words.
- **The operating system's personal dictionary.** Words can affect unrelated spaces and apps, and
  the space can't carry them to another machine.

[dictionary]: ../specs/space-document.md#spelling-dictionary-notes
