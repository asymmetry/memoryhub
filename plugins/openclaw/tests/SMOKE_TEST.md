# Smoke Test — OpenClaw plugin

Manual checklist (requires Node.js + an OpenClaw install).

## Prerequisites

- `memoryhub-mcp` on PATH; `memoryhub-mcp config` done; a MemoryHub server running.
- OpenClaw installed; this plugin's hooks copied into `~/.openclaw/hooks/` and enabled.

## Steps

- [ ] Unit: `cd plugins/openclaw && npm install && npm test` → `collectItems` tests pass.
- [ ] Register the MCP server in `~/.openclaw/openclaw.json` (see README); confirm the model can call `search_memory`.
- [ ] Capture: in an OpenClaw session, write something, then `/new` or `/reset`. Confirm the workspace daily note appears in MemoryHub:
  ```
  curl -s -X POST "$URL/v1/memories/read" -H "Authorization: Bearer <token>" \
    -H "Content-Type: application/json" -d '{"agent_id":"<id>","filename":"<YYYY-MM-DD>.md"}'
  ```
- [ ] Recall: start a new session; confirm the `MEMORY.md` context includes a "## MemoryHub team summary" section (once a synthesis exists).
- [ ] Resilience: stop the server; confirm sessions still start (recall injects nothing, capture fails silently).
