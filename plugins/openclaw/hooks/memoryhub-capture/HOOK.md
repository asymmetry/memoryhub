---
name: memoryhub-capture
description: "Upload OpenClaw workspace memory to MemoryHub on /new and /reset"
metadata:
  { "openclaw": { "emoji": "⬆️", "events": ["command:new", "command:reset"], "requires": { "bins": ["memoryhub-mcp"] } } }
---

# MemoryHub Capture

Uploads the workspace daily notes (`<workspace>/memory/*.md`) to MemoryHub via
`memoryhub-mcp upload --agent openclaw` whenever a session is archived (`/new`, `/reset`).
