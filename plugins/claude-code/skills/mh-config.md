# mh-config

Configure MemoryHub for Claude Code. Run:

```bash
memoryhub-mcp config
```

This prompts for the server URL and API token and writes them to
`<config_dir>/memoryhub/config.json`, which the capture/recall hooks read. The
hooks are declared by the plugin manifest — no `settings.json` editing needed.
