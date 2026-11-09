# ZIB0018 An agent chat sidebar

## Context

Zib needs an agent beside the note editor. The agent reads and searches notes, creates notes, and
uses the same markdown editor for its draft. A conversation belongs to one machine until the owner
archives it as a note.

## Decision

Keep one active agent session in the machine's data directory, outside every space. The core owns
its state, persistence, OpenAI requests, and tools. The Linux app draws a right sidebar and asks the
owner to choose a destination whenever the agent creates a note.

<!-- vale Project.WordListCase = NO -->

Use the OpenAI Responses application programming interface (API) through `reqwest`.

<!-- vale Project.WordListCase = YES -->

Stream text and use function tools. Keep the API key, selected model, and system prompt note
identifier in machine settings. Snapshot the system prompt when the first turn starts. Every turn
records its requested model and the model identifiers each response reports. Changing the selection
affects the next turn, including after a restart.

Read and search tools check which spaces show each time they run. The configured system prompt is
explicit context and may belong to any space this machine holds. Each turn also supplies the open
note's title and identifier. The agent fetches its body through its read tool.

The write tool also checks which spaces show. It replaces an existing note's markdown body after
comparing it with the last body the agent read or wrote. A concurrent edit makes the tool ask the
agent to read again. A write changes neither the type nor the fields and enters no editor undo.

The composer uses a detached editor body, with its own text undo. Conversation actions and agent
note creations stay outside the note editor's undo. They have no undo of their own.

The plus button archives the session as an ordinary, editable `agent-transcript` note, then starts a
new session. Archiving never resumes a conversation. The sidebar has no session switcher. The
transcript starts with the session title and labels replies with their models. A durable creation
identifier makes retrying an interrupted archive or tool operation safe from duplicate note
creation.

### Web search

Offer web search alongside the note tools in every conversation request. The configured system
prompt can tell the agent when to search notes and when to search the web.

Use OpenAI's hosted web search in a separate request that carries the model's standalone public
query. This keeps the chat, prompt note, and tool results out of the web research request. Instruct
the agent to keep private details out of the generated query. Return cited findings to the
conversation and keep clickable sources in the sidebar and in archived transcripts.

## Rejected alternatives

- **A session in a space.** It would sync live conversations, which belong to one machine.
- **Chat-level undo.** Reversing a note creation after another edit needs rules for preserving that
  edit and restoring the note. The owner manages created notes through ordinary note operations.
- **Server-owned conversation state.** Local history makes recovery, archiving, and model changes
  independent of a remote conversation's lifetime.
- **Hosted search in the main conversation request.** That request carries the conversation and
  retrieved note text. A separate request limits the search model's input to the public query.

## Consequences

The agent specification defines local settings, session storage, tools, and transcript formatting.
The built-in types specification gains `agent-transcript`. The Settings screen starts with agent
preferences and leaves room for other settings.
