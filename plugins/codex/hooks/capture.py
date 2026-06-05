#!/usr/bin/env python3

"""SessionStart hook: upload Codex's memory files (~/.codex/memories/) via memoryhub-mcp."""

import json
import os
import subprocess
import sys
from pathlib import Path


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
        # project omitted -> server's _default bucket; filename is the memories-relative path.
        items.append({"filename": rel.as_posix(), "path": str(path)})
    return items


def main() -> None:
    items = collect_items(memories_dir())
    if not items:
        sys.exit(0)
    try:
        subprocess.run(
            ["memoryhub-mcp", "upload", "--agent", "codex"],
            input=json.dumps(items),
            text=True,
            check=False,
        )
    except FileNotFoundError:
        print("[memoryhub] memoryhub-mcp not found on PATH", file=sys.stderr)
    sys.exit(0)


if __name__ == "__main__":
    main()
