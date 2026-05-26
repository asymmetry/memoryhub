import json
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parent.parent))
import memoryhub


# --- is_memory_path ---

def test_is_memory_path_matches_memory_md(tmp_path):
    projects = tmp_path / "projects"
    f = projects / "proj-hash" / "memory" / "user_role.md"
    f.parent.mkdir(parents=True)
    f.touch()
    assert memoryhub.is_memory_path(str(f), projects_dir=projects)


def test_is_memory_path_rejects_non_memory_dir(tmp_path):
    projects = tmp_path / "projects"
    f = projects / "proj-hash" / "code" / "main.rs"
    assert not memoryhub.is_memory_path(str(f), projects_dir=projects)


def test_is_memory_path_rejects_non_md_extension(tmp_path):
    projects = tmp_path / "projects"
    f = projects / "proj-hash" / "memory" / "note.txt"
    assert not memoryhub.is_memory_path(str(f), projects_dir=projects)


def test_is_memory_path_rejects_outside_projects(tmp_path):
    projects = tmp_path / "projects"
    f = tmp_path / "other" / "memory" / "note.md"
    assert not memoryhub.is_memory_path(str(f), projects_dir=projects)


# --- get_filename ---

def test_get_filename_returns_relative_to_memory(tmp_path):
    projects = tmp_path / "projects"
    f = projects / "proj-hash" / "memory" / "user_role.md"
    assert memoryhub.get_filename(str(f), projects_dir=projects) == "user_role.md"


def test_get_filename_nested(tmp_path):
    projects = tmp_path / "projects"
    f = projects / "proj-hash" / "memory" / "sub" / "deep.md"
    result = memoryhub.get_filename(str(f), projects_dir=projects)
    assert result == str(Path("sub") / "deep.md")


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
    memoryhub.inject_hook(settings_path=settings_path)
    settings = json.loads(settings_path.read_text())
    post = settings["hooks"]["PostToolUse"]
    write_entry = next(e for e in post if e["matcher"] == "Write")
    commands = [h["command"] for h in write_entry["hooks"]]
    assert memoryhub.HOOK_COMMAND in commands


def test_inject_hook_does_not_duplicate(tmp_path):
    settings_path = tmp_path / "settings.json"
    memoryhub.inject_hook(settings_path=settings_path)
    memoryhub.inject_hook(settings_path=settings_path)
    settings = json.loads(settings_path.read_text())
    post = settings["hooks"]["PostToolUse"]
    write_entry = next(e for e in post if e["matcher"] == "Write")
    commands = [h["command"] for h in write_entry["hooks"]]
    assert commands.count(memoryhub.HOOK_COMMAND) == 1


def test_inject_hook_merges_existing_hooks(tmp_path):
    settings_path = tmp_path / "settings.json"
    existing = {
        "hooks": {
            "PreToolUse": [
                {"matcher": "Bash", "hooks": [{"type": "command", "command": "rtk hook claude"}]}
            ]
        }
    }
    settings_path.write_text(json.dumps(existing))
    memoryhub.inject_hook(settings_path=settings_path)
    settings = json.loads(settings_path.read_text())
    # Existing hook preserved
    assert settings["hooks"]["PreToolUse"][0]["matcher"] == "Bash"
    # New hook added
    post = settings["hooks"]["PostToolUse"]
    assert any(e["matcher"] == "Write" for e in post)


# --- push_single_file ---
import io
from unittest.mock import MagicMock, patch
from urllib.error import HTTPError, URLError


def test_push_single_file_success(tmp_path):
    f = tmp_path / "test.md"
    f.write_text("hello world")
    config = {"url": "http://localhost:8000", "username": "alice", "agent_id": "abc123"}
    mock_resp = MagicMock()
    mock_resp.__enter__ = lambda s: s
    mock_resp.__exit__ = MagicMock(return_value=False)
    with patch("memoryhub.get_filename", return_value="test.md"), \
         patch("urllib.request.urlopen", return_value=mock_resp):
        ok, err = memoryhub.push_single_file(config, f)
    assert ok is True
    assert err is None


def test_push_single_file_http_error(tmp_path):
    f = tmp_path / "test.md"
    f.write_text("hello")
    config = {"url": "http://localhost:8000", "username": "alice", "agent_id": "abc"}
    with patch("memoryhub.get_filename", return_value="test.md"), \
         patch("urllib.request.urlopen", side_effect=HTTPError(None, 500, "Server Error", {}, None)):
        ok, err = memoryhub.push_single_file(config, f)
    assert ok is False
    assert "500" in err


