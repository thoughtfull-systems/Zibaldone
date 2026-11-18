# Draft plan for search, projects, and tasks

This temporary plan records the discussion through November 18, 2026. It separates the owner's
requirements from proposed designs and open questions. It isn't an accepted architectural decision
or a specification, and it doesn't change the existing decisions or authorize implementation. Revise
or replace it as the design develops.

## Confirmed requirements

### Projects and tasks

- **Dates.** Projects and tasks can have scheduled and due dates. Resolve each date independently:
  an explicit subtask or subproject date overrides its ancestors; otherwise inherit the nearest
  ancestor's date. A future effective scheduled date keeps a task out of the work view. No effective
  scheduled date makes it available immediately. Effective due dates give eligible tasks priority
  and affect their appearance before and after they fall due.
- **Hierarchy.** A project is a note that holds design work and other project notes, with tasks and
  subtasks beneath it. A task can have a task or project as its parent. A project can have a project
  as its parent. Standalone tasks can have no project. The hierarchy can show or hide completed
  tasks.
- **Project navigation.** Keep the workflow centered on projects. Searches and views find tasks
  within projects, and selecting a task opens its project with that task in context. Break a large
  project into subprojects by creating projects whose parent is the original project.
- **Repetition.** Tasks support all three modes described below. Each unfinished occurrence that the
  mode retains has its own result row.
- **Reuse.** Calendar events and journals with scheduled prompts should eventually reuse the
  recurrence scheme. Seek compatibility with the calendar standard in
  [Request for Comments (RFC) 5545][ical-rules].
- **Types.** Tasks and projects each have their own note type. To replace an ordinary note with a
  project, the owner creates a new project and deletes the old note.

The repetition patterns include intervals of days, weeks, months, and years; selected weekdays every
specified number of weeks; every weekday; a day of the month; an ordinal weekday of the month; a
month and day each year; and an ordinal weekday within a specified month each year. Ordinals include
first, second, third, fourth, fifth, and last.

The owner confirmed these three modes:

- **Calendar, keep every occurrence.** Keep the schedule anchored to the calendar and retain each
  unfinished occurrence.
- **Calendar, skip missed occurrences.** Keep the schedule anchored to the calendar. Keep the
  current occurrence visible while overdue. Only completing or explicitly skipping it jumps to the
  next current or future occurrence; the passage of time doesn't discard it.
- **Completion interval.** Determine the next occurrence from the last completion and an interval.

### Search and browsing

- **Browsing.** Opening a result shows its note while preserving the results so the owner can select
  another note without returning to a separate results screen.
- **Field conditions.** Combine full-text search across searchable content with conditions on named
  fields. Support comparisons for value kinds that have an ordering.
- **Navigation.** Replace the current sidebar and its search with access to saved searches and a
  separate results pane.
- **Saved views.** Use saved searches or views for recurring queries, including the list of projects
  and the work view.

The owner wants to review the plan and interface sketches before implementation. Commonplace can
inform the design, but resolving every recurrence edge case shouldn't block the earlier features.

## Existing foundations and constraints

- **Field schemas.** [ZIB0002][schemas-decision] defines typed fields and additive schema changes.
  Fields can be absent. Desktop search now supports text matching within fields and typed value
  conditions. Task fields and their effective inherited values still need schemas and indexing.
- **Identifiers.** [The identifier specification][identifiers] requires randomly generated version 7
  Universally Unique Identifiers (UUIDs) for notes. A logical recurrence key isn't automatically a
  note identifier.
- **Note types.** [The space document specification][space-document] makes a note's type immutable.
  Dedicated task and project types respect this constraint. Replacing an ordinary note with a
  project creates a new note rather than changing the old note's type.
- **Search.** [ZIB0008][search-decision] already chooses SQLite full-text search with typed field
  indexes for combined queries. The desktop supports field conditions, Boolean combinations,
  sorting, ranges, and partial timestamps.
- **Storage and sync.** [ZIB0001][storage-decision] uses Loro for authoritative data and local,
  derived search indexes. Concurrent writes to ordinary fields use last-write-wins behavior.

