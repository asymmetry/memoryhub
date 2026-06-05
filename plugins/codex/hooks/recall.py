#!/usr/bin/env python3

"""SessionStart hook: inject the latest synthesized summary via memoryhub-mcp recall."""

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
            ["memoryhub-mcp", "recall", "--agent", "codex", "--scope", "user"],
            capture_output=True,
            text=True,
            check=False,
        )
        summary = result.stdout
    except FileNotFoundError:
        sys.exit(0)
    out = format_context(summary)
    if out:
        print(json.dumps(out))
    sys.exit(0)


if __name__ == "__main__":
    main()
