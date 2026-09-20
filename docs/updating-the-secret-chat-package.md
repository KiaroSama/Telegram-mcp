# Updating the secret-chat package

Secret chats run on [`KiaroSama/Telethon-Secret-Chat`](https://github.com/KiaroSama/Telethon-Secret-Chat),
which is maintained in its own repository and released on its own schedule. This is how
this project takes a newer version of it.

## The short version

1. Edit one line in `pyproject.toml` — the `@<ref>` at the end of the
   `telethon-secret-chat` requirement — to the tag or commit you want.
2. `uv lock --upgrade-package telethon-secret-chat`
3. Commit both files, push, read CI.

Nothing in `telegram_mcp/` should need to change. If something does, CI says so and
names it, and the change is in one file.

## Why it is pinned by git and not by name

**PyPI already serves a different package called `telethon-secret-chat`.** It is
painor's archived project, published at **0.2.4** — a *higher* version than this one's
0.0.1. A bare `telethon-secret-chat>=...` requirement therefore resolves to the wrong
project, installs cleanly, and nothing about the version number reveals the
substitution.

So the requirement names the repository:

```toml
"telethon-secret-chat @ git+https://github.com/KiaroSama/Telethon-Secret-Chat.git@<ref>"
```

`<ref>` is a tag once the package has releases, and a commit SHA before then. Either
way the resolved commit is recorded in `uv.lock`, so the version actually in force is
visible in one place and cannot drift.

`tests/test_secret_backend_contract.py` asserts the installed distribution really came
from that repository, by reading the `direct_url.json` the installer writes. A machine
that already had the archived package fails that test rather than shadowing the real one
in silence.

## Why no source change should be needed

Two properties, both enforced by tests rather than by convention:

**One importer.** `telegram_mcp/secret_backend.py` is the only module in this project
that imports `telethon_secret_chat`. Every secret-chat tool goes through
`secret_manager(account)`. A test walks the package's syntax tree and fails if any other
module imports it, so an upstream signature change stays a one-file edit instead of
becoming a search across nineteen call sites.

**Written-down assumptions.** The operations this project calls, the manager's
constructor shape, the storage backend's shape and the package's public exports are all
asserted in `tests/test_secret_backend_contract.py`. They do not test the package's
correctness — its own suite owns that. They test the boundary, so a version that renamed
`flush_history` or dropped `send_file` fails in CI naming what it broke, rather than
raising `AttributeError` inside a tool at whatever moment the operator happened to use
that path.

One of those tests deliberately builds an incompatible stand-in and asserts the guard
catches it. A guard nobody has watched fail is a guard nobody knows works.

## When CI does go red after an update

Read which contract test failed; it names the operation or the signature that changed.
Then:

- **An operation was renamed or its arguments moved** — adapt
  `telegram_mcp/secret_backend.py`, and update `CALLED_OPERATIONS` in the contract test
  in the same commit.
- **`storage` gained a default** — do not accept it silently. The package refusing to
  guess where key material goes is a safety property, and a default would mean this
  server starts writing keys somewhere the operator did not choose.
- **The distribution check failed** — the pin did not hold and something installed the
  archived package from the index. Re-run the lock command; do not "fix" the test.
- **Nothing is obviously wrong** — pin back to the previous ref, which is one line, and
  reproduce against the package's own suite in its repository.

## What this project does NOT do

It does not vendor the package, patch it at runtime, or work around a defect in it. A
defect belongs upstream, in its own repository, with its own test. Pinning back is the
correct response here while that happens.
