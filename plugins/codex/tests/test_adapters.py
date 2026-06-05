import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "hooks"))
import capture  # noqa: E402


def test_collect_items_globs_markdown_recursively(tmp_path):
    (tmp_path / "MEMORY.md").write_text("durable")
    sub = tmp_path / "rollout_summaries"
    sub.mkdir()
    (sub / "2026-06-04-abc.md").write_text("thread summary")
    (tmp_path / "state.sqlite").write_text("not markdown")

    items = capture.collect_items(tmp_path)
    by_name = {i["filename"]: i for i in items}

    assert set(by_name) == {"MEMORY.md", "rollout_summaries/2026-06-04-abc.md"}
    assert by_name["MEMORY.md"]["path"] == str(tmp_path / "MEMORY.md")
    assert "project" not in by_name["MEMORY.md"]  # omitted -> server _default bucket


def test_collect_items_empty_when_dir_missing(tmp_path):
    assert capture.collect_items(tmp_path / "nope") == []


import recall  # noqa: E402


def test_recall_format_context_wraps_additional_context():
    out = recall.format_context("digest")
    assert out["hookSpecificOutput"]["hookEventName"] == "SessionStart"
    assert out["hookSpecificOutput"]["additionalContext"] == "digest"


def test_recall_format_context_empty_is_none():
    assert recall.format_context("") is None
    assert recall.format_context("   ") is None


import check_config  # noqa: E402


def test_check_config_emit_shapes_system_message(capsys):
    check_config.emit("hello")
    out = capsys.readouterr().out
    import json as _json
    assert _json.loads(out) == {"systemMessage": "hello"}
