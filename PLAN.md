# Draft plan for search, projects, and tasks

This temporary plan records the discussion through November 19, 2026. It separates the owner's
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
  conditions. Search indexes effective task dates and work status.
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

Structured desktop search, saved searches, and the browsing sidebar now work. Ordinary projects and
tasks now support inherited dates and work status, project navigation, relative dates in searches,
and due-date sorting. Sharing the desktop syntax with the CLI remains follow-up work. Recurring
tasks are the next stage.

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
- **Recurring results.** Rows identify the occurrence as well as the task. Selecting an occurrence
  shows its copy of the instructions, checklist progress, dates, and outcome.
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

The default **What should I work on?** search uses effective dates and work status.

```text
@type=task AND @recurrence!? AND @completed-on!? AND @skipped-on!? AND @superseded-on!?
AND (@scheduled-on!? OR @scheduled-on<=today) AND @status!?
```

A future scheduled date hides even an overdue task. Someday, Maybe, and Waiting hide a task until
its effective status clears. Parent tasks can appear alongside their subtasks. The view sorts by
effective due date, then title and identifier, with undated tasks last. Due soon covers today and
the next three calendar days. The [task specification][work-specification] describes inheritance,
search, and display.

## Proposed recurrence model

This draft allows reopening a completed occurrence. One task can have multiple open occurrences.
Reopening one corrects that occurrence. The next occurrence keeps its identity, its edits, and any
successor it has already produced. Disallowing reopening remains the simpler option if correcting a
mistaken completion proves unnecessary.

A recurring task owns its body template, parent, work status, and rule. Each occurrence has a
recurrence date (its nominal day), a copy of the body, its own date overrides, and an outcome of
open, completed, or skipped. A realized occurrence is one the owner can act on, even if a saved
search hides it. When starting a series, choose its first occurrence date. For a calendar rule,
preview the first matching date on or after the chosen start. A past start produces a backlog in
keep-every mode. In jump and interval modes it realizes the first occurrence and waits for an
action. A preview shows possible dates. Opening a preview, project, or search has no side effects.

### Linked occurrence notes

Use a separate `task` note with a series link for each realized repetition. The recurring task holds
the rule, template, project context, and the records that prevent a second advancement. The
occurrence note owns its editable Markdown body, checklist, date overrides, and outcome. Store each
body once. Checking off May edits the May note and advances the series, when applicable, in the same
commit. Reopening May uses that same note and link.

The occurrence editor links to its series in the Parent field. Later edits and reopening retain the
owner's text.

An occurrence's stable identity already includes its recurring task's identifier. Expose that
identifier to search as a read-only `@series-id` field. Use it for project context and recurrence
actions. Keep `recurrence-on` for the occurrence date and `parent-id` for the project and task
hierarchy. A journal entry can link directly to the May occurrence note after June becomes current.

Create notes as the rule realizes work. Jump and completion intervals realize the first occurrence
and then one successor at each advancement. Keep-every catches up retained dates after time away,
including open backlog nobody has edited. The finite future rows in Dates and visibility also get
notes when they become work. An explicit skip retains its note even without a comment. Simply
previewing next year's calendar creates no notes. A rule change supersedes the old rule's open notes
and keeps them addressable in history.

### Behavior by mode

Start a daily task on November 10 with its scheduled date set to its nominal day. In this table,
numbers are November dates. Each row continues the preceding row. The three result columns show the
open rows in **What should I work on?** after each action.

| Action                       | Keep every date | Jump   | One day from completion |
| ---------------------------- | --------------- | ------ | ----------------------- |
| Nov 10, open                 | 10              | 10     | 10                      |
| Nov 13, return               | 10, 11, 12, 13  | 10     | 10                      |
| Nov 13, complete 10          | 11, 12, 13      | 13     | Empty until 14          |
| Nov 13, reopen 10            | 10, 11, 12, 13  | 10, 13 | 10                      |
| Nov 14, complete 10 again    | 11, 12, 13, 14  | 13     | 14                      |
| Nov 14, skip latest open row | 11, 12, 13      | 14     | Empty until 15          |

In the interval column, the first completion on November 13 realizes the November 14 occurrence
immediately. Its scheduled date hides it for one day. Reopening November 10 leaves that successor
intact. In the last row, the jump column skips November 13, making November 14 available that day.

- **Calendar, keep every.** The rule supplies one occurrence per matching date, starting at the
  first date. Completing or skipping any of them affects only that row. Completing Friday before
  Thursday leaves Thursday open. Closing the app for a month retains the month's backlog.
