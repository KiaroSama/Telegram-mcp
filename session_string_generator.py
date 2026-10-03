#!/usr/bin/env python3
"""
Telegram Session String Generator

This script generates a session string that can be used for Telegram authentication
with the Telegram MCP server. The session string allows for portable authentication
without storing session files.

Usage:
    python session_string_generator.py
    python session_string_generator.py --qr

Requirements:
    - telethon
    - python-dotenv

Note on ID Formats:
When using the MCP server, please be aware that all `chat_id` and `user_id`
parameters support integer IDs, string representations of IDs (e.g., "123456"),
and usernames (e.g., "@mychannel").
"""

import argparse
import asyncio
import getpass
import hashlib
import json
import io
import os
import re
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path
from typing import Optional

from dotenv import load_dotenv
from telegram_mcp.session_health import parse_config
from telethon import errors
from telethon.sessions import StringSession
from telethon.sync import TelegramClient
from telegram_mcp.client_identity import client_identity_kwargs
from telegram_mcp.aliases import normalise_account_label, restrict_to_owner
from telegram_mcp.alias_store import _alias_lock
from telegram_mcp.console_theme import default_hint, failure, heading, hint, note
from telegram_mcp.install_guard import UnsafeInstallationError, assert_safe_distribution

# How many times the QR code is regenerated after expiry before giving up.
_QR_MAX_REFRESHES = 10

_ENV_PATH = Path(__file__).resolve().with_name(".env")
_UNCHECKED = object()


_ENV_KEY_RE = re.compile(r"\A[A-Za-z_][A-Za-z0-9_]*\Z")

# One normaliser, not a second copy of the rule: the account manager applies the
# same one before it ever gets here, and the client registry reads back the env
# keys this produces. See telegram_mcp.aliases.normalise_account_label.
normalise_label = normalise_account_label


# How many times the 2FA password is asked for before the run gives up. `while
# True` had no ceiling at all, and "the user gives up" is not a condition that
# exists when stdin is a script.
MAX_PASSWORD_ATTEMPTS = 5


ENV_BACKUP_RETENTION = 5
_MAX_BACKUP_COLLISIONS = 100


