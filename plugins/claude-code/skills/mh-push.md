# mh-push

Push all memory files for the current project to MemoryHub.

Run this command, substituting the actual project directory from your system context:

```bash
python3 ~/.claude/plugins/memoryhub/memoryhub.py push-all --project-dir ~/.claude/projects/<project-dir>
```

`<project-dir>` is the project folder that holds this project's memory files. Use the path already shown in your context (the directory under `~/.claude/projects` containing the `memory` folder); do not construct it by hand.

Show the command output to the user.
