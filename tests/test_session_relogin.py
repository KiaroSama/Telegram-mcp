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


@pytest.mark.parametrize("empty", ['""', "''", ""])
def test_repair_existing_empty_entry(login, monkeypatch, empty):
    from telegram_mcp import account_snapshot

    env, actions, _ = login
    monkeypatch.setattr(account_snapshot, "PROCESS_ACCOUNT_VARS", {})
    env.write_text(f"TELEGRAM_SESSION_STRING={empty}\nOTHER=keep\n", encoding="utf-8")
    monkeypatch.setattr(
        "sys.argv",
        ["generator", "--phone", "--no-echo", "--replace-key", "TELEGRAM_SESSION_STRING"],
    )
    monkeypatch.setattr("builtins.input", lambda _: "y")
    generator.main()
    assert actions == ["connect", "phone", "disconnect"]
    assert env.read_text(encoding="utf-8") == "TELEGRAM_SESSION_STRING=new\nOTHER=keep\n"


def test_repair_interpolated_entry_matches_health_identity(login, monkeypatch):
    from telegram_mcp import account_snapshot, session_health

    env, actions, _ = login
    monkeypatch.setattr(account_snapshot, "PROCESS_ACCOUNT_VARS", {})
    monkeypatch.delenv("SESSION_VALUE", raising=False)
    env.write_text(
        "SESSION_VALUE=old\nTELEGRAM_SESSION_STRING=${SESSION_VALUE}\nOTHER=keep\n",
        encoding="utf-8",
    )
    fingerprint = session_health.digest(session_health.read_config(env)["TELEGRAM_SESSION_STRING"])
    monkeypatch.setattr(
        "sys.argv",
        [
            "generator",
            "--phone",
            "--no-echo",
            "--replace-key",
            "TELEGRAM_SESSION_STRING",
            "--expected-config-digest",
            fingerprint,
        ],
    )
    monkeypatch.setattr("builtins.input", lambda _: "y")
    generator.main()
    assert actions == ["connect", "phone", "disconnect"]
    assert (
        env.read_text(encoding="utf-8")
        == "SESSION_VALUE=old\nTELEGRAM_SESSION_STRING=new\nOTHER=keep\n"
    )


def test_config_writer_holds_lock_through_publication(tmp_path, monkeypatch):
    import os
    import subprocess
    import sys
    from pathlib import Path

    path = tmp_path / ".env"
    path.write_text("TELEGRAM_SESSION_STRING=old\nOTHER=keep\n", encoding="utf-8")
    root = str(Path(generator.__file__).parent)
    script = """
import sys
from pathlib import Path
import dotenv
dotenv.find_dotenv = lambda *a, **k: ''
dotenv.main.find_dotenv = dotenv.find_dotenv
sys.path.insert(0, sys.argv[1])
from telegram_mcp.alias_store import _try_acquire
import session_string_generator as generator
path = Path(sys.argv[2])
if _try_acquire(path):
    generator.write_env_value('OTHER', 'newer', path)
    print('wrote')
else:
    print('held')
"""
    original_replace = generator.os.replace
    observed = []

    def publish(source, target):
        child = subprocess.run(
            [sys.executable, "-c", script, root, str(path)],
            capture_output=True,
            text=True,
            timeout=15,
            check=True,
            env={**os.environ, "XDG_STATE_HOME": str(tmp_path / "child-state")},
        )
        observed.append(child.stdout.strip())
        assert observed == [
            "held"
        ], "Another writer entered between final comparison and publication."
        original_replace(source, target)

    monkeypatch.setattr(generator.os, "replace", publish)
    generator.write_env_value("TELEGRAM_SESSION_STRING", "new", path)
    assert path.read_text(encoding="utf-8") == "TELEGRAM_SESSION_STRING=new\nOTHER=keep\n"
    monkeypatch.setattr(generator.os, "replace", original_replace)
    child = subprocess.run(
        [sys.executable, "-c", script, root, str(path)],
        capture_output=True,
        text=True,
        timeout=15,
        check=True,
        env={**os.environ, "XDG_STATE_HOME": str(tmp_path / "child-state")},
    )
    assert child.stdout.strip() == "wrote"
    assert path.read_text(encoding="utf-8") == "TELEGRAM_SESSION_STRING=new\nOTHER=newer\n"


