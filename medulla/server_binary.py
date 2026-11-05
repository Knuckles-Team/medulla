"""Locate the ``medulla-server`` executable shipped in the ``medulla`` wheel.

Every published ``medulla`` wheel carries the native server as package data at
``medulla/_bin/medulla-server`` (``medulla-server.exe`` on Windows), built for
the wheel's own platform. :func:`resolve_server_binary` returns that packaged
path and nothing else. An install without it (the source distribution, which
builds the extension modules only, or a source checkout) raises
:class:`ServerBinaryNotFoundError`.
"""

from __future__ import annotations

import os
from pathlib import Path

SERVER_BINARY_NAME = "medulla-server"
PACKAGED_BINARY_DIR = "_bin"


class ServerBinaryNotFoundError(RuntimeError):
    """The packaged medulla-server executable is absent or not executable."""


def packaged_server_binary() -> Path:
    """Return where the wheel places the server executable for this platform."""
    suffix = ".exe" if os.name == "nt" else ""
    return (
        Path(__file__).resolve().parent
        / PACKAGED_BINARY_DIR
        / (SERVER_BINARY_NAME + suffix)
    )


def resolve_server_binary() -> str:
    """Return the path of the packaged server executable or raise."""
    path = packaged_server_binary()
    if not path.is_file():
        raise ServerBinaryNotFoundError(
            f"{SERVER_BINARY_NAME} is not in this medulla install ({path} is "
            "missing). The server ships inside the platform wheels: install a "
            "wheel with `pip install --only-binary medulla medulla`."
        )
    if os.name != "nt" and not os.access(path, os.X_OK):
        raise ServerBinaryNotFoundError(
            f"{SERVER_BINARY_NAME} at {path} is not executable; reinstall the "
            "medulla wheel."
        )
    return str(path)