## Proposed implementation order

The order below follows dependencies and delivers a usable task workflow before recurrence. The
recurrence design should account for all three modes before implementing the first one.

<!-- The stages follow dependency order rather than alphabetical order. -->

- **Structured search.** Add a query model shared by the command-line interface (CLI) and graphical
  interface. Support free text, text within a field, exact values, typed comparisons, missing
  values, Boolean combinations, sorting, and relative dates. Exercise it with existing article and
  file fields before adding tasks. A narrow query language is enough for this stage.
- **Saved views and browsing.** Save queries with their space scope, sorting, and presentation.
  Introduce compact saved-view navigation, an independent results pane, and the note editor. Keep
  selection and scroll position when opening notes or refreshing results.
- **Ordinary projects and tasks.** Add project membership, task hierarchy, completion, scheduled
  dates, and due dates. Deliver Projects and the work view through the shared query engine. This is
  the first complete milestone for daily use and should guide further search work.
- **Calendar recurrence.** Implement stable occurrence identities, occurrence state, date overrides,
  and both calendar modes. Validate offline conflicts before committing to the storage format.
- **Completion intervals.** Add repetition measured from completion using the same occurrence and
  editing model. Keep future calendar and journal consumers separate from task completion behavior.

Structured desktop search, saved searches, and the browsing sidebar now work. The working tree now
also implements ordinary projects and tasks, inherited dates, project context navigation, `today` in
date searches, and due-date sorting. Sharing the desktop syntax with the CLI remains follow-up work.
Recurrence remains the next major stage, after trying the ordinary workflow.

## Proposed search and saved views

Keep text matching within a field distinct from exact value matching. An author-name text search and
an exact author value should have different operators. Compare numbers numerically and dates by
their date values. Specify missing-value and list-membership behavior rather than inheriting
accidental database behavior.

A saved search holds a query, space scope, and sort order. It doesn't store a fixed list of result
identifiers. Additional presentation settings remain a possible extension. Relative dates such as
`today` should stay symbolic and refresh when the date changes, even when no note changes. Define
the timezone that supplies those dates when adding relative-date searches.

Saved-search definitions now sync in their home spaces, following [ZIB0022][saved-search-decision].
Temporary selection, scroll position, and sidebar visibility remain in the window session. Search
definitions name spaces by stable identifiers and report unavailable explicit scopes.

### First search syntax sketch

Try field conditions and groups in the existing desktop search box. Spaces imply `AND`. Keep the
existing word prefix matching for ordinary text. Type `@` to see field names from visible spaces.
Use the arrow keys to choose a field and press Tab or Enter to insert its name. Then type the
operator.

```text
(gardening OR composting) AND @authors:"Jane Doe" AND NOT @published-on>=2025-01-01
```

`NOT` takes precedence over `AND`, and `AND` takes precedence over `OR`. Parentheses group text
terms, field conditions, or both. Operators accept any letter case. Quote a word like `"or"` to
search for that word. Quote a field value that contains literal parentheses.

- `@authors:"Jane Doe"` matches both word prefixes within the indexed author text. Quotes group the
  value. They don't require an exact name or phrase.
- `@authors="Jane Doe"` requires the complete author value and respects case and punctuation. For a
  list, one matching value is enough.
- `@id="01m7mpqsqre0b8y1y7gff6bv88"` finds a note by the identifier shown in **Note metadata**.
- `@published-on>=2025-01-01` compares dates. The ordered operators are `<`, `<=`, `>`, and `>=`.
  They apply to dates, decimals, integers, times, and timestamps. Enter dates as `YYYY-MM-DD` and
  timestamps as calendar values in the local timezone or milliseconds since the Unix epoch. A
  partial timestamp matches the whole supplied year, month, day, hour, minute, or second. Range
  starts use the first instant; inclusive ends use the last instant, and exclusive ends use the
  first instant of the ending period. Resolve each boundary's offset for its date, including clock
  changes.
- `@published-on?` requires a populated field. `@published-on!?` finds notes without one, including
  notes whose type doesn't define the field. An empty list or an invalid stored value counts as
  missing.