@pytest.mark.skipif(
    __import__("os").name != "nt", reason="PowerShell account manager uses Windows ACLs"
)
@pytest.mark.parametrize("operation", ["set", "rename", "remove"])
def test_powershell_writers_share_python_transaction_lock(tmp_path, operation):
    import shutil
    import subprocess
    from pathlib import Path
    from telegram_mcp.alias_store import _alias_lock

    shell = shutil.which("pwsh") or shutil.which("powershell")
    if shell is None:
        pytest.skip("PowerShell is not installed")
    path = tmp_path / ".env"
    original = "TELEGRAM_SESSION_STRING_WORK=old\nOTHER=keep\n"
    path.write_text(original, encoding="utf-8")
    root = Path(generator.__file__).parent
    script = tmp_path / "writer.ps1"
    script.write_text(
        """param($Root, $EnvPath, $Operation)
$ErrorActionPreference = 'Stop'
$envPath = $EnvPath
$script:EnvBackupRetention = 5
$script:MaxBackupCollisions = 100
. (Join-Path $Root 'account-manager/FileSafety.ps1')
. (Join-Path $Root 'account-manager/EnvFile.ps1')
$coreLock = ${function:Invoke-EnvWriteLock}
function Invoke-EnvWriteLock {
    param([scriptblock] $Action)
    & $coreLock -Action $Action -TimeoutMilliseconds 0
}
try {
    switch ($Operation) {
        set { Set-EnvValue -Key OTHER -Value newer | Out-Null }
        rename { Rename-EnvKey -From TELEGRAM_SESSION_STRING_WORK -To TELEGRAM_SESSION_STRING_RENAMED | Out-Null }
        remove { Remove-EnvKey -Key TELEGRAM_SESSION_STRING_WORK | Out-Null }
    }
    'wrote'
} catch {
    if ($_.Exception.Message -ne 'Configuration write lock timed out.') { throw }
    'held'
}
""",
        encoding="utf-8",
    )

    def run_writer():
        child = subprocess.run(
            [
                shell,
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-File",
                str(script),
                str(root),
                str(path),
                operation,
            ],
            capture_output=True,
            text=True,
            timeout=15,
            check=True,
        )
        return child.stdout.strip()

    with _alias_lock(path):
        assert run_writer() == "held"
        assert path.read_text(encoding="utf-8") == original
        assert list(tmp_path.glob(".env.backup-*")) == []
    assert run_writer() == "wrote"
    text = path.read_text(encoding="utf-8")
    if operation == "set":
        assert "OTHER=newer" in text and "TELEGRAM_SESSION_STRING_WORK=old" in text
    elif operation == "rename":
        assert "TELEGRAM_SESSION_STRING_RENAMED=old" in text and "OTHER=keep" in text
    else:
        assert "TELEGRAM_SESSION_STRING_WORK" not in text and "OTHER=keep" in text
    assert list(tmp_path.glob(".env.backup-*"))


def test_newest_backup_survives_reused_collision_name(tmp_path, monkeypatch):
    from datetime import datetime, timezone

    class Clock:
        @staticmethod
        def now(_):
            return datetime(2026, 10, 3, 20, 0, 0, tzinfo=timezone.utc)

    monkeypatch.setattr(generator, "datetime", Clock)
    monkeypatch.setattr(generator, "ENV_BACKUP_RETENTION", 1)
    path = tmp_path / ".env"
    path.write_text("OTHER=first\n", encoding="utf-8")
    for value in ("second", "third", "fourth"):
        previous = path.read_bytes()
        backup = generator.write_env_value("OTHER", value, path)
        assert backup.exists(), "Retention deleted the backup just returned to the owner."
        assert backup.read_bytes() == previous
        assert list(tmp_path.glob(".env.backup-*")) == [backup]
