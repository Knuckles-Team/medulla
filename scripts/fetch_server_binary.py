#!/usr/bin/env python3
"""Record and verify the per-platform release server executable.

``record`` writes ``<exe>.sha256`` next to the freshly built executable in a
directory; ``verify`` requires exactly one executable whose bytes match that
checksum, so every folded-wheel pass of a platform folds the same bytes.

Usage::

    python scripts/fetch_server_binary.py record dist-server-bin
    python scripts/fetch_server_binary.py verify dist-server-bin
"""

from __future__ import annotations

import hashlib
import sys
from pathlib import Path

SERVER_NAMES = ("medulla-server", "medulla-server.exe")


def _executable(directory: Path) -> Path:
    found = [directory / name for name in SERVER_NAMES if (directory / name).is_file()]
    if len(found) != 1:
        raise SystemExit(f"expected exactly one server executable in {directory}")
    return found[0]


def _digest(path: Path) -> str:
    data = path.read_bytes()
    if not data:
        raise SystemExit(f"server executable is empty: {path.name}")
    return hashlib.sha256(data).hexdigest()


def record(directory: Path) -> str:
    exe = _executable(directory)
    digest = _digest(exe)
    Path(f"{exe}.sha256").write_text(f"{digest}  {exe.name}\n", encoding="utf-8")
    return digest


def verify(directory: Path) -> str:
    exe = _executable(directory)
    sidecar = Path(f"{exe}.sha256")
    if not sidecar.is_file():
        raise SystemExit(f"missing checksum for {exe.name}")
    expected, _, name = sidecar.read_text(encoding="utf-8").strip().partition("  ")
    if name != exe.name or expected != _digest(exe):
        raise SystemExit(f"server executable does not match its checksum: {exe.name}")
    return expected


def main(argv: list[str]) -> int:
    if len(argv) != 3 or argv[1] not in ("record", "verify"):
        raise SystemExit(__doc__)
    digest = (record if argv[1] == "record" else verify)(Path(argv[2]))
    print(f"{argv[1]} {digest}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
