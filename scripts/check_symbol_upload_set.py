#!/usr/bin/env python3
"""Check the symbol upload set of a release (MEDULLA-CONTRACT-R060.2).

    check_symbol_upload_set.py <dir> <name>...

Every required platform <name> must have a ``symbols-<name>-<build-id>.tar.zst`` and
its ``.sha256`` somewhere under <dir>. A missing leg exits 1 with
``SYMBOL_ARTIFACT_MISSING: <name>``.
"""

from __future__ import annotations

import sys
from pathlib import Path


def missing_legs(directory: Path, names: list[str]) -> list[str]:
    """The required platform names with no archive plus checksum under `directory`."""
    absent = []
    for name in names:
        found = [
            archive
            for archive in directory.rglob(f"symbols-{name}-*.tar.zst")
            if archive.with_name(archive.name + ".sha256").is_file()
        ]
        if not found:
            absent.append(name)
    return absent


def main(argv: list[str]) -> int:
    if len(argv) < 3:
        print("usage: check_symbol_upload_set.py <dir> <name>...", file=sys.stderr)
        return 2
    absent = missing_legs(Path(argv[1]), argv[2:])
    for name in absent:
        print(f"SYMBOL_ARTIFACT_MISSING: {name}", file=sys.stderr)
    return 1 if absent else 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
