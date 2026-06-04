# memoryhub-mcp

A stdio [MCP](https://modelcontextprotocol.io) server that exposes your MemoryHub memories to any MCP-capable coding agent as `search_memory`, `write_memory`, `upload_memory`, and `read_memory` tools.

## Configure

Point your agent at the binary and set two env vars:

```json
{
  "command": "memoryhub-mcp",
  "env": {
    "MEMORYHUB_URL": "https://your-memoryhub.example.com",
    "MEMORYHUB_TOKEN": "mh_your_token"
  }
}
```

`write_memory` saves a memory you compose inline (content + filename); `upload_memory` reads a file from an absolute path and stores it under the filename you give. Both take an optional `project` bucket and update in place when the filename is re-used.

Each agent automatically gets its own memory namespace (a UUID persisted under `~/.config/memoryhub/agents/<client-name>`). Override it with `MEMORYHUB_AGENT_ID` if you want to pin or share a namespace explicitly.
