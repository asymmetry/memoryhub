#!/usr/bin/env python3

"""SessionStart hook: recall the latest synthesized summary from memoryhub."""

import json
import subprocess
import sys


def format_context(summary: str) -> dict | None:
    summary = summary.strip()
    if not summary:
        return None
    return {
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": summary,
        }
    }


def main() -> None:
    try:
        result = subprocess.run(
            ["memoryhub-mcp", "recall", "--agent", "codex", "--scope", "global"],
            capture_output=True,
            text=True,
            check=False,
        )
        summary = result.stdout
    except FileNotFoundError:
        return

    out = format_context(summary)
    if out:
        print(json.dumps(out))


if __name__ == "__main__":
    main()
    sys.exit(0)