- **Calendar, jump.** Keep one occurrence that can advance the series. On its first completion or
  explicit skip, take the first matching nominal day strictly after its nominal day and on or after
  the action day. Take a Monday rule whose current occurrence is November 2. Complete it on Monday,
  November 16 and the next occurrence is November 16. Complete both on that day and the next is
  November 23. Completing November 16 early, on November 12, also yields November 23. November 9 in
  the late example never became an occurrence and has no history row to reopen.
- **Completion interval.** Keep one occurrence that can advance the series. Add the positive
  interval to the day of its first completion. This draft uses the day of an explicit skip in the
  same calculation and records a skip. Completing an occurrence early can bring the next one
  forward. Two occurrences can even share a nominal date. Each has its own identity.

### Actions on a realized occurrence

| Action                       | Effect on the selected row        | Effect on later work         |
| ---------------------------- | --------------------------------- | ---------------------------- |
| Complete                     | Set its completion date           | First close follows the mode |
| Skip                         | Record the skip date              | First close follows the mode |
| Reopen a completion or skip  | Clear the current outcome dates   | Keep every successor         |
| Complete or skip it again    | Record the new outcome and date   | Keep every successor         |
| Move a scheduled or due date | Override that date only           | Keep the cadence             |
| Clear a date override        | Resume its series or task default | Keep the cadence             |

In jump and interval modes, retain the first advancement even when the owner reopens the row. For
example, complete A to realize B, complete B to realize C, then reopen A and B. All three can now
appear as open rows. Only C can advance the series. Correcting A's completion date, reopening B, or
closing either of them again leaves C alone. An explicit change to the recurrence can start a
different cadence.

An open occurrence has neither outcome date. A completed one has only `completed-on`. A skipped one
has only `skipped-on`. Reopening clears both in the winning action. The default work search checks
for missing outcome dates and uses `@recurrence!?` to exclude series templates. It includes
incomplete ordinary tasks and available occurrences. Preserve customized saved searches. Show the
nominal day beside each occurrence result. When a task has two rows with the same nominal day,
distinguish them by cycle number within a revision or identify the earlier rule across revisions.
Selecting one opens that occurrence's note. Show its title and index its own body for searches and
snippets.

### Dates and visibility

Give the recurrence date its own meaning. It identifies which day the rule chose for the occurrence.
The scheduled date controls availability. The due date controls urgency. The editor can express both
dates as calendar-day offsets before, on, or after the recurrence date, or use the task's ordinary
dates. Suppose a report repeats on day 15 of each month. Schedule it three days before and make it
due on the recurrence date. In May, work appears on May 12 with a May 15 deadline. Postponing work
to May 14 leaves the May 15 deadline and the June 15 recurrence in place.

Resolve scheduled and due dates independently. For each, use the first applicable source in this
order.

1. The date the owner set on this occurrence.
1. A series setting such as “schedule two days before the nominal day” or “due on the nominal day.”
1. The task's ordinary date, including its nearest ancestor's date if the task inherits.

With no source the date is absent. An inherited project deadline of November 30 stays November 30 on
every occurrence using it. Moving one occurrence's scheduled date leaves its due date alone. Editing
a revision's date offset or an ancestor date updates open rows that use it. When someone completes
or skips an occurrence, save its effective scheduled and due dates with that outcome, including
dates it lacked. History and searches of closed occurrences use those saved dates. Reopening resumes
live dates. Closing again captures the dates for the new outcome. An ordinary subtask inherits
through task and project notes. Selecting, moving, or completing one occurrence of its parent never
moves, completes, or duplicates the subtask.

For a new series, suggest scheduling on the nominal day and taking the due date from the task. Show
the first dates in the preview. Allow either date to use the task's ordinary inheritance. When the
effective scheduled date is absent, even a realized future occurrence is available immediately. A
due date affects urgency and sorting. A future scheduled date still hides an overdue row. A Waiting
project can hide open occurrences for weeks. When the status clears, keep-every shows the backlog,
while jump and completion intervals still show their outstanding occurrence and any reopened rows.

Keep the work list finite by finding candidate rows before applying the saved search. For
keep-every, include the following, counting each occurrence once.

- Every nominal day from the start through today.
- Future days whose series scheduling offset makes them available by today. With a two-day lead,
  expand only through today plus two days.
- The next future occurrence if no keep occurrence is open. Keep existing occurrences, including a
  future one created before the owner reopened an earlier occurrence.

Then apply the actual scheduled date, status, and outcome filters to each candidate. A daily task
that inherits yesterday's project date can expose its backlog. That inherited absolute date can't
bring an unbounded number of future rows into the search. Jump and interval modes supply their
realized current row and retained history. The same eligibility filters apply. A bounded calendar
preview can expand dates beyond these rows.

