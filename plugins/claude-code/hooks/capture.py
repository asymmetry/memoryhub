#!/usr/bin/env python3

"""PostToolBatch hook: upload memory files written this batch via memoryhub-mcp."""

import json
import subprocess
import sys
from pathlib import Path

PROJECTS_DIR = Path.home() / ".claude" / "projects"
WRITE_TOOLS = {"Write", "Edit", "MultiEdit"}


def is_memory_path(path: Path) -> bool:
    try:
        rel = path.relative_to(PROJECTS_DIR)
    except ValueError:
        return False
    parts = rel.parts
    return len(parts) >= 3 and parts[1] == "memory" and path.suffix == ".md"


def to_item(path: Path) -> dict:
    rel = path.relative_to(PROJECTS_DIR)
    # rel = <hash>/memory/<...>.md
    return {
        "project": rel.parts[0],
        "filename": str(Path(*rel.parts[1:])),
        "path": str(path),
    }


def collect_items(payload: dict) -> list[dict]:
    seen, items = set(), []
    for call in payload.get("tool_calls", []):
        if call.get("tool_name") not in WRITE_TOOLS:
            continue
        fp = call.get("tool_input", {}).get("file_path", "")
        if not fp:
            continue
        path = Path(fp)
        if path in seen or not is_memory_path(path):
            continue
        seen.add(path)
        items.append(to_item(path))
    return items


def main() -> None:
    try:
        payload = json.load(sys.stdin)
    except (json.JSONDecodeError, ValueError):
        sys.exit(0)
    items = collect_items(payload)
    if not items:
        sys.exit(0)
    try:
        subprocess.run(
            ["memoryhub-mcp", "upload", "--agent", "claude-code"],
            input=json.dumps(items),
            text=True,
            check=False,
        )
    except FileNotFoundError:
        print("[memoryhub] memoryhub-mcp not found on PATH", file=sys.stderr)
    sys.exit(0)


if __name__ == "__main__":
    main()
