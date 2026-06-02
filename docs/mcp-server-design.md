# MCP Server — Component Design

## Overview

`memoryhub-mcp` is a small standalone binary that exposes a user's MemoryHub memories to any MCP-capable coding agent (Claude Code, Cursor, Codex, Zed, …). It speaks the [Model Context Protocol](https://modelcontextprotocol.io) over **stdio** and is a thin **client** of the existing MemoryHub HTTP API — each tool call becomes one `/v1` request carrying the user's bearer token. One server covers every MCP-speaking agent, so adding an agent is a config snippet, not new code.

## Crate layout

A new top-level workspace member `memoryhub-mcp/` producing the binary `memoryhub-mcp`. It depends only on `rmcp` (official MCP Rust SDK, stdio transport), `reqwest` (`json`, `rustls`), `serde`/`serde_json`, `tokio`, `uuid`, and `thiserror` — **not** on the `memoryhub` crate, so no rusqlite / sqlite-vec / acktor links in and the binary stays small. The handful of request/response DTOs are small and stable, so they are **duplicated** here rather than shared through a new crate.

## Configuration & agent identity

Configured by environment variables in the agent's MCP config block:

```json
{
  "command": "memoryhub-mcp",
  "env": { "MEMORYHUB_URL": "https://…", "MEMORYHUB_TOKEN": "mh_…" }
}
```

`MEMORYHUB_URL` and `MEMORYHUB_TOKEN` are required; a missing one → exit non-zero with a clear stderr message.

Memories are stored server-side under `{username}/{agent_id}/{project}/{filename}` (opaque UUID folder per agent). Each agent gets its own folder, so `agent_id` is resolved per agent, in order:

1. `MEMORYHUB_AGENT_ID` is set → use it.
2. Else key off the client's `clientInfo.name` from the MCP `initialize` handshake and read-or-create `~/.config/memoryhub/agents/<client-name>`.
3. No usable client name → a single persisted `default` UUID.

Keying on the client name (not one fixed file) is what puts each agent in its own folder automatically while staying stable across restarts. `agent_id` resolves right after `initialize`, which always precedes any tool call; the token and `agent_id` are never exposed to the model.

## Tools

Each tool forwards to the API with the resolved `agent_id` and a `Authorization: Bearer <token>` header.

| Tool            | Input                        | Request                    | Result                                           |
| --------------- | ---------------------------- | -------------------------- | ------------------------------------------------ |
| `search_memory` | `{query, scope?, raw_only?}` | `POST /v1/memories/search` | hits as `path (score): snippet`, or "no matches" |
| `save_memory`   | `{path, project?}`           | `POST /v1/memories/write`  | confirmation                                     |
| `read_memory`   | `{filename}`                 | `POST /v1/memories/read`   | content, or "not found"                          |

- **`save_memory`** does not let the model name the memory. It takes the absolute `path` of a file the agent has written, reads that file from disk, and uploads it using the absolute path as the `filename`. A relative path, or a missing/unreadable file, is an error. Writes are replace-by-path, so re-saving the same path updates it.
- **`read_memory`** takes the absolute `filename` (the path used at save time) and returns the stored content from the server.
- **`search_memory`** defaults to the server's `all` scope, so it spans the whole team's memories and summaries; the optional `scope` (`all`/`user`/`agent`) and `raw_only` narrow it. Results show their full path, so the owning user/agent is visible.
- Tool descriptions and the MCP `initialize` instructions are written to drive proactive use (search before a task; save durable notes/decisions/facts).

## Error handling

HTTP/transport failures map to MCP tool errors with actionable messages: 401 → "check `MEMORYHUB_TOKEN`"; connection failure → "cannot reach MemoryHub at `<url>`". A 404 on `read_memory` is a normal not-found result, not an error.

## Out of Scope

Remote (streamable-HTTP) transport (the client core stays transport-agnostic so it can be added later); a destructive `delete_memory` tool; MCP resources/prompts; self-serve token minting.
