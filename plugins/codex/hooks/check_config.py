#!/usr/bin/env python3

"""SessionStart hook: nudge the user to run `memoryhub-mcp config` when unconfigured."""

import json
import subprocess
import sys

NOT_CONFIGURED = (
    "MemoryHub isn't configured — run `memoryhub-mcp config` in your terminal to set the "
    "server URL and token. Memories won't sync until then."
)
NOT_INSTALLED = (
    "MemoryHub: `memoryhub-mcp` isn't on your PATH. Install it, then run "
    "`memoryhub-mcp config` to set the server URL and token."
)


def emit(message: str) -> None:
    print(json.dumps({"systemMessage": message}))


def main() -> None:
    try:
        result = subprocess.run(
            ["memoryhub-mcp", "config", "--check"],
            capture_output=True,
            check=False,
        )
    except FileNotFoundError:
        emit(NOT_INSTALLED)
        sys.exit(0)
    if result.returncode != 0:
        emit(NOT_CONFIGURED)
    sys.exit(0)


if __name__ == "__main__":
    main()
