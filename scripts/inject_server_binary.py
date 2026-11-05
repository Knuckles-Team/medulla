#!/usr/bin/env python3
"""Fold the native ``medulla-server`` executable into the medulla wheel.

The ``medulla`` wheel ships the server as package data at
``medulla/_bin/medulla-server`` (``medulla-server.exe`` on Windows), built for
the same platform as the wheel's extension modules. The release build compiles
the binary with the ``release-server`` profile (``scripts/build_release_server.sh``)
and this script places it in the already-built wheel, mode 0755, and rewrites
``RECORD``. The wheel is rewritten in place; an existing server member is
replaced.

Usage::

    python scripts/inject_server_binary.py <medulla_wheel> <server_binary>
"""

from __future__ import annotations

import sys
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from scripts.inject_numeric_kernel import _record_bytes

TARGET_DIR = "medulla/_bin"
SERVER_NAMES = ("medulla-server", "medulla-server.exe")
SERVER_MODE = 0o755


def _is_server_member(name: str) -> bool:
    directory, _, base = name.rpartition("/")
    return directory == TARGET_DIR and base in SERVER_NAMES


def inject(wheel: Path, binary: Path) -> str:
    if binary.name not in SERVER_NAMES or not binary.is_file():
        raise SystemExit(f"not a medulla-server executable: {binary.name}")
    data = binary.read_bytes()
    if not data:
        raise SystemExit("server executable is empty")
    target = f"{TARGET_DIR}/{binary.name}"
    with zipfile.ZipFile(wheel) as archive:
        infos = archive.infolist()
        names = [info.filename for info in infos]
        if len(names) != len(set(names)):
            raise SystemExit("wheel contains duplicate members")
        records = [i for i in infos if i.filename.endswith(".dist-info/RECORD")]
        if len(records) != 1:
            raise SystemExit("wheel must contain exactly one RECORD")
        record_info = records[0]
        comment = archive.comment
        entries = [
            (info, archive.read(info.filename))
            for info in infos
            if info is not record_info and not _is_server_member(info.filename)
        ]
        stamp = next(
            (i.date_time for i in infos if i.filename.startswith("medulla/")),
            record_info.date_time,
        )
    info = zipfile.ZipInfo(target, date_time=stamp)
    info.compress_type = zipfile.ZIP_DEFLATED
    info.create_system = 3  # Unix, so installers honour the mode bits
    info.external_attr = (0o100000 | SERVER_MODE) << 16
    entries.append((info, data))
    record = _record_bytes(entries, record_info.filename)
    temporary = wheel.with_suffix(".whl.tmp")
    with zipfile.ZipFile(temporary, "w", zipfile.ZIP_DEFLATED) as out:
        out.comment = comment
        for entry, payload in entries:
            out.writestr(entry, payload)
        out.writestr(record_info, record)
    temporary.replace(wheel)
    return target


def main(argv: list[str] | None = None) -> int:
    args = sys.argv[1:] if argv is None else argv
    if len(args) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    target = inject(Path(args[0]), Path(args[1]))
    print(f"OK: injected {target} (mode {SERVER_MODE:04o}) into the release wheel")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
