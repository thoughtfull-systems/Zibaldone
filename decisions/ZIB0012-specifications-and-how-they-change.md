# ZIB0012 Specifications and how they change

## Context

The decisions so far choose formats, protocols, and libraries, and explain why, but they leave the
exact details to "a later specification": the containers and keys in a space's document, the
encoding of each field's values, the syntax of wikilinks and list tables, the messages that sync and
pairing exchange, and more. [Decision ZIB0007](ZIB0007-programming-languages-and-technologies.md)
makes the specifications the contract between implementations, even though they share one core, and
has each one record how platforms differ. [Decision ZIB0001](ZIB0001-storage-format-for-spaces.md)
lets the layout of a space change only by adding to it.

Nothing yet says where specifications live, what belongs in them, or how they change.

## Criteria

- **Precise.** A specification says exactly what to write and read, so someone could build a
  compatible implementation from it, and the core's tests can check against it.
- **Easy to find.** A reader finds the specification for an area without knowing which decision
  mentions it.
- **Current.** A specification changes in the same commit as the code that implements the change.
- **Compatible.** A change never breaks what an earlier release wrote or expects, in keeping with
  decision ZIB0001.
- **Low ceremony.** Writing or changing a specification takes no more process than a commit.

## Decision

Keep specifications as markdown files in a `specs` directory, one for each area, and change them
only by adding to them.

- **What gets one.** Anything that more than one release, platform, or process has to agree on: the
  data in a space and on disk, the syntax of note bodies, the sync and pairing protocols, and the
  protocol between the `zib` command and the running instance. Details that only the core reads,
  such as the search index's tables or the page styles for printing, belong to the code and its
  tests.
- **Names.** Each file takes the name of its area, in lowercase words joined by hyphens, such as
  `specs/space-document.md`, with no number. Names never change, so links to them stay valid.
- **Decisions and specifications.** A decision chooses and explains why. A specification states
  exactly what, and links to the decisions behind it. A change that contradicts a decision needs a
  new or revised decision first.
- **Writing.** Specifications state each requirement as a plain sentence in the present tense, such
  as "A writer sets `created` once", and follow the same writing rules and checks as every other
  markdown file. Where a syntax needs precision, a grammar in Augmented Backus-Naur Form (ABNF)
  defines it. Examples and test vectors, such as an identifier with its encoded text, sit beside the
  rules they illustrate.
- **Changes.** A specification grows by adding keys, messages, syntax, and values. It never removes
  one, renames one, or changes what one means, and it says nothing about releases or versions. Git's
  history records when each part arrived.
- **Platforms.** Each specification ends with a section that lists where a platform differs, such as
  the Android app's lack of printing, or says that none do.
- **Tests.** The core's tests name the section of the specification they check, so a change to a
  rule leads to its tests.

Against the criteria:

- **Precise:** rules, grammars, and test vectors leave little to interpretation, and tests tie each
  rule to code.
- **Easy to find:** one directory, with a file named for each area.
- **Current:** a specification lives beside the code and changes in the same commits.
- **Compatible:** the additive rule applies to every specification, as it does to the document.
- **Low ceremony:** a specification is a markdown file, changed by an ordinary commit.

## Rejected alternatives

- **Details inside the decisions.** Each decision would carry its own exact formats, so a reader
  would need only one document. But decisions record a choice when it's made, and adding exact
  details would make them long and make every refinement a revision of a decision. It fails easy to
  find and low ceremony.
- **The code as the specification.** The core's types and tests would define everything, with no
  documents to keep current. But a reader would have to read Rust to learn a format, and nothing
  would separate a deliberate rule from an accident of the code. It fails precise.
- **Numbered specifications with statuses, like requests for comments.** Numbers and statuses such
  as draft and final would track maturity. But a single maintainer gains little from them, and
  statuses go stale. It fails low ceremony.
- **A formal schema language for every format.** Tools could check data against a language like
  Protocol Buffers or JSON Schema. But Loro's containers, such as text and movable trees, don't map
  onto those languages, and the sync protocol rides on iroh's own framing. Grammars where they help
  serve better. It does worse on low ceremony, for little gain in precise.

## Consequences

- The repository gains a `specs` directory, and AGENTS.md points to it beside the decisions.
- Each later specification starts as a commit that adds its file. Its rules and tests follow in the
  code.
- A specification that needs a breaking change follows decision ZIB0001: the owner migrates by hand,
  with a decision that records the change.
