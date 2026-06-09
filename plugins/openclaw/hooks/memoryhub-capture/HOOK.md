---
name: memoryhub-capture
description: "Upload OpenClaw workspace memory to MemoryHub"
metadata:
  {
    "openclaw":
      {
        "emoji": "⬆️",
        "events": ["command:new", "command:reset"],
        "requires": { "bins": ["memoryhub-mcp"] },
      },
  }
---

# MemoryHub Capture

Uploads the workspace daily notes (`<workspace>/memory/*.md`) to MemoryHub whenever a session is archived (`/new`, `/reset`).
