---
name: memoryhub-recall
description: "Inject the MemoryHub team summary into Project Context at agent bootstrap"
metadata:
  { "openclaw": { "emoji": "⬇️", "events": ["agent:bootstrap"], "requires": { "bins": ["memoryhub-mcp"] } } }
---

# MemoryHub Recall

At `agent:bootstrap`, fetches the latest synthesized team summary
(`memoryhub-mcp recall --agent openclaw`) and appends it to the in-memory `MEMORY.md`
bootstrap entry, so the model sees it without any disk write (and capture never re-uploads it).