Tasks use calendar dates. Record the acting machine's local day once when the owner completes or
skips an occurrence, following the existing completion convention. Use that recorded day for
advancement across sync, travel, and daylight saving changes. Each device still evaluates the work
search's `today` in its own local timezone.

### When to stop repeating

Offer one ending choice per rule. **Never** leaves it open. **Through a date** includes that
recurrence date if the rule matches it. **After N occurrences** includes the first occurrence and
requires a positive whole number. Preview the first matching date. Reject an end date before it. The
two calendar modes can express these choices with the optional `UNTIL` or `COUNT` in an
[RFC 5545 recurrence rule][ical-rules].

| Mode                 | What counts toward N                                      |
| -------------------- | --------------------------------------------------------- |
| Calendar, keep every | Each matching calendar date                               |
| Calendar, jump       | Each matching date, including dates a late jump passes    |
| Completion interval  | Each distinct cycle, starting with the initial occurrence |

For a three-Monday rule starting November 2, the matching dates are November 2, 9, and 16.
Keep-every retains all three. In jump mode, finishing November 2 on November 16 can reveal November
16\. Finishing November 2 on November 17 ends the series with just that one realized row. For an
interval with a limit of three, completing or skipping A can create B, and closing B can create C.
Closing or reopening C never creates D. Correcting A can't consume another cycle.

A limit stops new occurrences. It leaves the last open occurrence available past the end and
preserves earlier unfinished rows. An occurrence on May 31 can have a due date of June 2, or move
its scheduled date into June, even when the series runs only through May 31. Use the nominal
candidate date when testing an interval successor against the end date. For concurrent first closes,
choose the winning proposal before applying the limit. If the two proposals give May 31 and June 1,
the earlier-date rule retains the single May 31 successor. Record a first close even when the limit
prevents a successor, so reopening can't restart the series.

### Recurring body checklists

Use the task's whole Markdown body as its template. Each occurrence starts with its own copy. Reset
checked task-list items, including nested ones, to unchecked. Preserve the surrounding text, links,
and formatting. A literal `[x]` in a code example stays as written. Zib already recognizes task-list
items in its [note body format][note-bodies].

For example, the template for a monthly report contains “Collect figures” and “Send report.” Check
the first item in May and add a note about a missing invoice. June starts with both items unchecked
and the template's original prose. Reopening or rescheduling May keeps the invoice note and its
checkmarks. Checking the last item leaves completion of the occurrence to the owner. Checklist items
live in that body. The task hierarchy keeps its existing subtasks and their dates.

Edit the recurring task's body to change the starting text for all newly created occurrences,
including a backlog that the app has yet to create. Edit an occurrence directly to change its own
copy, including its title line. Dates that jump mode passes over never acquire a body. Explicitly
skipping a realized occurrence keeps its body and checklist.

Two offline devices starting from the same series body must converge on one copy of the starter text
with both sets of checklist edits. Checking the same item on both must yield one valid checked
marker. When devices create the same occurrence from different series bodies, merge the two
initializations and subsequent edits as concurrent text changes from a shared starting point. Treat
body edits as work to preserve across rule changes.

### Browsing past occurrences

The existing backlinks pane lists each occurrence whose body links to the recurring task, alongside
other notes that link to it. Removing the last body link to that task removes the backlink. The
occurrence still belongs to the same series and appears in its history. Linking to another series
adds an ordinary backlink there. Neither edit changes which task advances when the occurrence
closes.

Make **Occurrence history** a shortcut to a search for that series, ordered by recurrence date. For
example, this proposed search finds skipped occurrences that mention an invoice. It uses the example
series identifier from the [identifier specification][identifiers].

```text
@series-id=01k97krzh1fcx9rk9fhsgt1de7 @skipped-on? invoice
```

Offer **Occurrence history** in the recurring task's menu. In a project, use the right pane for the
dated list and the note pane for the selected occurrence's body. Keep the list visible when the
owner clicks another date. **Back to project** restores the project note and its task list. A
standalone recurring task offers the same history in its own view. A body search hit opens the
matching occurrence in context and preserves the search sidebar.

List past occurrences newest recurrence date first, with **Show earlier** for older rows. Include
completed, explicitly skipped, reopened, and still-open past occurrences. Include a future-dated
occurrence handled early as soon as the owner handles it. Keep one row per occurrence. Use the cycle
or rule to distinguish rows with the same date. Show the outcome and its completion or skip day. For
example, “May 15 · Skipped May 18” is a single occurrence. The project's **Show completed** setting
controls the normal task list. History includes closed rows regardless of that setting. An ended
series remains accessible from its project with **Show completed** or through a search for any of
its retained occurrences.

