#!/usr/bin/env python3

"""SessionStart hook: recall the latest synthesized summary from memoryhub."""

import json
import subprocess
import sys

TIMEOUT = 10  # seconds


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
            timeout=TIMEOUT,
        )
    except (FileNotFoundError, subprocess.TimeoutExpired):
        return

    # Fail open: on a non-zero exit (misconfig, auth rejected, partial output) inject nothing
    # rather than feeding diagnostic/error text on stdout into the agent's context.
    if result.returncode != 0:
        return
    summary = result.stdout

    out = format_context(summary)
    if out:
        print(json.dumps(out))


if __name__ == "__main__":
    main()
    sys.exit(0)
