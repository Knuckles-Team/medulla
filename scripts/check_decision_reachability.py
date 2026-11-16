#!/usr/bin/env python3
"""Fail closed when a public decision item has no production caller.

Every public item declared in the decision crates (``medulla-decision-types``,
the ``solve`` module of ``medulla-decision-policy`` and the ``assemble`` module
of ``medulla-compute``) must be named by non-test code somewhere under
``crates/*/src/`` outside its own declaration. An item used only by tests, or by
nothing, is dead weight: wire it, move it to a ``test_support`` module, or delete
it. The gate has no baseline file and no allowlist.

The universe (roots, files scanned, public items found, production files read)
is printed on every run, and an empty universe fails (MEDULLA-CONTRACT-R055).
"""

from __future__ import annotations

import re
import sys
from collections.abc import Iterable
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

SCAN_ROOTS = (
    "crates/medulla-decision-types/src",
    "crates/medulla-decision-policy/src/solve",
    "crates/medulla-compute/src/assemble",
)

_ITEM = re.compile(
    r"^\s*pub\s+(?:async\s+)?(?:const\s+)?(?:unsafe\s+)?"
    r"(?:fn|struct|enum|trait|const|static|type)\s+([A-Za-z_][A-Za-z0-9_]*)"
)
_CFG_TEST = re.compile(r"^\s*#\[cfg\((?:test|all\(test[^)]*\))\)\]")
_IDENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")


@dataclass(frozen=True)
class Item:
    name: str
    path: str
    line: int


def is_test_file(path: Path, root: Path = ROOT) -> bool:
    parts = path.relative_to(root).parts
    name = path.name
    return (
        "tests" in parts
        or "benches" in parts
        or "test_support" in parts
        or any(part.endswith("_fixtures") for part in parts)
        or name in {"tests.rs", "test_support.rs"}
        or name.endswith("_tests.rs")
        or name.endswith("_test.rs")
    )


def production_lines(text: str) -> list[tuple[int, str]]:
    """Return (line number, text) for lines outside any ``#[cfg(test)]`` item."""
    lines = text.splitlines()
    kept: list[tuple[int, str]] = []
    i = 0
    while i < len(lines):
        if _CFG_TEST.match(lines[i]):
            depth = 0
            seen_open = False
            j = i + 1
            while j < len(lines):
                depth += lines[j].count("{") - lines[j].count("}")
                seen_open = seen_open or "{" in lines[j]
                if (seen_open and depth <= 0) or (not seen_open and ";" in lines[j]):
                    break
                j += 1
            i = j + 1
            continue
        kept.append((i + 1, lines[i]))
        i += 1
    return kept


def rust_files(directory: Path) -> Iterable[Path]:
    return sorted(p for p in directory.rglob("*.rs") if p.is_file())


def declared_items(root: Path = ROOT) -> list[Item]:
    items: list[Item] = []
    for scan in SCAN_ROOTS:
        base = root / scan
        for path in rust_files(base):
            if is_test_file(path, root):
                continue
            rel = str(path.relative_to(root))
            for number, line in production_lines(path.read_text(encoding="utf-8")):
                match = _ITEM.match(line)
                if match:
                    items.append(Item(match.group(1), rel, number))
    return items


def production_mentions(
    root: Path = ROOT,
) -> tuple[dict[str, set[tuple[str, int]]], int]:
    """Map each identifier to the (file, line) pairs naming it in production code."""
    mentions: dict[str, set[tuple[str, int]]] = {}
    files = 0
    production_roots = sorted((root / "crates").glob("*/src"))
    build_scripts = sorted((root / "crates").glob("*/build.rs"))
    for src in production_roots:
        for path in [
            *rust_files(src),
            *[b for b in build_scripts if b.parent == src.parent],
        ]:
            if is_test_file(path, root):
                continue
            files += 1
            rel = str(path.relative_to(root))
            for number, line in production_lines(path.read_text(encoding="utf-8")):
                code = line.split("//", 1)[0]
                for ident in _IDENT.findall(code):
                    mentions.setdefault(ident, set()).add((rel, number))
    return mentions, files


def find_orphans(root: Path = ROOT) -> tuple[list[Item], int, int]:
    items = declared_items(root)
    mentions, files = production_mentions(root)
    orphans = [
        item
        for item in items
        if not (mentions.get(item.name, set()) - {(item.path, item.line)})
    ]
    return orphans, len(items), files


def main() -> int:
    orphans, declared, files = find_orphans()
    print(
        f"decision reachability gate: {declared} public item(s) in "
        f"{len(SCAN_ROOTS)} root(s), {files} production file(s) read"
    )
    if declared == 0 or files == 0:
        print(
            "FAIL: empty universe: the scan roots drifted or the crates were deleted",
            file=sys.stderr,
        )
        return 1
    if orphans:
        print(
            f"FAIL: {len(orphans)} public item(s) have no production caller:",
            file=sys.stderr,
        )
        for item in orphans:
            print(f"  {item.path}:{item.line} {item.name}", file=sys.stderr)
        return 1
    print("OK: every public decision item has a production caller")
    return 0


if __name__ == "__main__":
    sys.exit(main())
