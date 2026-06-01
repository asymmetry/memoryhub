# Claude Code Plugin — Design Spec

Keeps Claude Code memories in sync with MemoryHub. Memory files are pushed automatically on every write; `/mh-push` and `/mh-config` are available as slash commands.

## Layout

```
plugins/claude-code/
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

Stored at `~/.claude/memoryhub.json`:

```json
{
  "url": "http://localhost:8000",
  "username": "alice",
  "agent_id": "550e8400-e29b-41d4-a716-446655440000"
}
```

`agent_id` is generated once on first `/mh-config` run and never changes. MemoryHub namespaces files under `{username}/{agent_id}/{project}/`; the plugin sends `project` (its per-project grouping; omitted → the `_default` bucket).

## Hook

`PostToolBatch` (no matcher — fires once per resolved tool batch), added to `~/.claude/settings.json` by `/mh-config`. Reads the `tool_calls` array from stdin JSON, keeps `Write`/`Edit`/`MultiEdit` calls whose `file_path` is under `~/.claude/projects/*/memory/`, dedups them, and pushes each. Always exits 0 — server outages must not interrupt the session.

## Skills

**`/mh-push`** — runs `memoryhub.py push-all --project-dir <path>` for the current project. Claude passes the project directory from its context. Prints a summary on completion.

**`/mh-config`** — runs `memoryhub.py config`. Prompts for each config field (current values as defaults), writes config, and merges the hook entry into `~/.claude/settings.json` without disturbing existing hooks.

## `memoryhub.py`

All HTTP via stdlib `urllib`. Three subcommands:

**`push`** — reads the `tool_calls` array from stdin JSON; selects `Write`/`Edit`/`MultiEdit` calls with a memory-dir `file_path`, dedups them; reads each file; POSTs to `/v1/memories/write` with `project` set to the file's project grouping (its `{project_hash}` under `~/.claude/projects`) and `filename` set to the memory-relative leaf, so memory files from different projects land in distinct project folders server-side.

**`push-all --project-dir <path>`** — walks the given project dir for memory files, pushes each with the same `project` + `filename` split as `push`, prints summary.

**`config`** — interactive config editor; merges hook into `~/.claude/settings.json`.

## Error Handling

| Situation                     | Behavior                                 |
| ----------------------------- | ---------------------------------------- |
| Config missing                | Print "Run /mh-config first." and exit 0 |
| Network / server error        | Warn and continue to next file           |
| Path doesn't match memory dir | Silent exit 0                            |
| Memory dir not found          | Warn and exit 0                          |
| Malformed stdin JSON          | Silent exit 0                            |

## Testing

- **Unit** (`test_memoryhub.py`): path matching, config I/O, filename derivation, settings.json patching. No network required.
- **Integration** (`test_integration.sh`): live server via `cargo run`, push and verify via read API.
- **Smoke** (`SMOKE_TEST.md`): manual checklist for a real Claude Code session.

## Out of Scope (v1)

- `/mh-search`, `/mh-pull`
- Authentication
- Incremental sync