- `@published-on=2026-01-01..2027-01-01` includes the start and excludes the end.
  `@published-on=2026-01-01...2026-12-31` includes both bounds. Dates, decimals, integers, times,
  and timestamps support ranges. Validate both bounds and keep the start at or before the end.
- `@type=article` restricts results to articles.

A note without a field fails both a word match and a value comparison on that field. `NOT` reverses
that result. To exclude missing dates from `NOT @published-on>=2025-01-01`, also require
`@published-on?`. An invalid query value such as `@published-on>=2025-02-30` produces an error for
the whole search, even inside an `OR` group or under `NOT`. A field that the searched spaces don't
define or that doesn't support the chosen operator also produces an error. The note list displays
the error in place of results.

Add a sort control for relevance, last modified, and title. Relevance uses last modified for a
search with only value conditions. Keep the first pass to the desktop note list. Relative dates and
sharing the syntax with the CLI can follow after reviewing the first pass.

### Workspace sketch

The accepted sidebar has a saved-search dropdown at the top, a multiline query editor with Save and
Save copy actions, and results below. A button shows or hides the sidebar. The note editor remains
beside the results. This layout now works in the desktop app.

- **Ad-hoc search.** Enter search text, add field conditions, and save the resulting view.
- **Note selection.** Selecting another result changes the editor while keeping the query and
  results. A task result opens its project and brings the selected task into view.
- **Projects.** Selecting a project shows its prose and task hierarchy, including a show-completed
  control.
- **Recurring results.** Rows identify the occurrence as well as the task. Selecting two occurrences
  of one task shows shared instructions with the selected occurrence's dates and state.
- **Result changes.** If an edit or completion removes a note from the results, keep its editor open
  until the owner selects something else.

The interactive sketches in the planning conversation illustrate this workspace and a repetition
editor. They explore behavior and layout; they don't define the final interface or query grammar.

## Proposed project and task model

The owner confirmed dedicated note types for tasks and projects. An ordinary note doesn't acquire a
project role or change type. When replacing an ordinary note with a project, the owner creates a new
project and deletes the old note. The initial workflow doesn't need a conversion feature.

Tasks have their own bodies and fields. Present their hierarchy within the project so adding and
organizing tasks doesn't require leaving the project note. The initial implementation follows the
[projects and tasks specification][work-specification]. Parents belong to the same space.

The owner now wants compact task rows with inline title inputs rather than expanded note editors.
Keep existing task body content when editing a title. Projects and tasks show dates beside calendar
icons. Each icon opens a date text box, calendar selector, and Clear date control. Clearing resumes
inheritance. A folder icon opens a parent picker with incremental search over project titles.

The owner confirmed these parent relationships:

- A project can have another project as its parent, or have no parent.
- A task can have a project or another task as its parent, or have no parent.

Projects can't have tasks as parents. A task beneath another task belongs to the nearest project in
its ancestor chain, if one exists. A subproject supplies its own project context for its tasks.
Searches across a larger project should be able to include tasks in its subprojects.

Proposed navigation: selecting a task result opens the nearest ancestor project, expands the task's
ancestor path, and focuses the task without losing the results. Show a project path in task results
so tasks with similar names remain distinguishable. For a task with no project ancestor, open its
standalone task tree. Focus the selected task within that tree. Opening the parent project should
expose its subprojects as well as its direct tasks; the precise presentation still needs a sketch.

Nesting doesn't yet imply dependencies or automatic completion of subtasks. Keep completed ancestors
visible as context when they contain unfinished descendants. Define how concurrent moves preserve a
valid hierarchy. Prevent cycles and invalid parent kinds.

### Inherited dates

The owner confirmed that an explicit subtask or subproject date overrides its ancestors. Resolve
scheduled and due dates independently by walking from the item to its nearest ancestor with that
date. If no item in the chain supplies the date, the effective date is absent. Inheritance crosses
both project and task parents, including subprojects.

