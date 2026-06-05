# MemoryHub plugin for Codex

Bridges Codex's local memories (`~/.codex/memories/`) into MemoryHub and injects the team summary at session start. Read/write tools also come from the `memoryhub-mcp` MCP server.

## Setup

1. Put `memoryhub-mcp` on your PATH.
2. Configure the server once: `memoryhub-mcp config` (writes `<config_dir>/memoryhub/config.toml`).
3. Register the MCP server in `~/.codex/config.toml`:

   ```toml
   [mcp_servers.memoryhub]
   command = "memoryhub-mcp"
   ```
4. Install this plugin so Codex loads `hooks/hooks.json` (see Codex's plugin docs for the install path / command).

The hooks run on `SessionStart`: **capture** uploads `~/.codex/memories/`, **recall** injects your latest team summary, **check-config** nudges you here if setup is missing.
