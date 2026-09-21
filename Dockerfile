# A Debian base, and the reason changed on 2026-09-21.
#
# It was a REQUIREMENT while a native dependency shipped manylinux wheels with no
# musllinux wheel and no source distribution: on Alpine the build failed at
# `uv sync` with "doesn't have a source distribution or wheel for the current
# platform", several minutes in. That dependency is gone with TDLib, and every
# remaining one is pure Python, so nothing here needs glibc any more.
#
# It stays `-slim` because a move to Alpine is an untested change with no measured
# benefit, not because it is still forced. What IS still required is `git`, for the
# encryption package's VCS pin - see the layer below, and
# tests/test_container_build.py, which asserts it.
FROM python:3.14-slim

# Set the working directory in the container
WORKDIR /app

# Prevent Python from writing pyc files to disc
ENV PYTHONDONTWRITEBYTECODE=1
# Ensure Python output is sent straight to terminal (useful for logs)
ENV PYTHONUNBUFFERED=1

# uv.lock is the single dependency source of truth for this project. The image
# used to install from the pip requirements file, which carries floors and not
# pins, so two images built from the same commit a week apart held different
# dependency trees, and a compromised release of Pillow or Telethon landed in
# the container with no commit and no lockfile diff.
#
# --locked, NOT --frozen. The comment that used to sit here said --frozen "fails
# loudly when uv.lock and pyproject.toml disagree"; it does not. --frozen means
# "install what the lock says and do not re-resolve", which is exactly how a
# manifest edited without a re-lock built a green image from a dependency set
# nobody had reviewed. --locked asserts the lock still describes this pyproject
# and fails when it does not, which is what that comment was promising.
# --no-install-project installs the dependencies only. main.py is run as a
# script from /app, so the project is never imported from site-packages, and
# building it here would need README.md, LICENSE and NOTICE in the context for
# nothing.
# UV_PYTHON_DOWNLOADS=never keeps uv on the base image's own interpreter instead
# of fetching a second managed CPython into an image chosen for being minimal.
# UV_PYTHON names the base image's interpreter outright rather than leaving uv to
# pick one: with DOWNLOADS=never there is only one to find today, but "there is
# only one" is a property of the image, not a promise about it.
ENV UV_PYTHON_DOWNLOADS=never
ENV UV_PYTHON=/usr/local/bin/python3
COPY pyproject.toml uv.lock ./
# uv itself is pinned. An unpinned resolver is a dependency of every version
# resolved below it, and the one input to this image that was still "whatever
# PyPI served that day" - the very thing the lockfile exists to stop.
# `git` is here for one requirement: the secret-chat package is pinned by git URL,
# because PyPI serves a DIFFERENT project under the same name (painor's archived
# `telethon-secret-chat`, at a higher version), so a bare requirement would install
# the wrong one and no version constraint would reveal it. `python:*-slim` ships no
# git, and uv answers "Git executable not found" rather than falling back. It is
# removed again in the same layer so the image does not carry a build tool it never
# runs.
RUN apt-get update \
    && apt-get install -y --no-install-recommends git \
    && rm -rf /var/lib/apt/lists/* \
    && pip install --no-cache-dir uv==0.12.8 \
    && uv sync --locked --no-dev --no-install-project \
    && apt-get purge -y git && apt-get autoremove -y

# uv puts the environment in /app/.venv, so putting it first on PATH is what makes
# the bare `python` in CMD below the interpreter holding the locked dependencies.
ENV PATH="/app/.venv/bin:$PATH"

# Copy the rest of the application code
COPY main.py ./
COPY telegram_mcp ./telegram_mcp
# COPY session_string_generator.py . # Optional: if needed within the container, otherwise can be run outside

# Sessions live OUTSIDE /app so a persistence mount cannot cover the application.
# Created here, before the user switch, so the bind-mount target already exists
# and is owned by the account that has to write the session database.
#
# `/data/state` is the second half, and without it the volume was persisting
# only one of the two things worth persisting. `state_dir()` resolves under
# `XDG_STATE_HOME`, which defaulted to the container user's home - so the
# Telethon session survived a container replacement while the secret-chat keys,
# the identity notes beside them, any quarantined database, the alias store and
# the event feed all went with the old container. Losing a key store is not
# a re-login: it takes the secret-chat keys, and those cannot be re-derived.
RUN mkdir -p /data /data/state

# Create a non-root user and switch to it
RUN adduser --disabled-password --gecos "" appuser && chown -R appuser:appuser /app /data
USER appuser

# Private: the state directory holds session files and secret-chat keys, each of
# which IS the account to whoever can read it.
RUN chmod 700 /data/state

VOLUME ["/data"]

# Define environment variables needed by the application
# These should be provided at runtime, not hardcoded (especially secrets)
ENV TELEGRAM_API_ID=""
ENV TELEGRAM_API_HASH=""
# Specify one of the following at runtime:
# Default session path. Absolute and under /data on purpose: a bare filename
# would land in WORKDIR and be lost on every container replacement.
ENV TELEGRAM_SESSION_NAME="/data/telegram_mcp_session"
# Everything `state_dir()` resolves - secret-chat keys, owner.json, quarantined
# databases, the alias store, the event feed, the error log - lands under the
# mounted volume rather than in the container's own filesystem.
ENV XDG_STATE_HOME="/data/state"
# Or provide the session string directly
ENV TELEGRAM_SESSION_STRING=""

# Expose any ports if the application were a web server (not needed for stdio MCP)
# EXPOSE 8000

# Define the command to run the application
CMD ["python", "main.py"]
