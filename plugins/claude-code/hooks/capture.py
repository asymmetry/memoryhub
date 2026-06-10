#!/usr/bin/env python3

"""PostToolBatch hook: upload memory files edited this batch to memoryhub."""

import json
import subprocess
import sys
from pathlib import Path

PROJECTS_DIR = Path.home() / ".claude" / "projects"
WRITE_TOOLS = {"Write", "Edit", "MultiEdit"}

TIMEOUT = 10  # seconds


def is_memory_path(path: Path) -> bool:
    try:
        rel = path.relative_to(PROJECTS_DIR)
    except ValueError:
        return False
    parts = rel.parts
    return len(parts) >= 3 and parts[1] == "memory" and path.suffix == ".md"


def to_item(path: Path) -> dict:
    rel = path.relative_to(PROJECTS_DIR)
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
        return
    items = collect_items(payload)

    if not items:
        return

    try:
        subprocess.run(
            ["memoryhub-mcp", "upload", "--agent", "claude-code"],
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