Opening May shows the checklist and comments from May's saved Markdown. Each box keeps its state.
Reading leaves the outcome alone. **Edit this occurrence** allows a later correction or note without
reopening it. History shows the latest saved body for each occurrence. Reopening May keeps its notes
and updates the same row. A change to June's template can't rewrite May.

### Notes on completion or skip

Keep the ordinary checkbox and quick **Skip** action. Offer **Complete with note…** and **Skip with
note…** when the owner wants to record why. Show the occurrence date while typing. Saving appends a
dated paragraph to that occurrence's body and records the outcome together. Canceling makes no
change to the occurrence. A quick completion or skip adds no text. For example, skipping the May 15
report on May 18 could leave this paragraph below its partly finished checklist.

```markdown
**Skipped 2026-05-18** — Finance hasn't sent the April figures.
```

The paragraph is ordinary editable, searchable Markdown. Its wording never controls scheduling.
Reopening the occurrence preserves it. A later completion can append its own note, and neither entry
enters the series template. Someone who already clicked the checkbox can open that same occurrence
in history and add a note without changing the outcome.

Keep the editor bound to the occurrence selected when it opened. If sync closes it while the owner
is typing, preserve the draft and offer to add the text to that occurrence. Never redirect the draft
to the successor. Replaying a submission must add its paragraph only once. If two offline devices
submit different explanations, preserve both paragraphs. The existing conflict rules resolve the
outcome and allow at most one advancement.

### Commonplace prototype

The Commonplace recurrence prototype in `thoughtfull-systems/vibes` has a History dialog for
individual occurrences and a **Show earlier** button. Its **Write a note** action creates a linked
memo. For Zib, use that direct access to an occurrence and put the explanation beside its checklist
in the same body. See `commonplace/linux/src/history.rs` and `commonplace/core/src/recurring.rs` in
the [Commonplace prototype][commonplace-prototype].

Commonplace's recurring-actions decision saves the effective dates when an occurrence closes and
targets commands to the occurrence shown on screen. Both choices help when dates or the current
occurrence change on another device. Its completion-relative example also illustrates the cost of
recalculating a successor. Completing January 3 gives a January 10 successor. Reopening and
completing again on January 6 moves that successor to January 13 in the Commonplace design. The Zib
draft keeps January 10 and any checklist work already there.

### Changing or ending a series

A change to the pattern, mode, or first date starts a new revision of the same task. Choose today or
a future date for the new rule to start. Every open occurrence of the old rule gets a
`superseded-on` date even if it comes from the backlog or has edits. Its note and date overrides
remain in history. Completed and skipped occurrences retain their outcomes. A reopened occurrence of
a replaced rule appears as superseded. Ending a rule applies the same supersession and creates no
additional ordinary task. Deleting the task follows the existing subtree deletion rule.

The replacement preview carries forward the existing end date. For a count limit it suggests the
number of occurrences remaining at the boundary if any remain. A new count starts with the first
occurrence of the replacement rule.

### Offline behavior to verify

For keep-every, identify a slot by the task, rule revision, and original calendar day. For jump and
completion intervals, identify it by the task, revision, and cycle number. Two devices that close
cycle 4 both refer to cycle 5, even if they act on different days. These keys identify occurrences
within a task. The task keeps its ordinary note identifier.

Two offline devices can independently realize the same occurrence. Under Zib's existing random note
identifiers they would create different note addresses. Selecting one address after sync can strand
a link to the other and lose edits. Prototype deriving the generated occurrence note's identifier
from its stable occurrence key. This would require an explicit exception to the
[identifier rules][identifiers] for generated notes. The alternative with random identifiers would
need permanent aliases and a way to merge independently initialized bodies.

Test two offline devices that each realize May, check different boxes, add different notes, and link
to May from their journals. After sync both links must open the same body with both edits and one
copy of the checklist. Add a body link to the series and remove it again. Verify that the backlink
disappears while history and the work search still show May once. Repeat with both devices
completing May and verify one successor. Sharing an identifier alone doesn't establish that
concurrent creation and text initialization merge correctly.

Keep two facts separately for each advancing cycle. One records the first close and the nominal
successor it proposed. The other records corrections to the row's current outcome. An action that
already saw a close of this cycle can only correct the outcome. Two offline devices can each make a
first close before learning of the other. As a draft tie-breaker, use the proposal with the earlier
successor day, breaking ties by a stable action identifier. Take that proposal's outcome, action
day, and successor day together. A retry of the same action has the same identity. For example, two
first completions on November 9 and 10 with a seven-day interval propose November 16 and 17 for the
same cycle. November 16 wins. Later correcting the displayed completion to November 12 leaves that
successor in place.

