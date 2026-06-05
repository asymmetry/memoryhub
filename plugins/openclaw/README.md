# MemoryHub plugin for OpenClaw

Bridges OpenClaw's workspace memory into MemoryHub and injects the team summary at agent bootstrap. Read/write tools come from the `memoryhub-mcp` MCP server.

## Setup

1. Put `memoryhub-mcp` on your PATH; run `memoryhub-mcp config` once.
2. Register the MCP server in `~/.openclaw/openclaw.json`:

   ```json
   { "mcp": { "servers": { "memoryhub": { "command": "memoryhub-mcp" } } } }
   ```

3. Install the hooks: copy `hooks/memoryhub-capture/` and `hooks/memoryhub-recall/` into `~/.openclaw/hooks/`, then:

   ```bash
   openclaw hooks enable memoryhub-capture memoryhub-recall
   ```

**capture** uploads `<workspace>/memory/*.md` on `/new`,`/reset`; **recall** appends your team summary to `MEMORY.md` context at session start. If OpenClaw's MCP `clientInfo.name` isn't `openclaw`, set `MEMORYHUB_AGENT_ID` so model-driven and hook writes share one folder.
