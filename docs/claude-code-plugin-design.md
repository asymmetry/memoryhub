# Claude Code Plugin — Design Spec

Keeps Claude Code memories in sync with MemoryHub. Memory files are pushed automatically on every write; `/mh-push` and `/mh-config` are available as slash commands.

## Layout

```
agents/claude-code/
  plugin.json
  memoryhub.py
  skills/
    mh-push.md
    mh-config.md
  tests/
    test_memoryhub.py
    test_integration.sh
    SMOKE_TEST.md
```

## Config

Stored at `~/.claude/memoryhub-config.json`:

```json
{
  "url": "http://localhost:8000",
  "username": "alice",
  "agent_id": "550e8400-e29b-41d4-a716-446655440000"
}
```

`agent_id` is generated once on first `/mh-config` run and never changes. MemoryHub namespaces files under `{username}/{agent_id}/`.

## Hook

`PostToolUse` on `Write`, added to `~/.claude/settings.json` by `/mh-config`. Reads `file_path` from stdin JSON, silently exits if the path is not under `~/.claude/projects/*/memory/`. Always exits 0 — server outages must not interrupt the session.

## Skills

**`/mh-push`** — runs `memoryhub.py push-all --memory-dir <path>`. Claude passes the memory directory path from its context. Prints a summary on completion.

**`/mh-config`** — runs `memoryhub.py config`. Prompts for each config field (current values as defaults), writes config, and merges the hook entry into `~/.claude/settings.json` without disturbing existing hooks.

## `memoryhub.py`

All HTTP via stdlib `urllib`. Three subcommands:

**`push-file`** — reads `file_path` from stdin JSON; checks memory dir pattern; reads file; POSTs to `/v1/memories/write`.

**`push-all --memory-dir <path>`** — walks the directory, calls push logic per file, prints summary.

**`config`** — interactive config editor; merges hook into `~/.claude/settings.json`.

## Error Handling

| Situation | Behavior |
|---|---|
| Config missing | Print "Run /mh-config first." and exit 0 |
| Network / server error | Warn and continue to next file |
| Path doesn't match memory dir | Silent exit 0 |
| Memory dir not found | Warn and exit 0 |
| Malformed stdin JSON | Silent exit 0 |

## Testing

- **Unit** (`test_memoryhub.py`): path matching, config I/O, filename derivation, settings.json patching. No network required.
- **Integration** (`test_integration.sh`): live server via `cargo run`, push and verify via read API.
- **Smoke** (`SMOKE_TEST.md`): manual checklist for a real Claude Code session.

## Out of Scope (v1)

- `/mh-search`, `/mh-pull`
- Authentication
- Incremental sync
