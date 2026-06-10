import io
import json
import sys
from pathlib import Path
from unittest.mock import Mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "hooks"))

import capture  # noqa: E402  # type: ignore
import check_config  # noqa: E402  # type: ignore
import recall  # noqa: E402  # type: ignore


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
    payload = {
        "tool_calls": [
            {"tool_name": "Write", "tool_input": {"file_path": mem}},
            {"tool_name": "Edit", "tool_input": {"file_path": mem}},  # dup
            {
                "tool_name": "Write",
                "tool_input": {"file_path": "/tmp/x.md"},
            },  # not memory
            {"tool_name": "Read", "tool_input": {"file_path": mem}},  # not a write tool
        ]
    }
    items = capture.collect_items(payload)
    assert items == [{"project": "h", "filename": "memory/a.md", "path": mem}]


def test_format_context():
    out = recall.format_context("my digest")
    assert out["hookSpecificOutput"]["hookEventName"] == "SessionStart"
    assert out["hookSpecificOutput"]["additionalContext"] == "my digest"
    assert recall.format_context("") is None
    assert recall.format_context("   ") is None


def _run_main():
    try:
        check_config.main()
    except SystemExit as e:
        assert e.code in (0, None)


def test_check_config_silent_when_configured(monkeypatch, capsys):
    monkeypatch.setattr(
        check_config.subprocess, "run", Mock(return_value=Mock(returncode=0))
    )
    _run_main()
    assert capsys.readouterr().out == ""


def test_check_config_nudges_when_unconfigured(monkeypatch, capsys):
    monkeypatch.setattr(
        check_config.subprocess, "run", Mock(return_value=Mock(returncode=1))
    )
    _run_main()
    out = json.loads(capsys.readouterr().out)
    assert out["systemMessage"] == check_config.NOT_CONFIGURED


def test_check_config_reports_missing_binary(monkeypatch, capsys):
    monkeypatch.setattr(
        check_config.subprocess, "run", Mock(side_effect=FileNotFoundError)
    )
    _run_main()
    out = json.loads(capsys.readouterr().out)
    assert out["systemMessage"] == check_config.NOT_INSTALLED


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
    assert capsys.readouterr().out == ""  # fail-open: inject nothing


def test_capture_passes_timeout_and_survives_timeout(monkeypatch, capsys):
    mem = str(Path.home() / ".claude" / "projects" / "h" / "memory" / "a.md")
    payload = {"tool_calls": [{"tool_name": "Write", "tool_input": {"file_path": mem}}]}
    monkeypatch.setattr(capture.sys, "stdin", io.StringIO(json.dumps(payload)))

    def fake_run(*args, **kwargs):
        assert "timeout" in kwargs, "capture must bound subprocess.run with a timeout"
        raise capture.subprocess.TimeoutExpired(cmd="x", timeout=kwargs.get("timeout"))

    monkeypatch.setattr(capture.subprocess, "run", fake_run)
    capture.main()  # must not raise
    assert capsys.readouterr().out == ""  # stdout may be injected, so keep it clean


def test_check_config_passes_timeout_and_survives_timeout(monkeypatch, capsys):
    def fake_run(*args, **kwargs):
        assert "timeout" in kwargs, "check_config must bound subprocess.run with a timeout"
        raise check_config.subprocess.TimeoutExpired(
            cmd="x", timeout=kwargs.get("timeout")
        )

    monkeypatch.setattr(check_config.subprocess, "run", fake_run)
    _run_main()  # must not raise
    assert capsys.readouterr().out == ""  # don't nag on a timeout
