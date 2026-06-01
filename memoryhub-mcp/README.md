# memoryhub-mcp

A stdio [MCP](https://modelcontextprotocol.io) server that exposes your MemoryHub
memories to any MCP-capable coding agent as `search_memory`, `save_memory`, and
`read_memory` tools.

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

`save_memory` takes the absolute path of a file you wrote and stores it under that
absolute path as its filename.

Each agent automatically gets its own memory namespace (a UUID persisted under
`~/.config/memoryhub/agents/<client-name>`). Override it with `MEMORYHUB_AGENT_ID`
if you want to pin or share a namespace explicitly.
