---
name: memoryhub-recall
description: "Inject the memory into Project Context at agent bootstrap"
metadata:
  {
    "openclaw":
      {
        "emoji": "⬇️",
        "events": ["agent:bootstrap"],
        "requires": { "bins": ["memoryhub-mcp"] },
      },
  }
---

# MemoryHub Recall

At `agent:bootstrap`, fetches the latest synthesized memory and appends it to the in-memory `MEMORY.md` bootstrap entry.