For example, a project scheduled for November 20 and due November 30 gives those dates to a task
with neither date set. If that task sets its due date to December 2, it keeps the inherited November
20 scheduled date and uses December 2 as its due date. Its subtasks inherit those dates unless they
supply their own overrides. A subtask can also explicitly schedule itself before its project.

Proposed presentation: show whether each date is explicit or inherited, name the ancestor supplying
an inherited date, and offer a way to remove an override and resume inheritance. Search, sorting,
work eligibility, and urgency should use effective dates. Preserve explicit dates separately so a
parent edit or a move changes inherited dates without rewriting descendants. The implementation now
stores explicit values and indexes effective values as the task specification describes.

The owner confirmed that leaving a date unset means inherit. The initial design has no explicit
option to block inheritance and have no date. Revisit that option only if a need arises.

### Work view

The proposed initial predicate is:

```text
Task is open
AND
(effective scheduled date is absent OR effective scheduled date <= today)
```

Sort eligible tasks by effective due date, with undated tasks afterward. Distinguish due soon, due
today, and overdue with text and visual treatments. A future effective scheduled date still hides an
overdue task; due-date priority doesn't override the scheduled-date gate. A project's scheduled date
is an inherited default, not an additional gate on a task with its own earlier scheduled date.

The implementation uses three calendar days for due soon and sorts ties by title and identifier.
Parent tasks remain eligible alongside their subtasks. Waiting states and inactive projects remain
later work. The preceding eligibility rule describes the initial view, not every possible saved
view.

## Proposed recurrence model

Separate the recurrence definition, each occurrence's stable identity, and its state. A recurring
task note can hold shared instructions; an occurrence holds its completion state and date overrides.
Per-occurrence notes remain a possible later addition.

The calendar rule determines nominal dates. Rescheduling changes an occurrence's effective dates
without changing its identity. Keep the recurrence anchor distinct from the scheduled date that
gates visibility and the due date that controls urgency. Specify how those dates relate before
choosing fields.

### Occurrence actions

- **Change future repetition.** Apply a new definition from an explicit boundary. Preserve history
  and any occurrence that already contains work instead of reinterpreting it under the new rule.
- **Complete a reopened occurrence.** Resolve it again without advancing the series a second time.
- **Complete the current occurrence.** Resolve it and determine what follows using the selected
  mode.
- **Reopen an old occurrence.** Show it as another unfinished row and preserve newer occurrences.
- **Reschedule an occurrence.** Change its dates while preserving its identity and the other
  occurrences. Moving its scheduled date doesn't implicitly change its due date.
- **Restart repetition.** Offer a separate action to change the future cadence from a chosen point,
  with a preview of the resulting dates.

These action rules are proposals. In particular, the owner hasn't yet accepted the reopen and
restart semantics. They aim to let someone correct history without changing current or future work.

For completion intervals, the completion that advances the series establishes its successor once.
Reopening that occurrence doesn't retract the successor. Completing it again doesn't create another
successor or move the existing one. An explicit restart handles an intentional cadence change.

### Finite results

An infinite schedule can't expose every future occurrence without a scheduled date. The recurrence
engine therefore needs a separate rule for which occurrences enter the task view. The proposed rule
for retaining every calendar occurrence includes:

- All unfinished past and current occurrences, subject to their scheduled dates.
- Explicitly reopened occurrences, subject to their scheduled dates.
- Future occurrences whose scheduled dates have arrived.
- The next unfinished future occurrence without a scheduled date.

Each included unfinished occurrence gets its own row. Early completion can reveal the next future
occurrence. This rule needs review; it reconciles immediate availability without a scheduled date
with a finite work list.

### Repetition editor sketch

Expose all three modes by name and explain what happens to missed occurrences. Provide interval,
weekday, monthly, and yearly controls. Distinguish fifth from last weekday choices. Preview what
remains outstanding or comes next after a late completion.

Separate editing this occurrence from editing this and future occurrences. An occurrence-only edit
shows scheduled and due dates and makes clear that other occurrences keep their dates.

### Local-first behavior

Sync recurrence definitions, occurrence state, skips, and overrides. Derive result lists locally.
Avoid making one shared mutable current-task pointer responsible for advancing a series.