With no corrections, the winning proposal supplies the displayed outcome. A correction takes
precedence over the initial outcome. Between corrections, a later action that saw the earlier one
wins. Among concurrent corrections left after that comparison, prefer reopening, then use stable
action order for ties. For keep-every use that same causal ordering for outcome edits, including the
first closes. Independently edited scheduled and due overrides merge separately. Conflicting writes
to one override use the existing field ordering.

| Offline actions                      | Result after sync                                    |
| ------------------------------------ | ---------------------------------------------------- |
| Both complete A                      | One outcome and at most one advancing successor      |
| Complete A and skip A                | One proposal supplies outcome and successor together |
| Reopen A and complete B              | A open, B completed, one C                           |
| Reopen A and close a stale copy of A | A open, one original advancement                     |
| Move A's due date and complete A     | A keeps its override and completion                  |
| Revise the rule and work on old B    | B keeps its work under the old revision              |
| Choose competing new rules           | One active rule, retained work from both             |
| Delete the task and complete A       | The deleted task stays deleted                       |

For example, two offline devices close the November 2 occurrence of a Monday jump rule. One records
a November 9 completion and proposes November 9 for B. The other records a November 15 completion
and proposes November 16 for B. Both refer to the same B. After sync B's default nominal day is
November 9. An explicit due date set on either copy of B stays attached to it. If someone already
completed B and produced C, keep that advancement and C's dates. A disputed default date can change
on sync. Reopening an occurrence on one device can never cause that change by itself.

Treat a concurrent rule replacement as a choice between complete revisions. At a fork from the same
revision, pick one by stable revision order, independent of message arrival. Follow the winning
revision's subsequent edits. Keep occurrences on the other branch in history, with open ones marked
superseded. Derive the superseded date from the winning revision at the point of divergence. Closing
an occurrence offline can create a provisional successor. If a replacement rule wins, sync keeps
that successor in history as superseded. Replay each example with reversed delivery order, duplicate
updates, and a restart, comparing row identities, dates, outcomes, and the next eligible
advancement.

### Calendar rules and reuse

Use [RFC 5545 recurrence rules][ical-rules] for calendar expansion. A monthly rule for day 31 yields
January 31 and March 31 in 2027. A rule for the fifth Monday omits a month that has four Mondays,
while a last-Monday rule still yields a date. Keep the first matching date and the week start with
an every-other-week rule so every machine agrees which weeks qualify. Moving an individual calendar
slot keeps its original date for the [standard recurrence identifier][ical-identity].

For completion intervals, propose calendar arithmetic in whole days, weeks, months, or years. Clamp
a missing day to the last day of the target month. Completing on January 31, 2027 with a one-month
interval gives February 28. Completing that occurrence on February 28 gives March 28. A one-year
interval from February 29, 2028 gives February 28, 2029. The interval editor needs a quantity and
unit. Weekday and ordinal selectors belong to the two calendar modes. The editor should distinguish
editing this occurrence's dates from changing the future rule and show the dates each choice
affects.

Share calendar expansion and date exceptions with future event and journal features. A calendar rule
can describe nominal task dates. Jumping on completion, accumulating unfinished work, and measuring
from completion also need task lifecycle information. An export must identify which behavior it
preserves. Events with times will need their own timezone rules.

## Next planning step

Review how the draft handles an explicit skip in a completion interval, the first unscheduled future
row, and an offline disagreement about the next date. The tables use a proposed default for each.
Before building the editor, exercise these examples against two offline replicas in the core,
including a successor that already has its own edits and a revised rule with unfinished backlog.
Record the resulting durable choices through [the decision process][decision-process] and add exact
specifications with the code they govern, following [ZIB0012][specification-decision].

[commonplace-prototype]: https://github.com/thoughtfull-systems/vibes/tree/49a4ba5/commonplace
[decision-process]: decisions/ZIB0000-record-decisions.md
[ical-identity]: https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.4.4
[ical-rules]: https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.10
[identifiers]: specs/identifiers.md
[note-bodies]: specs/note-bodies.md
[saved-search-decision]: decisions/ZIB0022-saved-searches.md
[schemas-decision]: decisions/ZIB0002-note-types-and-schemas.md
[search-decision]: decisions/ZIB0008-search-index.md
[space-document]: specs/space-document.md
[specification-decision]: decisions/ZIB0012-specifications-and-how-they-change.md
[storage-decision]: decisions/ZIB0001-storage-format-for-spaces.md
[work-specification]: specs/projects-and-tasks.md
