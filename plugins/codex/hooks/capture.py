#!/usr/bin/env python3

"""Stop hook: upload Codex's memory files to memoryhub."""

import json
import os
import subprocess
import sys
from pathlib import Path

TIMEOUT = 10  # seconds


def memories_dir() -> Path:
    home = os.environ.get("CODEX_HOME")
    base = Path(home) if home else Path.home() / ".codex"
    return base / "memories"


def collect_items(root: Path) -> list[dict]:
    items = []
    for path in sorted(root.rglob("*.md")):
        if not path.is_file():
            continue
        rel = path.relative_to(root)
        # Skip the consolidated MEMORY.md: MemoryHub re-synthesizes from raw
        # entries, so uploading Codex's own consolidation would double-synthesize.
        if rel.as_posix() == "MEMORY.md":
            continue
        items.append({"filename": rel.as_posix(), "path": str(path)})
    return items


def main() -> None:
    items = collect_items(memories_dir())
    if not items:
        return

    try:
        subprocess.run(
            ["memoryhub-mcp", "upload", "--agent", "codex"],
            input=json.dumps(items),
            text=True,
            check=False,
            timeout=TIMEOUT,
        )
    except FileNotFoundError:
        print("[memoryhub] memoryhub-mcp not found on PATH", file=sys.stderr)
    except subprocess.TimeoutExpired:
        print("[memoryhub] upload timed out", file=sys.stderr)


if __name__ == "__main__":
    main()
    sys.exit(0)