def _write_owner_only(path: Path, text: str) -> None:
    """Create ``path`` owner-only from the first byte, refusing to clobber.

    The 0600 in the open is the POSIX half and is what makes the file private
    before a single byte is in it. `restrict_to_owner` is the Windows half:
    there the mode argument is ignored and the file inherits the directory's
    ACL, so a backup of every configured login lands readable by every account
    on the machine.
    """
    fd = os.open(str(path), os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8", newline="") as handle:
        if not restrict_to_owner(path):
            raise OSError("Private file permissions could not be verified.")
        handle.write(text)


def _backup_env(env_path: Path) -> Path:
    """Copy `.env` aside owner-only, under a name nothing else has taken."""
    stamp = datetime.now(timezone.utc).strftime("%Y-%m-%d_%H-%M-%S_UTC")
    text = env_path.read_bytes().decode("utf-8")
    for attempt in range(_MAX_BACKUP_COLLISIONS):
        suffix = "" if attempt == 0 else f"-{attempt}"
        backup = env_path.with_name(f"{env_path.name}.backup-{stamp}{suffix}")
        try:
            _write_owner_only(backup, text)
            return backup
        except FileExistsError:
            continue
    raise OSError(f"could not find a free backup name for {env_path} within one second")


def _prune_env_backups(env_path: Path, latest: Optional[Path] = None) -> None:
    """Keep the newest ``ENV_BACKUP_RETENTION`` backups and delete the rest.

    Each one holds a complete login to every account configured at the time, so
    an unbounded pile of them turns one readable directory into a leak of every
    session ever generated on the machine.
    """
    backups = sorted(
        env_path.parent.glob(f"{env_path.name}.backup-*"), key=lambda path: (path == latest, path)
    )
    for stale in backups[: max(0, len(backups) - ENV_BACKUP_RETENTION)]:
        try:
            stale.unlink()
        except OSError:
            pass


def write_env_value(
    key: str,
    value: str,
    env_path: Path = Path(".env"),
    *,
    expected: object = _UNCHECKED,
    replace_key: Optional[str] = None,
) -> Optional[Path]:
    r"""Set one key in `.env`, replacing its line or appending it, and back the file up.

    Returns the backup's path, or None when there was no file to back up.

    Extracted from main() so it can be exercised without a Telegram login. It sat
    inside the interactive flow, which meant the only way to test the file handling
    was to fake an entire sign-in - so it never was tested, and it rewrote the file
    holding every configured account with no way back.

    Every other line survives byte-for-byte: comments, ordering, and every key this
    knows nothing about, which is most of the file.

    The file and its backups are session strings -- full logins -- so both are
    created 0600 rather than inheriting the umask's 0644, the replacement is
    atomic so a crash cannot leave a half-written `.env`, and old backups are
    pruned rather than kept for ever.
    """
    if not key or any(character.isspace() for character in key):
        # python-dotenv drops such a line on read, so the write would look like a
        # success and produce a setting that never loads.
        raise ValueError(f"an env key cannot contain whitespace: {key!r}")
    if not _ENV_KEY_RE.match(key):
        # An '=' or any other punctuation moves where the line splits, so the
        # value is read back under a different key than the one written.
        raise ValueError(f"not a usable env key: {key!r}")

    env_path = env_path.resolve()
    with _alias_lock(env_path):
        original = env_path.read_bytes() if env_path.exists() else None
        if expected is not _UNCHECKED and original != expected:
            raise RuntimeError("Configuration changed during login; nothing was replaced.")
        if any(c in value for c in "\r\n\x00"):
            raise ValueError("A session value cannot contain a newline or NUL.")
        old_key = replace_key or key
        if not _ENV_KEY_RE.fullmatch(old_key):
            raise ValueError("Not a usable replacement key.")
        text = original.decode("utf-8") if original is not None else ""
        # Parse the same syntax the runtime accepts, including whitespace and quotes.
        from dotenv.parser import parse_stream

        bindings = list(parse_stream(io.StringIO(text)))
        if any(binding.error for binding in bindings):
            raise ValueError("Configuration cannot be parsed; nothing was replaced.")
        found = [binding for binding in bindings if binding.key == old_key]
        if len(found) > 1 or (replace_key and len(found) != 1):
            raise ValueError("The account must have exactly one configuration entry.")
        if old_key != key and any(binding.key == key for binding in bindings):
            raise ValueError("The replacement string key already exists; nothing was replaced.")
        newline = "\r\n" if "\r\n" in text else "\n"
        updated = "".join(
            f"{key}={value}{newline}" if binding.key == old_key else binding.original.string
            for binding in bindings
        )
        if not found:
            if updated and not updated.endswith(("\n", "\r")):
                updated += newline
            updated += f"{key}={value}{newline}"
        backup = _backup_env(env_path) if original is not None else None

        # mkstemp makes a 0600 file with an unpredictable name in the same
        # directory, so the rename is atomic and no reader ever sees a partial file.
        fd, tmp = tempfile.mkstemp(
            dir=str(env_path.parent), prefix=env_path.name + ".", suffix=".tmp"
        )
        try:
            with os.fdopen(fd, "w", encoding="utf-8", newline="") as handle:
                if not restrict_to_owner(tmp):
                    raise OSError("Private file permissions could not be verified.")
                handle.write(updated)
                handle.flush()
                os.fsync(handle.fileno())  # the rename must not outrun the bytes
            # Before the rename: an ACL travels with the file, so `.env` is never
            # briefly readable under its real name.
            if not restrict_to_owner(tmp):
                raise OSError("Private file permissions could not be verified.")
            current = env_path.read_bytes() if env_path.exists() else None
            if current != original:
                raise RuntimeError(
                    "Configuration changed before publication; nothing was replaced."
                )
            os.replace(tmp, env_path)
        except BaseException:
            try:
                os.unlink(tmp)
            except OSError:
                pass
            raise

        _prune_env_backups(env_path, latest=backup)
        return backup


def _parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Generate a Telegram session string for telegram-mcp."
    )
    login_group = parser.add_mutually_exclusive_group()
    login_group.add_argument(
        "--qr",
        action="store_true",
        help="Use Telegram QR login without prompting for a login method.",
    )
    login_group.add_argument(
        "--phone",
        action="store_true",
        help="Use phone number + verification code login without prompting for a login method.",
    )
    parser.add_argument(
        "--label",
        default=None,
        help=(
            "Account label to save under, skipping the prompt. Passed by the account "
            "manager, which has already asked for one."
        ),
    )
    parser.add_argument(
        "--no-echo",
        action="store_true",
        help=(
            "Save the session to .env without printing it. A terminal is "
            "scrollback, often a screen share, and sometimes a shell log."
        ),
    )
    parser.add_argument(
        "--replace-key", help="Re-login for this exact existing configuration key."
    )
    parser.add_argument("--expected-user-id", type=int, help="Refuse a login to a different user.")
    parser.add_argument(
        "--expected-config-digest", help="Refuse repair if the checked entry changed."
    )
    args = parser.parse_args()
    if args.replace_key and (
        not re.fullmatch(r"TELEGRAM_SESSION_(?:STRING|NAME)(?:_[A-Za-z0-9_]+)?", args.replace_key)
        or not args.no_echo
    ):
        parser.error("Replacement requires an exact session key and --no-echo.")
    if args.expected_user_id is not None and (not args.replace_key or args.expected_user_id <= 0):
        parser.error("--expected-user-id requires a replacement and a positive user ID.")
    return args


