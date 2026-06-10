import sys
import json
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "hooks"))

import capture  # noqa: E402  # type: ignore
import check_config  # noqa: E402  # type: ignore
import recall  # noqa: E402  # type: ignore


def test_collect_items_globs_raw_entries_and_skips_consolidated_memory(tmp_path):
    (tmp_path / "MEMORY.md").write_text("durable")  # consolidated -> skipped
    sub = tmp_path / "rollout_summaries"
    sub.mkdir()
    (sub / "2026-06-04-abc.md").write_text("thread summary")
    (tmp_path / "state.sqlite").write_text("not markdown")

    items = capture.collect_items(tmp_path)
    by_name = {i["filename"]: i for i in items}

    # MEMORY.md is excluded to avoid double-synthesis; only raw entries upload.
    assert set(by_name) == {"rollout_summaries/2026-06-04-abc.md"}
    entry = by_name["rollout_summaries/2026-06-04-abc.md"]
    assert entry["path"] == str(sub / "2026-06-04-abc.md")
    assert "project" not in entry


def test_collect_items_empty_when_dir_missing(tmp_path):
    assert capture.collect_items(tmp_path / "nope") == []


def test_format_context():
    out = recall.format_context("digest")
    assert out["hookSpecificOutput"]["hookEventName"] == "SessionStart"
    assert out["hookSpecificOutput"]["additionalContext"] == "digest"
    assert recall.format_context("") is None
    assert recall.format_context("   ") is None


def test_check_config_emit_shapes_system_message(capsys):
    check_config.emit("hello")
    out = capsys.readouterr().out
    assert json.loads(out) == {"systemMessage": "hello"}


def test_recall_passes_timeout_and_survives_timeout(monkeypatch, capsys):
    captured = {}

    def fake_run(*args, **kwargs):
        captured.update(kwargs)
        raise recall.subprocess.TimeoutExpired(
            cmd="memoryhub-mcp", timeout=kwargs.get("timeout")
        )

    monkeypatch.setattr(recall.subprocess, "run", fake_run)
    recall.main()  # a hung server must not raise or hang the session
    assert "timeout" in captured, "recall must bound subprocess.run with a timeout"
    assert capsys.readouterr().out == ""


def test_capture_passes_timeout_and_survives_timeout(monkeypatch, capsys, tmp_path):
    (tmp_path / "note.md").write_text("raw entry")
    monkeypatch.setattr(capture, "memories_dir", lambda: tmp_path)

    def fake_run(*args, **kwargs):
        assert "timeout" in kwargs, "capture must bound subprocess.run with a timeout"
        raise capture.subprocess.TimeoutExpired(cmd="x", timeout=kwargs.get("timeout"))

    monkeypatch.setattr(capture.subprocess, "run", fake_run)
    capture.main()  # must not raise
    assert capsys.readouterr().out == ""


def test_check_config_passes_timeout_and_survives_timeout(monkeypatch, capsys):
    def fake_run(*args, **kwargs):
        assert "timeout" in kwargs, "check_config must bound subprocess.run with a timeout"
        raise check_config.subprocess.TimeoutExpired(
            cmd="x", timeout=kwargs.get("timeout")
        )

    monkeypatch.setattr(check_config.subprocess, "run", fake_run)
    check_config.main()  # must not raise
    assert capsys.readouterr().out == ""
