#!/usr/bin/env python3
"""Fail when a redb table is written outside the transaction crates.

MEDULLA-DURABLE-KERNEL-R043.102: every durable write goes through the
mutation envelope, which lives in ``crates/medulla-transaction`` and
``crates/medulla-storage``. Anywhere else under ``crates/`` or ``src/`` an
``open_table`` call on a write transaction is a direct table write and is
reported as ``SOLE_ENVELOPE_VIOLATION <file>:<line>``.

A write transaction is recognised syntactically: a binding taken from
``begin_write()``, a chained ``begin_write()?.open_table(``, or a parameter
typed ``WriteTransaction``. Known exceptions live in
``scripts/sole_write_envelope.allow`` as ``<path> <reason>`` lines; a line
without a reason, or one naming a path that no longer writes directly, fails
the gate so the allow file cannot rot.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
ALLOW = Path("scripts/sole_write_envelope.allow")
SCAN_ROOTS = ("crates", "src")
EXEMPT = ("crates/medulla-transaction/", "crates/medulla-storage/")
CODE = "SOLE_ENVELOPE_VIOLATION"

_LINE_COMMENT = re.compile(r"//.*$")
_WRITE_BINDING = re.compile(
    r"\blet\s+(?:mut\s+)?([a-z_][a-z0-9_]*)\s*(?::[^=]*)?=[^;]*\bbegin_write\s*\("
)
_WRITE_PARAM = re.compile(
    r"\b([a-z_][a-z0-9_]*)\s*:\s*&(?:mut\s+)?(?:[a-z_:]*::)?WriteTransaction\b"
)
_CHAINED = re.compile(r"\bbegin_write\s*\(\s*\)\s*\??\s*\.\s*open_table\s*\(")


def _code_lines(text: str) -> list[str]:
    return [_LINE_COMMENT.sub("", line) for line in text.splitlines()]


def scan_source(text: str) -> list[int]:
    """Return the 1-based lines of direct table writes in one Rust file."""
    lines = _code_lines(text)
    joined = "\n".join(lines)
    names = set(_WRITE_BINDING.findall(joined)) | set(_WRITE_PARAM.findall(joined))
    write_call = (
        re.compile(
            r"\b(?:"
            + "|".join(sorted(map(re.escape, names)))
            + r")\s*\.\s*open_table\s*\("
        )
        if names
        else None
    )
    hits: list[int] = []
    for number, line in enumerate(lines, start=1):
        if _CHAINED.search(line) or (write_call and write_call.search(line)):
            hits.append(number)
    return hits


def parse_allow(text: str) -> tuple[dict[str, str], list[str]]:
    """Parse ``<path> <reason>`` lines; return the entries and the errors."""
    entries: dict[str, str] = {}
    errors: list[str] = []
    for number, raw in enumerate(text.splitlines(), start=1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        path, _, reason = line.partition(" ")
        if not reason.strip():
            errors.append(f"{CODE} {ALLOW}:{number}: allow entry {path} has no reason")
            continue
        entries[path] = reason.strip()
    return entries, errors


def scan(root: Path, allow_text: str) -> list[str]:
    """Return every violation under ``root`` given the allow file text."""
    allowed, errors = parse_allow(allow_text)
    seen: set[str] = set()
    for scan_root in SCAN_ROOTS:
        base = root / scan_root
        if not base.is_dir():
            continue
        for path in sorted(base.rglob("*.rs")):
            relative = path.relative_to(root).as_posix()
            if relative.startswith(EXEMPT) or "/target/" in relative:
                continue
            hits = scan_source(path.read_text(encoding="utf-8", errors="replace"))
            if not hits:
                continue
            if relative in allowed:
                seen.add(relative)
                continue
            errors.extend(
                f"{CODE} {relative}:{line}: direct redb table write" for line in hits
            )
    errors.extend(
        f"{CODE} {ALLOW}: {path} no longer writes a table directly; remove it"
        for path in sorted(set(allowed) - seen)
    )
    return errors


def main() -> int:
    allow_path = ROOT / ALLOW
    allow_text = allow_path.read_text(encoding="utf-8") if allow_path.exists() else ""
    errors = scan(ROOT, allow_text)
    for error in errors:
        print(error, file=sys.stderr)
    return 1 if errors else 0


if __name__ == "__main__":
    raise SystemExit(main())
