# Agent

This specification defines agent settings, the local session, tools, and transcripts. It follows
[decision ZIB0018](../decisions/ZIB0018-agent-chat.md).

## Machine settings

The machine configuration has an `agent` object. Its keys are `api-key` and `model`, strings that
default to empty, and `system-prompt-note`, an optional note identifier. An empty key or model needs
configuration before sending. An absent prompt note uses the agent's built-in instructions alone.
The Settings screen edits these values. The sidebar also edits the same model selection.

These settings never sync. Files that hold credentials and sessions allow only their owner to read
and write them. The configuration and session use the build channel's data directory.

## Session

The machine keeps one session in `agent-session.json`, encoded as JavaScript Object Notation (JSON).
It atomically replaces the file after changes. While streaming, it saves text every 200 milliseconds
and when the turn ends. An absent file starts a new session. These keys use lowercase words joined
by hyphens:

- `archive`: an optional pending creation with `id` and `space`, both identifiers.
- `draft`: the unsent markdown string.
- `id`: the session identifier.
- `manual-title`: a boolean, initially false.
- `prompt`: an optional snapshot of the configured prompt note's markdown.
- `prompt-note`: the identifier of the prompt note captured with `prompt`, if any.
- `title`: a string, initially `New chat`.
- `turns`: a list of turns in submission order.

Each turn holds `message`, `model`, and `reply` strings; optional `context`, with the open note's
`id` and `title`; `reported-models`, a list of strings; `items`, the Responses input items retained
from responses and tool results; `tools`, the tool operations; `status`, one of `complete`,
`failed`, `running`, or `stopped`; and optional `error`, a string. A tool operation holds `call-id`,
`name`, `arguments` as JSON, optional `result` as JSON, and optional `creation` with the reserved
note `id` and destination `space`. A creation can lack a destination until the owner picks one.

A turn's `pending-text` holds streamed text that has no complete response item yet, for crash
recovery.

The first submission snapshots the configured prompt note. Later requests use that snapshot. Changes
to the settings or source note don't change it. An unavailable configured note prevents submission.
The core retains the entire input history locally and sends it with each request. It requests
encrypted reasoning items for replay and disables remote response storage.

A turn pins its selected model through all tool requests and responses. Changing the selector
changes the next turn. Automatic naming infers a topic from the first submitted message. A manual
rename prevents an automatic name from replacing it. Names contain one line.

The composer has text undo independent of the main editor. Other session actions, note creations,
and transcript creation enter neither editor's undo history. A running turn disables archiving.
Stopping preserves partial text and completed tool results. Restarting marks an interrupted turn as
stopped. Retry continues the last stopped or failed turn with its original model and recorded tool
results. It doesn't resubmit a completed creation.

## Tools

Tools return JSON. A failure returns an `error` string. Note identifiers use their canonical form.

- `create_note` takes a markdown `body` string. The owner chooses a destination for each creation.
  Cancellation returns an error to the agent. Success returns `id`, `title`, and `space`.
- `get_note` takes an `id` string. It returns `id`, `title`, and markdown `body`. The note must be
  in a space currently shown in the note list.
- `search_notes` takes a `query` string. It searches currently shown spaces and returns up to 20
  results, each with `id`, `title`, and `snippet` strings. Match markers don't enter the snippet.
- `write_note` takes an `id` string and a markdown `body` string. It replaces an existing note's
  complete body in a currently shown space and returns `id` and `title`. It keeps the note's type,
  fields, and identifier. It never creates a missing note or restores a deleted one.

Before writing, the agent reads the note with `get_note`, unless it already knows its body from a
successful creation or write. The writer compares the current body with that known body and returns
an error if it changed. An already matching replacement succeeds without replacing later edits. The
writer saves, indexes, and syncs the updated note. The open editor refreshes, and the write stays
outside the main editor's undo. Transcripts link to updated notes beside the write results.

The core checks visibility when each read or search runs. The configured system prompt is explicit
context, independent of tool visibility. The model never chooses a creation's destination.

A writer reserves and saves a note identifier before creating a note. Retrying that operation
returns the existing note with that identifier. It preserves subsequent edits, and never recreates a
note deleted for good. Before finishing the operation, it saves the destination space to disk.

## Errors

The agent reports the provider's error code and message for failed requests, including errors sent
inside a response stream. It includes the status code for Hypertext Transfer Protocol (HTTP) errors.
It reads error details from the event itself, a nested `error`, or `response.error`. When a response
is incomplete, it reports `incomplete_details.reason`. Without a provider message, it explains known
billing, quota, rate, and token limits, and content filtering. An unknown failure still identifies
the event or HTTP status when available. It redacts credentials from error messages.

A stream that closes without a final event reports that interruption separately. Partial reply text
remains in the session, and a failed request doesn't automatically retry.

## Transcripts

The plus button asks for a destination, creates an `agent-transcript` note, and replaces the local
session with a new empty session only after the space saves. The pending archive records its note
identifier and destination before creation. Retry uses that same pair. Cancelling leaves the session
active. An empty session starts over without creating a transcript.

The body starts with a level-one heading holding the session name. Turns follow in order, each with
a `User` heading and the submitted markdown, followed by an `Assistant` heading with the reported
models, or the requested model when no response reports one, and the reply markdown. Tool activity
follows with tool names, results, and links to created notes. A nonempty unsent draft follows under
a `Draft` heading. Stopped and failed turns show their status.

A transcript is an ordinary editable note. Editing it never changes a live session. The sidebar has
no operation to load or continue a transcript, and no switcher or recent-session limit.

## Platforms

Linux shows the agent in a resizable right sidebar. A button shows or hides it. Ctrl+Tab switches
between the note editor and composer, showing the sidebar when necessary. Ctrl+Enter and Send submit
the draft. Enter retains the markdown editor's behavior. Stop ends generation. The Settings screen
starts with agent settings.

When archiving a chat, Linux uses the only space if the machine holds one. If the machine holds more
than one space and shows only one in the note list, Linux uses the shown space. Otherwise, the
destination picker lists every space. A pending archive keeps its recorded destination.
