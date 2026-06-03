# Smoke Test

Manual checklist to verify the plugin works end-to-end in a real Claude Code session.

## Prerequisites
- MemoryHub server running (e.g. `cargo run -- --port 8000` from the repo root, or the compiled binary `memoryhub --port 8000`)
- `memoryhub-mcp` binary on your PATH (built from `memoryhub-mcp/Cargo.toml`)
- Plugin installed at `~/.claude/plugins/memoryhub/`

## Steps

- [ ] Run `/mh-config` (which invokes `memoryhub-mcp config` interactively) to write your server URL and API token (`mh_...`, minted by an admin via `POST /v1/admin/users/<username>/tokens`) to `<config_dir>/memoryhub/config.json`. Confirm the config file was written:
  ```
  cat "$(python3 -c 'import platformdirs; print(platformdirs.user_config_dir())')/memoryhub/config.json"
  ```

- [ ] Confirm the hooks are declared in the plugin manifest:
  ```
  cat ~/.claude/plugins/memoryhub/plugin.json | python3 -m json.tool | grep -A5 hooks
  ```

- [ ] Ask Claude to write or edit a file under `~/.claude/projects/*/memory/*.md` (e.g. "save a memory: I prefer tabs over spaces"). Wait for the Write tool to fire. The **capture** hook runs automatically after the batch and uploads the file via `memoryhub-mcp upload --agent claude-code`.

- [ ] Confirm the file appeared in MemoryHub:
  ```
  curl -s -X POST http://localhost:8000/v1/memories/read \
    -H "Authorization: Bearer <your-token>" \
    -H "Content-Type: application/json" \
    -d '{"agent_id":"<your-agent-id>","project":"<project-hash>","filename":"memory/<file>.md"}'
  ```

- [ ] Start a new Claude Code session. The **recall** hook runs automatically at SessionStart via `memoryhub-mcp recall --agent claude-code --scope user` and injects your latest synthesized summary as `additionalContext` (only once synthesis has run; the hook exits cleanly if no summary exists yet).

- [ ] Stop the server and start another session. Confirm the recall hook exits cleanly without crashing the session.