def _check_installation() -> None:
    try:
        assert_safe_distribution()
    except UnsafeInstallationError as exc:
        print(str(exc), file=sys.stderr)
        sys.exit(1)


def _render_qr(qr) -> None:
    import qrcode

    print()
    print(heading("QR code login"))
    print()

    qr_obj = qrcode.QRCode(border=1)
    qr_obj.add_data(qr.url)
    qr_obj.make(fit=True)
    f = io.StringIO()
    qr_obj.print_ascii(out=f, invert=True)
    try:
        print(f.getvalue())
    except UnicodeEncodeError:
        # A redirected Windows console (cp1252) has no block characters (upstream
        # chigwell #195); the link below logs in just the same.
        print(note("This console cannot draw the QR code; open the link below instead."))

    print(note("Scan the QR code above with your Telegram app:"))
    print(hint("  Open Telegram > Settings > Devices > Link Desktop Device"))
    print()
    print(f"Or open this link on a device where you're logged in:\n  {qr.url}\n")
    print(hint(f"Expires at: {_expiry_clock(qr)}"))
    print(hint("Waiting for you to scan..."))


def _aware_expiry(qr):
    """`qr.expires` as an aware datetime.

    Telethon returns it in UTC. One place, because the countdown normalised it
    and the printed clock did not, and the two disagreeing by the local offset
    is exactly the bug below.
    """
    expires = qr.expires
    return expires if expires.tzinfo is not None else expires.replace(tzinfo=timezone.utc)


def _expiry_clock(qr) -> str:
    """The expiry on the clock the person is actually looking at.

    Printed straight, `qr.expires` shows UTC with nothing saying so: someone in
    UTC+3:30 reads "Expires at: 14:32:47" while their own clock says 18:02, and
    a perfectly good sixty-second countdown looks broken. The seconds were never
    wrong - only this line was.
    """
    return _aware_expiry(qr).astimezone().strftime("%H:%M:%S")


def _seconds_until_expiry(qr) -> float:
    """Seconds left before this QR token expires, with a small safety margin."""
    remaining = (_aware_expiry(qr) - datetime.now(timezone.utc)).total_seconds()
    return max(1.0, remaining - 1.0)


