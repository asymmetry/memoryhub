#!/usr/bin/env python3

import argparse
import json
import os
import sys
import uuid
from pathlib import Path
from urllib import error, request

CONFIG_PATH = Path.home() / ".claude" / "memoryhub-config.json"
SETTINGS_PATH = Path.home() / ".claude" / "settings.json"
HOOK_COMMAND = "python3 ~/.claude/plugins/memoryhub/memoryhub.py push-file"


def load_config(path=None):
    if path is None:
        path = Path(os.environ.get("MEMORYHUB_CONFIG_PATH", str(CONFIG_PATH)))
    if not Path(path).exists():
        print("MemoryHub not configured. Run /mh-config first.")
        sys.exit(0)
    with open(path) as f:
        return json.load(f)


def save_config(config, path=None):
    if path is None:
        path = CONFIG_PATH
    with open(path, "w") as f:
        json.dump(config, f, indent=2)


def inject_hook(settings_path=None):
    if settings_path is None:
        settings_path = SETTINGS_PATH
    settings = {}
    if Path(settings_path).exists():
        try:
            with open(settings_path) as f:
                settings = json.load(f)
        except json.JSONDecodeError:
            print(f"settings.json is malformed. Fix it first: {settings_path}")
            sys.exit(1)
    hooks = settings.setdefault("hooks", {})
    post = hooks.setdefault("PostToolUse", [])
    # Check if already present
    for entry in post:
        if entry.get("matcher") == "Write":
            for h in entry.get("hooks", []):
                if h.get("command") == HOOK_COMMAND:
                    return
    # Find or create Write entry
    write_entry = next((e for e in post if e.get("matcher") == "Write"), None)
    if write_entry is None:
        write_entry = {"matcher": "Write", "hooks": []}
        post.append(write_entry)
    write_entry["hooks"].append({"type": "command", "command": HOOK_COMMAND})
    with open(settings_path, "w") as f:
        json.dump(settings, f, indent=2)


def is_memory_path(path, projects_dir=None):
    if projects_dir is None:
        projects_dir = Path.home() / ".claude" / "projects"
    try:
        rel = Path(path).relative_to(projects_dir)
    except ValueError:
        return False
    parts = rel.parts
    return len(parts) >= 3 and parts[1] == "memory" and Path(path).suffix == ".md"


def get_filename(path, projects_dir=None):
    if projects_dir is None:
        projects_dir = Path.home() / ".claude" / "projects"
    rel = Path(path).relative_to(projects_dir)
    parts = rel.parts
    # parts[0]=project_hash, parts[1]="memory", rest=filename
    return str(Path(*parts[2:]))


def push_single_file(config, file_path, filename=None):
    if filename is None:
        filename = get_filename(file_path)
    try:
        with open(file_path, errors="replace") as f:
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


def cmd_push_file():
    try:
        data = json.load(sys.stdin)
        file_path = data.get("tool_input", {}).get("file_path", "")
    except (json.JSONDecodeError, ValueError):
        sys.exit(0)
    if not file_path or not is_memory_path(file_path):
        sys.exit(0)
    config = load_config()
    ok, err = push_single_file(config, file_path)
    if not ok:
        print(f"[memoryhub] Warning: {err}", file=sys.stderr)


def cmd_push_all(memory_dir):
    config = load_config()
    p = Path(memory_dir)
    if not p.exists():
        print(f"No memory directory found at {memory_dir}")
        sys.exit(0)
    files = sorted(p.rglob("*.md"))
    if not files:
        print("No memory files found.")
        return
    ok_count = fail_count = 0
    for f in files:
        filename = str(f.relative_to(p))
        success, err = push_single_file(config, f, filename=filename)
        if success:
            ok_count += 1
            print(f"  ok {f.name}")
        else:
            fail_count += 1
            print(f"  fail {f.name}: {err}")
    print(f"\nPushed {ok_count} file(s) to MemoryHub ({fail_count} failed).")


def cmd_config():
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
    sub.add_parser("push-file")
    pa = sub.add_parser("push-all")
    pa.add_argument("--memory-dir", required=True)
    sub.add_parser("config")

    args = parser.parse_args()

    if args.command == "push-file":
        cmd_push_file()
    elif args.command == "push-all":
        cmd_push_all(args.memory_dir)
    elif args.command == "config":
        cmd_config()
    else:
        parser.print_help()


if __name__ == "__main__":
    main()
