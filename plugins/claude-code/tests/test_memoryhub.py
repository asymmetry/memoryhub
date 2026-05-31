import json
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parent.parent))
import memoryhub

# --- is_memory_path ---


def test_is_memory_path_matches_memory_md(tmp_path, monkeypatch):
    projects = tmp_path / "projects"
    monkeypatch.setattr("memoryhub.PROJECTS_DIR", projects)
    f = projects / "proj-hash" / "memory" / "user_role.md"
    f.parent.mkdir(parents=True)
    f.touch()
    assert memoryhub.is_memory_path(f)


def test_is_memory_path_rejects_non_memory_dir(tmp_path, monkeypatch):
    projects = tmp_path / "projects"
    monkeypatch.setattr("memoryhub.PROJECTS_DIR", projects)
    f = projects / "proj-hash" / "code" / "main.rs"
    assert not memoryhub.is_memory_path(f)


def test_is_memory_path_rejects_non_md_extension(tmp_path, monkeypatch):
    projects = tmp_path / "projects"
    monkeypatch.setattr("memoryhub.PROJECTS_DIR", projects)
    f = projects / "proj-hash" / "memory" / "note.txt"
    assert not memoryhub.is_memory_path(f)


def test_is_memory_path_rejects_outside_projects(tmp_path, monkeypatch):
    projects = tmp_path / "projects"
    monkeypatch.setattr("memoryhub.PROJECTS_DIR", projects)
    f = tmp_path / "other" / "memory" / "note.md"
    assert not memoryhub.is_memory_path(f)


# --- get_filename ---


def test_get_filename_returns_relative_to_projects(tmp_path, monkeypatch):
    projects = tmp_path / "projects"
    monkeypatch.setattr("memoryhub.PROJECTS_DIR", projects)
    f = projects / "proj-hash" / "memory" / "user_role.md"
    assert memoryhub.get_filename(f) == str(Path("proj-hash") / "memory" / "user_role.md")


def test_get_filename_nested(tmp_path, monkeypatch):
    projects = tmp_path / "projects"
    monkeypatch.setattr("memoryhub.PROJECTS_DIR", projects)
    f = projects / "proj-hash" / "memory" / "sub" / "deep.md"
    assert memoryhub.get_filename(f) == str(Path("proj-hash") / "memory" / "sub" / "deep.md")


# --- load_config / save_config ---


def test_load_config_exits_when_missing(tmp_path):
    with pytest.raises(SystemExit):
        memoryhub.load_config(path=tmp_path / "nonexistent.json")


def test_load_config_returns_dict(tmp_path):
    cfg = tmp_path / "config.json"
    data = {"url": "http://localhost:8000", "username": "alice", "agent_id": "abc"}
    cfg.write_text(json.dumps(data))
    assert memoryhub.load_config(path=cfg) == data


def test_save_config_writes_json(tmp_path):
    cfg = tmp_path / "config.json"
    data = {"url": "http://x", "username": "bob", "agent_id": "xyz"}
    memoryhub.save_config(data, path=cfg)
    assert json.loads(cfg.read_text()) == data


# --- inject_hook ---


def test_inject_hook_adds_hook_to_empty_settings(tmp_path):
    settings_path = tmp_path / "settings.json"
    memoryhub.inject_hook(path=settings_path)
    settings = json.loads(settings_path.read_text())
    batch = settings["hooks"][memoryhub.HOOK_EVENT]
    commands = [h["command"] for e in batch for h in e["hooks"]]
    assert memoryhub.HOOK_COMMAND in commands
    # PostToolBatch entries carry no matcher
    assert all("matcher" not in e for e in batch)


def test_inject_hook_does_not_duplicate(tmp_path):
    settings_path = tmp_path / "settings.json"
    memoryhub.inject_hook(path=settings_path)
    memoryhub.inject_hook(path=settings_path)
    settings = json.loads(settings_path.read_text())
    batch = settings["hooks"][memoryhub.HOOK_EVENT]
    commands = [h["command"] for e in batch for h in e["hooks"]]
    assert commands.count(memoryhub.HOOK_COMMAND) == 1


