import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "hooks"))
import capture  # noqa: E402


def test_is_memory_path():
    base = Path.home() / ".claude" / "projects"
    assert capture.is_memory_path(base / "hash1" / "memory" / "a.md")
    assert not capture.is_memory_path(base / "hash1" / "src" / "a.md")
    assert not capture.is_memory_path(base / "hash1" / "memory" / "a.txt")


def test_to_item_maps_project_and_filename():
    base = Path.home() / ".claude" / "projects"
    p = base / "hash1" / "memory" / "notes" / "a.md"
    item = capture.to_item(p)
    assert item == {"project": "hash1", "filename": "memory/notes/a.md", "path": str(p)}


def test_collect_items_dedups_and_filters():
    base = Path.home() / ".claude" / "projects"
    mem = str(base / "h" / "memory" / "a.md")
    payload = {"tool_calls": [
        {"tool_name": "Write", "tool_input": {"file_path": mem}},
        {"tool_name": "Edit", "tool_input": {"file_path": mem}},          # dup
        {"tool_name": "Write", "tool_input": {"file_path": "/tmp/x.md"}},  # not memory
        {"tool_name": "Read", "tool_input": {"file_path": mem}},          # not a write tool
    ]}
    items = capture.collect_items(payload)
    assert items == [{"project": "h", "filename": "memory/a.md", "path": mem}]
