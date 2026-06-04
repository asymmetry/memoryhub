# MemoryHub — Claude Code plugin

Syncs Claude Code's memory files to a MemoryHub server and recalls your summary at the
start of each session.

## Setup

1. Install the `memoryhub-mcp` binary and make sure it's on your `PATH`.
2. Run it once to set the server URL and token:

   ```bash
   memoryhub-mcp config
   ```

That's it. The plugin's hooks then upload memory files as you write them and inject your
summary when a session starts. Until you run `config`, Claude Code reminds you at session
start.