def test_inject_hook_merges_existing_hooks(tmp_path):
    settings_path = tmp_path / "settings.json"
    existing = {
        "hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "rtk hook claude"}]}]}
    }
    settings_path.write_text(json.dumps(existing))
    memoryhub.inject_hook(path=settings_path)
    settings = json.loads(settings_path.read_text())
    # Existing hook preserved
    assert settings["hooks"]["PreToolUse"][0]["matcher"] == "Bash"
    # New hook added
    batch = settings["hooks"][memoryhub.HOOK_EVENT]
    commands = [h["command"] for e in batch for h in e["hooks"]]
    assert memoryhub.HOOK_COMMAND in commands


# --- push_one ---
import io
from unittest.mock import MagicMock, patch
from urllib.error import HTTPError, URLError


def test_push_one_success(tmp_path):
    f = tmp_path / "test.md"
    f.write_text("hello world")
    config = {"url": "http://localhost:8000", "token": "mh_tok", "agent_id": "abc123"}
    mock_resp = MagicMock()
    mock_resp.__enter__ = lambda s: s
    mock_resp.__exit__ = MagicMock(return_value=False)
    captured = {}

    def capture(req, *a, **k):
        captured["headers"] = req.headers
        captured["payload"] = json.loads(req.data)
        return mock_resp

    with patch("memoryhub.get_filename", return_value="test.md"), patch(
        "urllib.request.urlopen", side_effect=capture
    ):
        ok, err = memoryhub.push_one(config, f)
    assert ok is True
    assert err is None
    # Identity comes from the bearer token; the body no longer carries a username.
    assert captured["headers"]["Authorization"] == "Bearer mh_tok"
    assert "username" not in captured["payload"]
    assert captured["payload"]["agent_id"] == "abc123"


def test_push_one_http_error(tmp_path):
    f = tmp_path / "test.md"
    f.write_text("hello")
    config = {"url": "http://localhost:8000", "token": "mh_tok", "agent_id": "abc"}
    with patch("memoryhub.get_filename", return_value="test.md"), patch(
        "urllib.request.urlopen", side_effect=HTTPError(None, 500, "Server Error", {}, None)
    ):
        ok, err = memoryhub.push_one(config, f)
    assert ok is False
    assert "500" in err


def test_push_one_network_error(tmp_path):
    f = tmp_path / "test.md"
    f.write_text("hello")
    config = {"url": "http://localhost:8000", "token": "mh_tok", "agent_id": "abc"}
    with patch("memoryhub.get_filename", return_value="test.md"), patch(
        "urllib.request.urlopen", side_effect=URLError("connection refused")
    ):
        ok, err = memoryhub.push_one(config, f)
    assert ok is False
    assert "Network error" in err


# --- cmd_push ---


def test_cmd_push_silent_on_non_memory_path(tmp_path, capsys):
    stdin_data = json.dumps(
        {"tool_calls": [{"tool_name": "Write", "tool_input": {"file_path": "/home/user/project/main.rs"}}]}
    )
    with patch("memoryhub.is_memory_path", return_value=False), patch("sys.stdin", io.StringIO(stdin_data)):
        with pytest.raises(SystemExit) as exc:
            memoryhub.cmd_push()
    assert exc.value.code == 0
    assert capsys.readouterr().out == ""


def test_cmd_push_silent_on_malformed_json(capsys):
    with patch("sys.stdin", io.StringIO("not json")):
        with pytest.raises(SystemExit) as exc:
            memoryhub.cmd_push()
    assert exc.value.code == 0


def test_cmd_push_ignores_read_only_batch(capsys):
    stdin_data = json.dumps({"tool_calls": [{"tool_name": "Read", "tool_input": {"file_path": "/x/memory/note.md"}}]})
    with patch("sys.stdin", io.StringIO(stdin_data)):
        with pytest.raises(SystemExit) as exc:
            memoryhub.cmd_push()
    assert exc.value.code == 0


def test_cmd_push_dedups_and_pushes_each_memory_file(tmp_path):
    stdin_data = json.dumps(
        {
            "tool_calls": [
                {"tool_name": "Write", "tool_input": {"file_path": "/x/memory/a.md"}},
                {"tool_name": "Edit", "tool_input": {"file_path": "/x/memory/a.md"}},
                {"tool_name": "MultiEdit", "tool_input": {"file_path": "/x/memory/b.md"}},
                {"tool_name": "Read", "tool_input": {"file_path": "/x/memory/c.md"}},
            ]
        }
    )
    config = {"url": "http://localhost:8000", "username": "alice", "agent_id": "abc"}
    pushed = []
    with patch("sys.stdin", io.StringIO(stdin_data)), patch("memoryhub.is_memory_path", return_value=True), patch(
        "memoryhub.load_config", return_value=config
    ), patch("memoryhub.push_one", side_effect=lambda c, p: pushed.append(p) or (True, None)):
        memoryhub.cmd_push()
    assert pushed == [Path("/x/memory/a.md"), Path("/x/memory/b.md")]