def _qr_login(client: TelegramClient) -> None:
    qr = client.qr_login()
    _render_qr(qr)

    for _ in range(_QR_MAX_REFRESHES):
        try:
            client.loop.run_until_complete(qr.wait(timeout=_seconds_until_expiry(qr)))
            return
        except asyncio.TimeoutError:
            client.loop.run_until_complete(qr.recreate())
            print()
            print(note("That QR code expired. Here is a fresh one."))
            _render_qr(qr)
        except errors.SessionPasswordNeededError:
            _sign_in_with_password(client)
            return
            return

    print()
    print(failure("The QR code expired too many times. Run the generator again."))
    client.disconnect()
    sys.exit(1)


def _sign_in_with_password(client: TelegramClient) -> None:
    """Ask for the 2FA password until it is accepted or the attempts run out.

    Shared by BOTH login paths on purpose. This loop used to exist only in the QR
    branch; the phone branch called sign_in once, so a single mistyped password
    raised PasswordHashInvalidError, escaped to the outer handler and killed the
    whole run with "Failed to generate session string" - after the code had
    already been used, which is the expensive part to redo.

    Bounded, because it used to be `while True` and "the user gives up" is not a
    condition that exists in automation: a scripted or piped stdin answering the
    same wrong password - or answering nothing, which does not even reach
    Telegram - never leaves the loop, and there is no one at the terminal to
    interrupt it. Every remaining attempt is counted out loud so a person can see
    the run ending before it does.
    """
    for remaining in range(MAX_PASSWORD_ATTEMPTS, 0, -1):
        pw = getpass.getpass("\nTwo-factor authentication enabled. Please enter your password: ")
        if not pw:
            print(note(f"No password entered. {remaining - 1} attempt(s) left."))
            continue
        try:
            client.sign_in(password=pw)
            # Accepted, and not carried any further. It used to be RETURNED, so
            # a second authorisation could reuse it rather than ask again; there
            # is no second authorisation now, and a password travelling across
            # three functions for nobody is how one ends up somewhere it was
            # never meant to be.
            return
        except errors.PasswordHashInvalidError:
            print(failure(f"That password was not accepted. {remaining - 1} attempt(s) left."))

    print()
    print(
        failure(
            f"The password was not accepted in {MAX_PASSWORD_ATTEMPTS} attempts, so nothing "
            "was saved. Run the generator again when you have it to hand - the cost is one "
            "more QR scan or login code."
        )
    )
    client.disconnect()
    sys.exit(1)


def _phone_login(client: TelegramClient) -> None:
    phone = input("Please enter your phone (or bot token): ")

    try:
        client.send_code_request(phone)
    except errors.FloodWaitError as e:
        print()
        print(failure(f"Telegram asked for a wait of {e.seconds} seconds before trying again."))
        client.disconnect()
        sys.exit(1)
    except errors.PhoneNumberInvalidError:
        print()
        print(failure("That phone number is not valid."))
        client.disconnect()
        sys.exit(1)
    except Exception as e:
        print()
        print(failure(f"Telegram would not send the code ({type(e).__name__})."))
        client.disconnect()
        sys.exit(1)

    code = input("\nPlease enter the code you received: ")
    try:
        client.sign_in(phone, code)
    except errors.SessionPasswordNeededError:
        _sign_in_with_password(client)
        return
    return None


def _report_session(env_var: str, session_string: str, *, echo: bool) -> None:
    """Say the login succeeded, and show the session only if asked to.

    Printing a StringSession puts a full account login into scrollback, into a
    screen share, and into whatever records the terminal. `--no-echo` names the
    key it will be saved under and nothing else.
    """
    print()
    print(heading("Authentication successful."))
    if echo:
        print(heading("Your session string"))
        print(f"\n{session_string}\n")
        print("Add this to your .env file as:")
        print(f"{env_var}={session_string}")
        print()
        print(note("This string is a full login to that account. Never share it."))
    else:
        print(note(f"The session will be saved to .env as {env_var} and not printed."))


