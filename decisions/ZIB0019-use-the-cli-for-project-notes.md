# ZIB0019 Use the command line for project notes

## Context

Coding agents need to find project notes in Zib and sometimes update them. The owner can keep
related notes in any space. The agents already use the shell and share this repository's agent
guide.

## Decision

Use the `zib` command to search and edit project notes. Add an optional space filter to search,
return full note bodies in a batch, and offer machine-readable search and read results. The running
instance continues to own the spaces and carry out each operation.

A write supplies the body the caller read and the body they want to save. Comparing the two against
the current note lets callers retry a completed write and detect a newer edit. Reuse the same
comparison and save path as the agent in Zib.

## Rejected alternatives

- **A Model Context Protocol (MCP) server.** It would give agents dedicated tools. The command
  already reaches the running instance and both agents can use it through the shell.
