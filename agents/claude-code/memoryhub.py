#!/usr/bin/env python3

import argparse
import json
import sys
import uuid
from pathlib import Path
from urllib import error, request

CONFIG_PATH = Path.home() / ".claude" / "memoryhub.json"
SETTINGS_PATH = Path.home() / ".claude" / "settings.json"
PROJECTS_DIR = Path.home() / ".claude" / "projects"
HOOK_COMMAND = "python3 ~/.claude/plugins/memoryhub/memoryhub.py push"
HOOK_EVENT = "PostToolBatch"
WRITE_TOOLS = {"Write", "Edit", "MultiEdit"}


def load_config(path: Path | None = None) -> dict:
    if path is None:
        path = CONFIG_PATH
    if not path.exists():
        print("MemoryHub not configured. Run /mh-config first.")
        sys.exit(0)
    with open(path) as f:
        return json.load(f)


def save_config(config: dict, path: Path | None = None) -> None:
    if path is None:
        path = CONFIG_PATH
    with open(path, "w") as f:
        json.dump(config, f, indent=2)


def inject_hook(path: Path | None = None) -> None:
    if path is None:
        path = SETTINGS_PATH
    settings = {}
    if path.exists():
        try:
            with open(path) as f:
                settings = json.load(f)
        except json.JSONDecodeError:
            print(f"settings.json is malformed. Fix it first: {path}")
            sys.exit(1)

    hooks = settings.setdefault("hooks", {})
    batch = hooks.setdefault(HOOK_EVENT, [])
    # PostToolBatch has no matcher; bail out if our command is already present
    for entry in batch:
        for h in entry.get("hooks", []):
            if h.get("command") == HOOK_COMMAND:
                return

    if not batch:
        batch.append({"hooks": []})
    batch[0]["hooks"].append({"type": "command", "command": HOOK_COMMAND})
    with open(path, "w") as f:
        json.dump(settings, f, indent=2)


def is_memory_path(path: Path) -> bool:
    try:
        rel = path.relative_to(PROJECTS_DIR)
    except ValueError:
        return False
    parts = rel.parts
    return len(parts) >= 3 and parts[1] == "memory" and path.suffix == ".md"


def get_filename(path: Path) -> str:
    # Full path relative to ~/.claude/projects ({project_hash}/memory/...),
    # so memory files from different projects stay distinct on the server.
    return str(path.relative_to(PROJECTS_DIR))


def push_one(config: dict, path: Path, filename: str | None = None) -> tuple[bool, str | None]:
    if filename is None:
        filename = get_filename(path)

    try:
        with open(path, errors="replace") as f:
            content = f.read()
    except OSError as e:
        return False, f"Read error: {e}"

    payload = json.dumps(
        {
            "username": config["username"],
            "agent_id": config["agent_id"],
            "filename": filename,
            "content": content,
        }
    ).encode()

    url = config["url"].rstrip("/") + "/v1/memories/write"
    req = request.Request(url, data=payload, headers={"Content-Type": "application/json"})
    try:
        with request.urlopen(req):
            return True, None
    except error.HTTPError as e:
        return False, f"HTTP {e.code}: {e.read().decode()[:200]}"
    except error.URLError as e:
        return False, f"Network error: {e.reason}"


def cmd_push() -> None:
    try:
        data = json.load(sys.stdin)
        tool_calls = data.get("tool_calls", [])
    except (json.JSONDecodeError, ValueError):
        sys.exit(0)

    # Collect unique memory files written/edited in this batch, preserving order
    seen = set()
    paths = []
    for call in tool_calls:
        if call.get("tool_name") not in WRITE_TOOLS:
            continue
        file_path = call.get("tool_input", {}).get("file_path", "")
        if not file_path:
            continue
        path = Path(file_path)
        if path in seen or not is_memory_path(path):
            continue
        seen.add(path)
        paths.append(path)

    if not paths:
        sys.exit(0)

    config = load_config()

    for path in paths:
        ok, err = push_one(config, path)
        if not ok:
            print(f"[memoryhub] Warning: {err}", file=sys.stderr)


def cmd_push_all(project_dir: Path | str) -> None:
    path = Path(project_dir)
    if not path.exists():
        print(f"No project directory found at {path}")
        sys.exit(0)

    files = sorted(f for f in path.rglob("*.md") if is_memory_path(f))
    if not files:
        print("No memory files found.")
        return

    config = load_config()

    ok_count = fail_count = 0
    for f in files:
        success, err = push_one(config, f)
        if success:
            ok_count += 1
            print(f"  ok {get_filename(f)}")
        else:
            fail_count += 1
            print(f"  fail {get_filename(f)}: {err}")
    print(f"\nPushed {ok_count} file(s) to MemoryHub ({fail_count} failed).")


def cmd_config() -> None:
    config = {}
    if CONFIG_PATH.exists():
        with open(CONFIG_PATH) as f:
            config = json.load(f)

    url = input(f"MemoryHub URL [{config.get('url', 'http://localhost:8000')}]: ").strip()
    config["url"] = url or config.get("url", "http://localhost:8000")

    username = input(f"Username [{config.get('username', '')}]: ").strip()
    config["username"] = username or config.get("username", "")

    if not config.get("agent_id"):
        config["agent_id"] = str(uuid.uuid4())

    print(f"Agent ID: {config['agent_id']}")
    save_config(config)

    print(f"Config saved to {CONFIG_PATH}")
    inject_hook()

    print(f"Hook installed in {SETTINGS_PATH}")


def main():
    parser = argparse.ArgumentParser(prog="memoryhub")
    sub = parser.add_subparsers(dest="command")
    sub.add_parser("push")
    pa = sub.add_parser("push-all")
    pa.add_argument("--project-dir", required=True)
    sub.add_parser("config")

    args = parser.parse_args()

    if args.command == "push":
        cmd_push()
    elif args.command == "push-all":
        cmd_push_all(args.project_dir)
    elif args.command == "config":
        cmd_config()
    else:
        parser.print_help()


if __name__ == "__main__":
    main()
