# Smoke Test

Manual checklist to verify the plugin works end-to-end in a real Claude Code session.

## Prerequisites
- MemoryHub server running (e.g. `cargo run -- --port 8000` from the repo root, or the compiled binary `memoryhub --port 8000`)
- Plugin installed at `~/.claude/plugins/memoryhub/`

## Steps

- [ ] Run `/mh-config` and enter the server URL and your username. Confirm the config file was written:
  ```
  cat ~/.claude/memoryhub-config.json
  ```

- [ ] Confirm the hook was added to settings:
  ```
  cat ~/.claude/settings.json | python3 -m json.tool | grep -A2 memoryhub
  ```

- [ ] Ask Claude to save a memory (e.g. "remember that I prefer tabs over spaces"). Wait for the Write tool to fire.

- [ ] Confirm the file appeared in MemoryHub:
  ```
  curl -s -X POST http://localhost:8000/v1/memories/read \
    -H "Content-Type: application/json" \
    -d '{"username":"<your-username>","agent_id":"<your-agent-id>","filename":"<memory-file-name>"}'
  ```

- [ ] Run `/mh-push`. Confirm the output shows all memory files pushed with 0 failures.

- [ ] Stop the server and run `/mh-push` again. Confirm it warns but does not crash the session.
