# mh-push

Push all memory files for the current project to MemoryHub.

Run this command, substituting the actual memory directory path from your system context:

```bash
python3 ~/.claude/plugins/memoryhub/memoryhub.py push-all --memory-dir ~/.claude/projects/<project-dir>/memory
```

The project directory name is the current working directory path with `/` replaced by `-` (e.g. `/home/alice/project` → `-home-alice-project`).

Show the command output to the user.