def _main(event) -> None:
    args = _parse_args()
    _check_installation()
    load_dotenv(_ENV_PATH, encoding="utf-8")

    API_ID = os.getenv("TELEGRAM_API_ID")
    API_HASH = os.getenv("TELEGRAM_API_HASH")

    if not API_ID or not API_HASH:
        print(failure("TELEGRAM_API_ID and TELEGRAM_API_HASH must be set in .env."))
        print(
            hint(
                "Get them from my.telegram.org/apps (API development tools), then put them in .env."
            )
        )
        sys.exit(1)

    try:
        API_ID = int(API_ID)
    except ValueError:
        print(failure("TELEGRAM_API_ID must be a number."))
        sys.exit(1)

    print()
    print(heading("Telegram session string generator"))
    print()
    if args.label is None:
        # Skipped when the account manager is the caller: it has just explained the
        # same thing, and saying it twice in two styles is what prompted this.
        print("This script will generate a session string for your Telegram account.")
        print("The generated session string can be added to your .env file.")
    print(
        "\nYour credentials will NOT be stored on any server and are only used for local authentication.\n"
    )

    original = _ENV_PATH.read_bytes() if _ENV_PATH.exists() else None
    if args.replace_key:
        values = parse_config(original or b"")
        if args.replace_key not in values:
            print(failure("The selected account entry is missing. Nothing was changed."))
            sys.exit(1)
        if (
            args.expected_config_digest
            and hashlib.sha256(values[args.replace_key].encode("utf-8")).hexdigest()
            != args.expected_config_digest
        ):
            print(failure("The checked session changed. Check health again before re-login."))
            sys.exit(1)
        suffix = (
            args.replace_key.removeprefix("TELEGRAM_SESSION_STRING")
            .removeprefix("TELEGRAM_SESSION_NAME")
            .lstrip("_")
        )
        label = suffix.lower()
        print(note("Re-login cannot restore old device-bound secret chats. Their files are kept."))
        if args.replace_key.startswith("TELEGRAM_SESSION_NAME"):
            if input(
                "Convert this file-based account to a string session? [Y/n]: "
            ).strip().lower() not in {"", "y", "yes"}:
                print(note("Cancelled. Nothing was changed."))
                sys.exit(1)
    elif args.label is not None:
        # Supplied by the account manager, which asked for it already.
        label = args.label.strip()
    else:
        try:
            label = (
                input(
                    "Account label (optional, e.g. 'work', 'personal'; leave empty for default): "
                )
                .strip()
                .lower()
            )
        except EOFError:
            # Non-interactive stdin (piped/scripted runs): fall back to the default label.
            label = ""

    # Before the login, not after it: a label that cannot become an env key used
    # to be discovered once the session already existed, and the run then either
    # wrote an unreadable line or threw the freshly minted session away.
    if label:
        try:
            safe_label = normalise_label(label)
        except ValueError as exc:
            print(failure(str(exc)))
            sys.exit(1)
        if safe_label != label.strip():
            print(hint(f"Saving under '{safe_label}' - a key cannot contain a space."))
        env_var = f"TELEGRAM_SESSION_STRING_{safe_label.upper()}"
    else:
        safe_label = "default"
        env_var = "TELEGRAM_SESSION_STRING"

    if args.replace_key:
        env_var = args.replace_key.replace("TELEGRAM_SESSION_NAME", "TELEGRAM_SESSION_STRING", 1)

    if args.phone:
        method = "1"
    elif args.qr:
        method = "2"
    else:
        print()
        print(heading("Choose login method:"))
        print("  1) Phone number + verification code (default)")
        print("  2) QR code login (scan from your Telegram app)")
        print()
        method = input(f'Selection {default_hint("[1]")}: ').strip() or "1"
    if method not in {"1", "2"}:
        print(failure("Choose 1 for phone login or 2 for QR login. Nothing was saved."))
        sys.exit(1)

    expected_user_id = args.expected_user_id
    if args.replace_key:
        base = os.getenv("XDG_STATE_HOME") or Path.home() / ".local" / "state"
        owner = Path(base) / "telegram-mcp" / "secret-chats" / f"{safe_label.lower()}.owner.json"
        if owner.exists():
            try:
                recorded = json.loads(owner.read_text(encoding="utf-8"))["user_id"]
                if type(recorded) is not int or recorded <= 0:
                    raise ValueError("Invalid owner identity.")
                if expected_user_id is not None and expected_user_id != recorded:
                    raise ValueError("Conflicting owner identity.")
                expected_user_id = recorded
            except Exception:
                print(
                    failure(
                        "The existing account identity cannot be verified. Nothing was changed."
                    )
                )
                sys.exit(1)
    client = None
    disconnected = False
    try:
        client = TelegramClient(StringSession(), API_ID, API_HASH, **client_identity_kwargs())
        client.connect()

        if not client.is_user_authorized():
            if method == "1":
                _phone_login(client)
            else:
                _qr_login(client)

        me = client.get_me()
        if me is None:
            raise RuntimeError("Authentication was not completed.")
        if args.replace_key:
            if expected_user_id is not None:
                if me.id != expected_user_id:
                    raise RuntimeError(
                        "A different Telegram account signed in; nothing was replaced."
                    )
            else:
                print(note(f"Authenticated user ID: {me.id}."))
                if input(
                    "Is this the account you intend to repair? [Y/n]: "
                ).strip().lower() not in {"", "y", "yes"}:
                    raise RuntimeError("Account confirmation cancelled; nothing was replaced.")
        session_string = StringSession.save(client.session)
        if not session_string:
            raise RuntimeError("Authentication produced no session; nothing was saved.")
        client.disconnect()
        if client.is_connected():
            raise RuntimeError("The new login did not disconnect; nothing was saved.")
        disconnected = True
        event("INFO", "Authentication completed; connection closed")
        _report_session(env_var, session_string, echo=not args.no_echo)

        if args.no_echo:
            # --no-echo means "save it, do not show it": there is nothing on
            # screen to copy, so asking whether to save would only be a way to
            # lose the session.
            save = True
        else:
            try:
                print()
                choice = input(f'Save it to .env as {env_var}? {default_hint("[Y/n]")}: ')
            except EOFError:
                # Nothing is reading the prompt, so nothing can confirm it either.
                choice = "n"
            save = choice.strip().lower() in {"", "y", "yes"}
        if save:
            try:
                backup = write_env_value(
                    env_var,
                    session_string,
                    _ENV_PATH,
                    expected=original,
                    replace_key=args.replace_key,
                )
                print("")
                event("INFO", "Session saved atomically")
                print(f".env updated: {env_var} is saved.")
                if backup:
                    print(hint(f"The previous file is kept as {backup.name}."))
            except Exception as e:
                print(
                    failure(
                        f"Saving failed ({type(e).__name__}). The original configuration is unchanged."
                    )
                )
                sys.exit(1)
            else:
                # Nothing follows. Until 2026-09-21 this signed the same account
                # in to a SECOND client as well, because secret chats ran on
                # a second authorisation. They run on this one now, so an account
                # that reaches here is finished - see docs/adr/0006.
                print()
                print(f"'{safe_label}' is saved. The server verifies it before activation.")
        elif args.label is not None:
            print(note("Nothing was saved."))
            sys.exit(1)

    except (Exception, KeyboardInterrupt) as e:
        print()
        print(failure(f"Login failed or was cancelled ({type(e).__name__}). Nothing was saved."))
        sys.exit(1)
    finally:
        if client is not None and not disconnected:
            try:
                client.disconnect()
            except Exception:
                print(failure("Disconnect could not be confirmed. No session was published."))


def main():
    from telegram_mcp.session_log import run_log

    with run_log("session_string_generator") as event:
        try:
            _main(event)
        except (EOFError, KeyboardInterrupt):
            print(failure("Cancelled. Nothing was saved."))
            sys.exit(1)


if __name__ == "__main__":
    main()
