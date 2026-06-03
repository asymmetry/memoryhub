# MCP Server — Component Design

## Overview

`memoryhub-mcp` is a small standalone binary that exposes a user's MemoryHub memories to any MCP-capable coding agent (Claude Code, Cursor, Codex, Zed, …). It speaks the [Model Context Protocol](https://modelcontextprotocol.io) over **stdio** and is a thin **client** of the existing MemoryHub HTTP API — each tool call becomes one `/v1` request carrying the user's bearer token. One server covers every MCP-speaking agent, so adding an agent is a config snippet, not new code.

With no subcommand it runs the stdio MCP server above; the `upload` and `recall` subcommands (see [Hook-support CLI](#hook-support-cli)) make it the shared engine for the deterministic hook layer that per-agent plugins drive. The binary never parses a raw agent hook payload — each plugin normalizes first (see [claude-code-plugin-design.md](claude-code-plugin-design.md)).

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

`MEMORYHUB_URL` and `MEMORYHUB_TOKEN` are required; a missing one → exit non-zero with a clear stderr message. Hook-CLI mode does not inherit the MCP block's env, so it reads the same two values from the env first, then from `<config_dir>/memoryhub/config.json` (`{url, token}`).

Memories are stored server-side under `{username}/{agent_id}/{project}/{filename}` (opaque UUID folder per agent). Each agent gets its own folder, so `agent_id` is resolved per agent, in order:

1. `MEMORYHUB_AGENT_ID` is set → use it.
2. Else key off a client name and read-or-create `<config_dir>/memoryhub/agents/<client-name>` — in MCP mode the name comes from the client's `clientInfo.name` (`initialize` handshake); in hook-CLI mode it comes from the `--agent <name>` flag.
3. No usable client name → a single persisted `default` UUID.

Keying on the client name (not one fixed file) is what puts each agent in its own folder automatically while staying stable across restarts. An agent passes the same name on `--agent` that it reports as its `clientInfo.name` (e.g. `claude-code`), so its tool-writes and its hook-uploads share one `agent_id`. The token and `agent_id` are never exposed to the model.

## Tools

Each tool forwards to the API with the resolved `agent_id` and a `Authorization: Bearer <token>` header.

| Tool            | Input                           | Request                    | Result                                           |
| --------------- | ------------------------------- | -------------------------- | ------------------------------------------------ |
| `search_memory` | `{query, scope?, raw_only?}`    | `POST /v1/memories/search` | hits as `path (score): snippet`, or "no matches" |
| `write_memory`  | `{project?, filename, content}` | `POST /v1/memories/write`  | confirmation                                     |
| `upload_memory` | `{project?, filename, path}`    | `POST /v1/memories/write`  | confirmation                                     |
| `read_memory`   | `{project?, filename}`          | `POST /v1/memories/read`   | content, or "not found"                          |

There are two ways to persist a memory; both store under `{username}/{agent_id}/{project}/{filename}` and are replace-by-(project, filename), so re-using a filename updates that memory.

- **`write_memory`** is the model-authored path: the model composes the `content` and names the memory via `filename`, with an optional `project` bucket (defaults to `_default`). Use this for durable decisions, preferences, and facts the model records as it works.
- **`upload_memory`** stores a file that already exists on disk: it takes an absolute `path`, reads the file, and stores it under the given `filename` and optional `project`. A relative path, or a missing/unreadable file, is an error. The two tools differ only in the content source — inline vs. read-from-disk.
- **`read_memory`** takes the `filename` and the `project` it was saved under (defaults to `_default` when omitted) and returns the stored content.
- **`search_memory`** defaults to the server's `all` scope, so it spans the whole team's memories and summaries; the optional `scope` (`all`/`user`/`agent`) and `raw_only` narrow it. Results show their full path, so the owning user/agent is visible.
- Tool descriptions and the MCP `initialize` instructions are written to drive proactive use (search before a task; save durable notes/decisions/facts).

## Hook-support CLI

Two subcommands the per-agent plugins call; they reuse the MCP tools' identity resolution, HTTP client, and DTOs.

| Subcommand | Input                                                                  | Behavior                                                                                                                          |
| ---------- | ---------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| `upload`   | `--agent <name>` + `--project/--filename/--path`, or a JSON array of `{project, filename, path}` on stdin | Persist memory files — the `upload_memory` operation (shares `do_upload`). The stdin-batch form lets a hook spawn the binary once for several files. |
| `recall`   | `--agent <name> [--scope user\|agent\|global]`                         | Print the latest synthesized summary for the scope (default `user`) for the plugin to inject; prints nothing when none exists.   |

No `write` subcommand: inline `content` is model-only, and the hook layer always works from files on disk. `recall` calls `POST /v1/memories/summary` (see [http-server-design.md](http-server-design.md)).

## Error handling

HTTP/transport failures map to MCP tool errors with actionable messages: 401 → "check `MEMORYHUB_TOKEN`"; connection failure → "cannot reach MemoryHub at `<url>`". A 404 on `read_memory` is a normal not-found result, not an error.

## Out of Scope

Remote (streamable-HTTP) transport (the client core stays transport-agnostic so it can be added later); a destructive `delete_memory` tool; MCP resources/prompts; self-serve token minting. On the CLI side, `scan` (dry-run of memory-file identification) and `upload --root` (bulk walk for hook-less agents) are deferred until an agent needs them.
