# Claude Code Plugin — Design Spec

Keeps Claude Code memories in sync with MemoryHub **deterministically**, complementing the
model-driven MCP tools. The model uses the `memoryhub-mcp` tools (`search_memory` /
`write_memory` / `upload_memory` / `read_memory`) when it chooses to; the plugin's hooks run
regardless, so some memory behavior is guaranteed:

|           | Model-driven (MCP tools)         | Deterministic (this plugin)                                      |
| --------- | -------------------------------- | ---------------------------------------------------------------- |
| **Write** | `write_memory` / `upload_memory` | **capture** hook — auto-upload memory files the model writes     |
| **Read**  | `search_memory` / `read_memory`  | **recall** hook — auto-inject a memory baseline at session start |

## Thin adapter over the binary

The plugin is a per-agent adapter, not a memory client: identity, transport, and the memory
operations live in the `memoryhub-mcp` binary (see [mcp-server-design.md](mcp-server-design.md)).
The plugin owns only what is Claude-Code-specific:

1. **Memory layout** — which files are memory and how a path maps to `(project, filename)`.
2. **Trigger glue** — small hook adapters that read Claude Code's event payloads and call the
   binary's `upload` / `recall`.

## Layout

```
plugins/claude-code/
  plugin.json        # hook declarations
  README.md          # setup instructions
  hooks/
    capture.py       # PostToolBatch -> memoryhub-mcp upload
    recall.py        # SessionStart  -> memoryhub-mcp recall
    check_config.py  # SessionStart  -> memoryhub-mcp config --check (setup nudge)
  tests/
```

Adapters are Python (stdlib only): event parsing, layout mapping, hook-output shaping.

## Memory layout (Claude Code)

Memory files are `~/.claude/projects/*/memory/**/*.md`. A file
`~/.claude/projects/<hash>/memory/<rest>.md` maps to:

- `project` = `<hash>` (keeps different projects' memory distinct server-side)
- `filename` = `memory/<rest>.md` (the server flattens `/` to `_`)

## Hooks

Declared in `plugin.json` (no `settings.json` editing). All exit 0 — a server outage
must never interrupt a session.

- **capture** (`PostToolBatch`) — from the stdin `tool_calls`, keep `Write`/`Edit`/`MultiEdit`
  calls touching a memory file, dedup, map each to `(project, filename)`, and pipe a JSON array
  to `memoryhub-mcp upload --agent claude-code`.
- **recall** (`SessionStart`) — run `memoryhub-mcp recall --agent claude-code` and emit its
  output as `additionalContext`; nothing on empty summary or failure.
- **check-config** (`SessionStart`, matcher `startup|resume`) — run `memoryhub-mcp config
--check`; on a non-zero exit (or a missing binary) emit a user-visible `systemMessage`
  pointing at `memoryhub-mcp config`. Silent once configured.

## Identity & config

`--agent claude-code` resolves to the same `agent_id` the MCP server uses for Claude Code, so the
model's tool-writes and the hooks' uploads share one folder (assumes Claude Code's
`clientInfo.name` slugs to `claude-code`; `MEMORYHUB_AGENT_ID` overrides). Connection config
(`url`, `token`) is read from `<config_dir>/memoryhub/config.toml`.

## Setup

Run `memoryhub-mcp config` once in a terminal to write `url` + `token` to
`<config_dir>/memoryhub/config.toml`. Setup is interactive, so it is the user's step, not the
agent's — there is no `/mh-config` skill (the agent must never drive the interactive command);
the **check-config** hook reminds the user via `systemMessage` until it is done. The `agent_id`
file is created lazily on the first `upload`/`recall`. Hooks are declared by the manifest.

## Testing

- **Unit**: `PostToolBatch` parsing and path → `(project, filename)` mapping; `recall`/
  `check-config` output shaping (no network).
- **Integration**: live server; a payload through `capture.py` → binary → server, verified via
  read; `recall.py` after a synthesis.

## Out of Scope

Bulk / no-hook sync (`scan`, `upload --root`) lives in the binary, deferred. Plugins for other
agents — this is the first instance of the engine + adapter pattern.