# --- cmd_push_all ---


def test_cmd_push_all_pushes_memory_files_and_excludes_others(tmp_path, monkeypatch):
    projects = tmp_path / "projects"
    monkeypatch.setattr("memoryhub.PROJECTS_DIR", projects)
    project_dir = projects / "h"
    (project_dir / "memory" / "sub").mkdir(parents=True)
    (project_dir / "memory" / "a.md").write_text("a")
    (project_dir / "memory" / "sub" / "b.md").write_text("b")
    (project_dir / "code").mkdir()
    (project_dir / "code" / "notes.md").write_text("not memory")
    config = {"url": "http://localhost:8000", "username": "alice", "agent_id": "abc"}
    pushed = []
    with patch("memoryhub.load_config", return_value=config), patch(
        "memoryhub.push_one", side_effect=lambda c, p: pushed.append(p) or (True, None)
    ), patch("builtins.print"):
        memoryhub.cmd_push_all(project_dir)
    assert sorted(pushed) == [
        project_dir / "memory" / "a.md",
        project_dir / "memory" / "sub" / "b.md",
    ]


def test_cmd_push_all_missing_project_exits(tmp_path):
    with patch("memoryhub.load_config", return_value={}), patch("builtins.print"):
        with pytest.raises(SystemExit) as exc:
            memoryhub.cmd_push_all(tmp_path / "nonexistent")
    assert exc.value.code == 0


def test_cmd_push_all_sends_projects_relative_filename(tmp_path, monkeypatch):
    projects = tmp_path / "projects"
    monkeypatch.setattr("memoryhub.PROJECTS_DIR", projects)
    project_dir = projects / "h"
    (project_dir / "memory").mkdir(parents=True)
    (project_dir / "memory" / "user_role.md").write_text("content")
    config = {"url": "http://localhost:8000", "token": "mh_tok", "agent_id": "abc"}
    pushed_filenames = []
    mock_resp = MagicMock()
    mock_resp.__enter__ = lambda s: s
    mock_resp.__exit__ = MagicMock(return_value=False)

    def capture(req, *a, **k):
        pushed_filenames.append(json.loads(req.data)["filename"])
        return mock_resp

    with patch("memoryhub.load_config", return_value=config), patch(
        "urllib.request.urlopen", side_effect=capture
    ), patch("builtins.print"):
        memoryhub.cmd_push_all(project_dir)
    assert pushed_filenames == [str(Path("h") / "memory" / "user_role.md")]


# --- cmd_config ---


def test_cmd_config_writes_config_and_injects_hook(tmp_path, monkeypatch):
    cfg_path = tmp_path / "config.json"
    monkeypatch.setattr("memoryhub.CONFIG_PATH", cfg_path)
    with patch("builtins.input", side_effect=["http://localhost:8000", "mh_tok"]), patch(
        "memoryhub.inject_hook"
    ) as mock_inject, patch("memoryhub.save_config") as mock_save, patch("builtins.print"):
        memoryhub.cmd_config()
    assert mock_save.called
    saved_config = mock_save.call_args[0][0]
    assert saved_config["url"] == "http://localhost:8000"
    assert saved_config["token"] == "mh_tok"
    assert len(saved_config["agent_id"]) == 36  # UUID
    mock_inject.assert_called_once()


def test_cmd_push_pushes_valid_memory_file(tmp_path, capsys):
    projects = tmp_path / "projects"
    f = projects / "proj-hash" / "memory" / "note.md"
    f.parent.mkdir(parents=True)
    f.write_text("# Note")
    stdin_data = json.dumps({"tool_calls": [{"tool_name": "Write", "tool_input": {"file_path": str(f)}}]})
    config = {"url": "http://localhost:8000", "username": "alice", "agent_id": "abc"}
    with patch("sys.stdin", io.StringIO(stdin_data)), patch("memoryhub.is_memory_path", return_value=True), patch(
        "memoryhub.load_config", return_value=config
    ), patch("memoryhub.push_one", return_value=(True, None)):
        memoryhub.cmd_push()
    assert capsys.readouterr().err == ""