The proposed model needs these properties:

- **Coherent advancement.** Merge a completion's scheduling effect as one coherent value.
  Independent writes to the completion time, interval, and successor date could otherwise produce a
  combination neither device intended.
- **Independent edits.** Changes to separate occurrences merge independently. Explicit date
  overrides remain separate from calculated dates.
- **One successor.** Two offline devices completing the same occurrence converge on one logical
  successor. Identity can't depend on whichever completion time a device first receives.
- **Preserved work.** Conflicting future schedules don't silently discard occurrences with edits,
  completions, or reopen actions.
- **Stable history.** Historical changes don't trigger cascading recalculation of later occurrences.

A possible completion-interval representation uses a series generation and cycle identity, with one
retained advancement record per cycle. This remains a candidate, not a storage decision. Prototype
the conflict behavior before defining its containers and keys.

Focused checks should cover concurrent completion, concurrent future edits, completion versus
reopen, occurrence creation on two devices, rescheduling versus completion, and schedule changes
after later occurrences already contain work. Creating the same logical occurrence on two devices
also needs a merge rule for any separately created note bodies; matching keys alone doesn't settle
that problem.

### Calendar compatibility and reuse

Use [the standard recurrence rules][ical-rules] for calendar patterns and preserve their distinction
between fifth and last weekdays. Calendar rules omit nonexistent dates. The original-date identity
in [the standard recurrence identifiers][ical-identity] also supports moving one occurrence without
changing its identity.

Completion intervals and task availability need Zib-specific behavior beyond a fixed calendar rule.
Define what an eventual calendar export preserves and how it represents behavior that has no direct
equivalent.

Share date expansion across tasks, events, and journal prompts while keeping their lifecycles
separate. A missed journal prompt doesn't automatically become an overdue task. Preserve the
distinction between dates, floating local times, and times in a named timezone.

## Open choices and proposed defaults

- **Completion-interval arithmetic.** Proposed default: use the last valid day when adding months or
  years, so January 31 plus one month reaches February's last day. A calendar rule for day 31
  instead omits months without that date. Confirm this distinction.
- **Completion-interval skipping.** Proposed default: skipping starts the next interval from the
  skip date, while recording a skip rather than a completion. Preview the next date before applying
  it.
- **Date relationships.** Decide how the recurrence anchor supplies scheduled and due dates,
  including any offsets, and which timezone converts a completion instant into a date.
- **Hierarchy.** Decide whether parent tasks are work candidates alongside their subtasks.
  Dependencies, repeating whole task trees, and automatic parent completion need separate
  consideration.
- **Reopening.** Confirm that reopening preserves successors and that changing future cadence
  requires an explicit action. Distinguish correcting history from undoing an accidental completion.
- **Same-day jumps.** Proposed default: completing an older occurrence can reveal today's occurrence
  if today matches the rule and remains unfinished. The chosen slot must follow the resolved slot.
- **Task search.** Define task field names, searching inherited values and project descendants,
  relative dates, and sorting by effective due date. Existing desktop search decisions cover the
  general operators and saved-search storage.

## Next planning step

Try the implemented project editor, task hierarchy, inherited date controls, and saved work views.
Use that experience to refine the ordinary workflow. Review the proposed recurrence model and its
offline conflict cases before implementing occurrences. Record durable recurrence choices through
[the decision process][decision-process]. Add exact specifications with the code they govern,
following [ZIB0012][specification-decision].

[decision-process]: decisions/ZIB0000-record-decisions.md
[ical-identity]: https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.4.4
[ical-rules]: https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10
[identifiers]: specs/identifiers.md
[saved-search-decision]: decisions/ZIB0022-saved-searches.md
[schemas-decision]: decisions/ZIB0002-note-types-and-schemas.md
[search-decision]: decisions/ZIB0008-search-index.md
[space-document]: specs/space-document.md
[specification-decision]: decisions/ZIB0012-specifications-and-how-they-change.md
[storage-decision]: decisions/ZIB0001-storage-format-for-spaces.md
[work-specification]: specs/projects-and-tasks.md