def test_push_single_file_network_error(tmp_path):
    f = tmp_path / "test.md"
    f.write_text("hello")
    config = {"url": "http://localhost:8000", "username": "alice", "agent_id": "abc"}
    with patch("memoryhub.get_filename", return_value="test.md"), \
         patch("urllib.request.urlopen", side_effect=URLError("connection refused")):
        ok, err = memoryhub.push_single_file(config, f)
    assert ok is False
    assert "Network error" in err


# --- cmd_push_file ---

def test_cmd_push_file_silent_on_non_memory_path(tmp_path, capsys):
    stdin_data = json.dumps({"tool_input": {"file_path": "/home/user/project/main.rs"}})
    with patch("memoryhub.is_memory_path", return_value=False), \
         patch("sys.stdin", io.StringIO(stdin_data)):
        with pytest.raises(SystemExit) as exc:
            memoryhub.cmd_push_file()
    assert exc.value.code == 0
    assert capsys.readouterr().out == ""


def test_cmd_push_file_silent_on_malformed_json(capsys):
    with patch("sys.stdin", io.StringIO("not json")):
        with pytest.raises(SystemExit) as exc:
            memoryhub.cmd_push_file()
    assert exc.value.code == 0


# --- cmd_push_all ---

def test_cmd_push_all_prints_summary(tmp_path):
    memory_dir = tmp_path / "memory"
    memory_dir.mkdir()
    (memory_dir / "a.md").write_text("file a")
    (memory_dir / "b.md").write_text("file b")
    config = {"url": "http://localhost:8000", "username": "alice", "agent_id": "abc"}
    with patch("memoryhub.load_config", return_value=config), \
         patch("memoryhub.push_single_file", return_value=(True, None)) as mock_push, \
         patch("builtins.print"):
        memoryhub.cmd_push_all(str(memory_dir))
    assert mock_push.call_count == 2


def test_cmd_push_all_missing_dir_exits(tmp_path):
    with patch("memoryhub.load_config", return_value={}), \
         patch("builtins.print"):
        with pytest.raises(SystemExit) as exc:
            memoryhub.cmd_push_all(str(tmp_path / "nonexistent"))
    assert exc.value.code == 0


def test_cmd_push_all_uses_relative_filename(tmp_path):
    memory_dir = tmp_path / "memory"
    memory_dir.mkdir()
    (memory_dir / "user_role.md").write_text("content")
    config = {"url": "http://localhost:8000", "username": "alice", "agent_id": "abc"}
    pushed_filenames = []
    def capture_push(cfg, path, filename=None):
        pushed_filenames.append(filename)
        return (True, None)
    with patch("memoryhub.load_config", return_value=config), \
         patch("memoryhub.push_single_file", side_effect=capture_push), \
         patch("builtins.print"):
        memoryhub.cmd_push_all(str(memory_dir))
    assert pushed_filenames == ["user_role.md"]


# --- cmd_config ---

def test_cmd_config_writes_config_and_injects_hook(tmp_path, monkeypatch):
    cfg_path = tmp_path / "config.json"
    monkeypatch.setattr("memoryhub.CONFIG_PATH", cfg_path)
    with patch("builtins.input", side_effect=["http://localhost:8000", "alice"]), \
         patch("memoryhub.inject_hook") as mock_inject, \
         patch("memoryhub.save_config") as mock_save, \
         patch("builtins.print"):
        memoryhub.cmd_config()
    assert mock_save.called
    saved_config = mock_save.call_args[0][0]
    assert saved_config["url"] == "http://localhost:8000"
    assert saved_config["username"] == "alice"
    assert len(saved_config["agent_id"]) == 36  # UUID
    mock_inject.assert_called_once()


def test_cmd_push_file_pushes_valid_memory_file(tmp_path, capsys):
    projects = tmp_path / "projects"
    f = projects / "proj-hash" / "memory" / "note.md"
    f.parent.mkdir(parents=True)
    f.write_text("# Note")
    stdin_data = json.dumps({"tool_input": {"file_path": str(f)}})
    config = {"url": "http://localhost:8000", "username": "alice", "agent_id": "abc"}
    with patch("sys.stdin", io.StringIO(stdin_data)), \
         patch("memoryhub.is_memory_path", return_value=True), \
         patch("memoryhub.load_config", return_value=config), \
         patch("memoryhub.push_single_file", return_value=(True, None)):
        memoryhub.cmd_push_file()
    assert capsys.readouterr().err == ""
