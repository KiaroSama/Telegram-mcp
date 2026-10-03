"""Login entry-point outcomes, with Telegram alone replaced by a controlled client."""

from types import SimpleNamespace

import pytest

import session_string_generator as generator


@pytest.fixture
def login(monkeypatch, tmp_path):
    env = tmp_path / ".env"
    env.write_text("TELEGRAM_SESSION_STRING=old\nOTHER=keep\n", encoding="utf-8")
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state"))
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(generator, "_ENV_PATH", env)
    monkeypatch.setenv("TELEGRAM_API_ID", "12345")
    monkeypatch.setenv("TELEGRAM_API_HASH", "synthetic")
    monkeypatch.setattr(generator, "_check_installation", lambda: None)
    actions = []

    class Client:
        session = object()

        def connect(self):
            actions.append("connect")

        def is_user_authorized(self):
            return False

        def get_me(self):
            return SimpleNamespace(id=42, username="synthetic_owner", phone=None)

        def disconnect(self):
            actions.append("disconnect")

        def is_connected(self):
            return False

    client = Client()
    monkeypatch.setattr(generator, "TelegramClient", lambda *a, **k: client)
    monkeypatch.setattr(generator.StringSession, "save", lambda _: "new")
    monkeypatch.setattr(generator, "_phone_login", lambda _: actions.append("phone"))
    monkeypatch.setattr(generator, "_qr_login", lambda _: actions.append("qr"))
    return env, actions, client


@pytest.mark.parametrize("choice, expected", [("", "phone"), ("1", "phone"), ("2", "qr")])
def test_login_menu_phone_first(login, monkeypatch, choice, expected):
    monkeypatch.setattr("sys.argv", ["generator", "--label", "work", "--no-echo"])
    monkeypatch.setattr("builtins.input", lambda _: choice)
    generator.main()
    assert login[1] == ["connect", expected, "disconnect"]


def test_replacement_saves_exact_default_after_disconnect(login, monkeypatch, capsys):
    env, actions, _ = login
    monkeypatch.setattr(
        "sys.argv",
        [
            "generator",
            "--phone",
            "--no-echo",
            "--replace-key",
            "TELEGRAM_SESSION_STRING",
            "--expected-user-id",
            "42",
        ],
    )
    real_write = generator.write_env_value

    def write(*args, **kwargs):
        assert actions[-1] == "disconnect"
        return real_write(*args, **kwargs)

    monkeypatch.setattr(generator, "write_env_value", write)
    generator.main()
    assert env.read_text(encoding="utf-8") == "TELEGRAM_SESSION_STRING=new\nOTHER=keep\n"
    assert "TELEGRAM_SESSION_STRING_DEFAULT=" not in env.read_text(encoding="utf-8")
    assert "\nnew\n" not in capsys.readouterr().out


@pytest.mark.parametrize("failure", ["identity", "disconnect", "save", "cancel", "concurrent"])
def test_failed_relogin_preserves_original(login, monkeypatch, failure):
    env, actions, client = login
    original = env.read_bytes()
    monkeypatch.setattr(
        "sys.argv",
        [
            "generator",
            "--phone",
            "--no-echo",
            "--replace-key",
            "TELEGRAM_SESSION_STRING",
            "--expected-user-id",
            "42",
        ],
    )
    if failure == "identity":
        monkeypatch.setattr(client, "get_me", lambda: SimpleNamespace(id=99))
    elif failure == "disconnect":
        monkeypatch.setattr(client, "is_connected", lambda: True)
    elif failure == "save":

        def fail(*a, **k):
            raise OSError("private-canary")

        monkeypatch.setattr(generator, "write_env_value", fail)
    elif failure == "cancel":

        def cancel(_):
            raise KeyboardInterrupt()

        monkeypatch.setattr(generator, "_phone_login", cancel)
    else:

        def changed(_):
            env.write_bytes(original + b"CONCURRENT=keep\n")

        monkeypatch.setattr(generator, "_phone_login", changed)
    with pytest.raises(SystemExit) as stopped:
        generator.main()
    assert stopped.value.code != 0
    assert env.read_bytes() == original + (
        b"CONCURRENT=keep\n" if failure == "concurrent" else b""
    )
    assert actions[-1] == "disconnect"


def test_file_session_conversion_preserves_other_bytes(login, monkeypatch):
    env, _, _ = login
    env.write_bytes(b'# note\r\n TELEGRAM_SESSION_NAME_WORK = "old-file"\r\nOTHER=keep\r\n')
    monkeypatch.setattr(
        "sys.argv",
        [
            "generator",
            "--phone",
            "--no-echo",
            "--replace-key",
            "TELEGRAM_SESSION_NAME_WORK",
            "--expected-user-id",
            "42",
        ],
    )
    monkeypatch.setattr("builtins.input", lambda _: "")
    generator.main()
    assert env.read_bytes() == b"# note\r\nTELEGRAM_SESSION_STRING_WORK=new\r\nOTHER=keep\r\n"
    backup = next(env.parent.glob(".env.backup-*"))
    assert b"TELEGRAM_SESSION_NAME_WORK" in backup.read_bytes()


def test_invalid_login_choice_opens_no_connection(login, monkeypatch):
    monkeypatch.setattr("sys.argv", ["generator", "--label", "work", "--no-echo"])
    monkeypatch.setattr("builtins.input", lambda _: "3")
    with pytest.raises(SystemExit):
        generator.main()
    assert login[1] == []


@pytest.mark.parametrize("flag, method", [("--phone", "phone"), ("--qr", "qr")])
def test_explicit_methods_remain_unchanged(login, monkeypatch, flag, method):
    monkeypatch.setattr("sys.argv", ["generator", flag, "--label", "work", "--no-echo"])
    generator.main()
    assert login[1] == ["connect", method, "disconnect"]


def test_changed_health_snapshot_refuses_login(login, monkeypatch):
    env, actions, _ = login
    original = env.read_bytes()
    monkeypatch.setattr(
        "sys.argv",
        [
            "generator",
            "--phone",
            "--no-echo",
            "--replace-key",
            "TELEGRAM_SESSION_STRING",
            "--expected-config-digest",
            "different",
        ],
    )
    with pytest.raises(SystemExit):
        generator.main()
    assert actions == []
    assert env.read_bytes() == original


def test_trusted_owner_binding_rejects_wrong_login(login, monkeypatch):
    env, _, _ = login
    owner = env.parent / "state" / "telegram-mcp" / "secret-chats" / "default.owner.json"
    owner.parent.mkdir(parents=True)
    owner.write_text('{"user_id":99}', encoding="utf-8")
    monkeypatch.setattr(
        "sys.argv",
        ["generator", "--phone", "--no-echo", "--replace-key", "TELEGRAM_SESSION_STRING"],
    )
    original = env.read_bytes()
    with pytest.raises(SystemExit):
        generator.main()
    assert env.read_bytes() == original
    assert owner.read_text(encoding="utf-8") == '{"user_id":99}'


def test_private_permission_failure_writes_no_credentials(login, monkeypatch):
    env, _, _ = login
    original = env.read_bytes()
    monkeypatch.setattr(generator, "restrict_to_owner", lambda _: False)
    with pytest.raises(OSError):
        generator.write_env_value("TELEGRAM_SESSION_STRING", "new", env)
    assert env.read_bytes() == original
    assert all(
        b"new" not in p.read_bytes() and b"old" not in p.read_bytes()
        for p in env.parent.glob(".env.backup-*")
    )
