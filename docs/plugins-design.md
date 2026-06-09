# Agent Plugins — Design Spec

Plugins keep a coding agent's memory in sync with MemoryHub **deterministically**, complementing the model-driven MCP tools (`search_memory` / `write_memory` / `upload_memory` / `read_memory`):

|           | Model-driven (MCP tools)         | Deterministic (plugin hooks)                               |
| --------- | -------------------------------- | ---------------------------------------------------------- |
| **Write** | `write_memory` / `upload_memory` | **capture** — auto-upload the agent's memory files         |
| **Read**  | `search_memory` / `read_memory`  | **recall** — auto-inject the team summary at session start |

## Architecture

Identity, transport, and the memory operations live once in the `memoryhub-mcp` binary — the agent-agnostic engine (see [mcp-server-design.md](mcp-server-design.md)), which exposes `upload` / `recall` / `config` and never parses a raw agent payload. Each plugin is a thin per-agent adapter owning only the agent's **memory layout** (which files are memory; how a path maps to `(project, filename)`) and the **trigger glue** that calls `upload` / `recall`. It runs in the agent's hook runtime — a `command` script that shells to the binary, or an in-process handler that spawns it / calls the HTTP API.

`--agent <name>` (matching the agent's MCP `clientInfo.name`) ties model-driven and hook-driven writes to one `agent_id`; `MEMORYHUB_AGENT_ID` overrides. Connection config (`url`, `token`) comes from env, then `<config_dir>/memoryhub/config.toml`, written once by `memoryhub-mcp config`.

Where an agent self-synthesizes its own memory (Codex, OpenClaw), `capture` bridges its **raw** entries into MemoryHub's pool — its consolidated summaries are optional, to avoid double-synthesis. Where an agent lacks a hook for a layer, that layer stays model-driven.

## Claude Code

Python `command` hooks in `plugin.json`. Memory: `~/.claude/projects/<hash>/memory/<rest>.md` → `project = <hash>`, `filename = memory/<rest>.md`.

- **capture** (`PostToolBatch`): upload `Write`/`Edit`/`MultiEdit`-touched memory files.
- **recall** (`SessionStart`): inject `memoryhub-mcp recall` output as `additionalContext`.
- **check-config** (`SessionStart`): `config --check` → `systemMessage` nudge when unconfigured.
- **identity:** `--agent claude-code`.

## Codex

Hooks mirror Claude Code (`PostToolUse`, `SessionStart` with `additionalContext`) — Python `command` hooks via `plugin.json` → `hooks.json`. MCP via `[mcp_servers.<id>]` in `config.toml`. Self-synthesizing native memory at `~/.codex/memories/` (user-global): `rollout_summaries/` (raw per-thread) + `MEMORY.md` (consolidated).

- **capture** (`Stop`): upload the raw `rollout_summaries/` entries under `~/.codex/memories/` (fixed `project` bucket); the consolidated `MEMORY.md` is skipped, since MemoryHub re-synthesizes from raw entries and uploading Codex's own consolidation would double-synthesize. `Stop` (turn end), not `SessionStart`: Codex flushes its self-synthesized memory at turn/thread end, so capturing at session start would upload stale memory and miss the final turn.
- **recall** (`SessionStart`): inject `memoryhub-mcp recall --agent codex` as `additionalContext`.
- **check-config** (`SessionStart`): `config --check` → `systemMessage` nudge when unconfigured.
- **identity:** `--agent codex`.

## OpenClaw

In-process **TypeScript** hooks (`handler.ts`), so the adapter spawns the binary or calls the HTTP API. MCP via `~/.openclaw/openclaw.json` (`mcp.servers`). Self-synthesizing native memory under `~/.openclaw/workspace/`: `MEMORY.md` + `memory/YYYY-MM-DD.md` daily notes (auto-loaded each session).

- **capture:** a `handler.ts` on the session-archive lifecycle (`command:new` / `command:reset`) uploads the daily notes via `memoryhub-mcp upload --agent openclaw`. Not on `session:compact:after`: that event's context carries only compaction stats, no `workspaceDir`, so the handler can't locate `<workspace>/memory/` — memory written in a session that is compacted (rather than ended via `/new` or `/reset`) is captured at the next archive.
- **recall:** a `handler.ts` on `agent:bootstrap` injects the team summary **in memory** by mutating `context.bootstrapFiles` (appends to the `MEMORY.md` bootstrap entry, or pushes a new one) — the model sees it in Project Context with **no disk write**, so capture never re-uploads it.
- **identity:** `--agent openclaw`.

## To verify

Codex's and OpenClaw's MCP `clientInfo.name` (Claude Code's CLI is confirmed `claude-code`; Codex's VS Code extension reports `codex_vscode`, so the CLI's value needs a runtime check) — set `MEMORYHUB_AGENT_ID` if it differs. Bulk / no-hook sync (`scan`, `upload --root`) stays deferred in the binary.
