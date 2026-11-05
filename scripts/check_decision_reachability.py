#!/usr/bin/env python3
"""Fail when a public decision item is named by no non-test code.

MEDULLA-CONTRACT-R055. Every public item declared in the decision type,
decision-policy solve and compute assemble trees must be named somewhere in
non-test code under ``crates/*/src/``, outside its own declaration. The six
decision-plane served methods (``RetrievalRound``, ``ViewportQuery``,
``ResolveViewportSelection``, ``ApproveSourceQuery``,
``VerifySourceQueryApproval``, ``GraphLearnPredictNary``) fall under the same
rule: once declared, each must be named by non-test code outside the
protocol method declarations. There is no baseline file: every orphan fails the gate.

The scan counts identifier tokens once over the whole non-test source set, so
an item is reachable exactly when its name occurs more often than it is
declared. Test code (``tests.rs``, ``*_tests.rs``, ``tests/`` directories and
``#[cfg(test)]`` modules) never makes an item reachable.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from collections import Counter
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCHEMA = "medulla-decision-reachability/v1"

SCANNED_TREES = (
    "crates/medulla-decision-types/src",
    "crates/medulla-decision-policy/src/solve",
    "crates/medulla-compute/src/assemble",
)
METHOD_TREE = "crates/medulla-protocol-types/src/protocol/method"
NAMED_METHODS = (
    "RetrievalRound",
    "ViewportQuery",
    "ResolveViewportSelection",
    "ApproveSourceQuery",
    "VerifySourceQueryApproval",
    "GraphLearnPredictNary",
)

_PUBLIC_ITEM = re.compile(
    r"^\s*pub\s+(?:(?:async|const|unsafe|extern\s+\"[^\"]*\")\s+)*"
    r"(?:fn|struct|enum|trait|type|const|static|union)\s+([A-Za-z_][A-Za-z0-9_]*)",
    re.MULTILINE,
)
_IDENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
_LINE_COMMENT = re.compile(r"//[^\n]*")
_BLOCK_COMMENT = re.compile(r"/\*.*?\*/", re.DOTALL)
_CFG_TEST_MOD = re.compile(
    r"#\[cfg\(test\)\]\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*\{"
)


class GateError(RuntimeError):
    """The gate could not establish its universe and must not report green."""


@dataclass(frozen=True)
class Item:
    name: str
    path: str


def is_test_path(path: Path) -> bool:
    parts = path.parts
    if "tests" in parts or "test_support" in parts:
        return True
    name = path.name
    return name == "tests.rs" or name.endswith("_tests.rs") or name == "test.rs"


def strip_comments(text: str) -> str:
    return _LINE_COMMENT.sub("", _BLOCK_COMMENT.sub("", text))


def strip_test_modules(text: str) -> str:
    """Remove inline ``#[cfg(test)] mod x { ... }`` bodies by brace matching."""
    out: list[str] = []
    cursor = 0
    while True:
        match = _CFG_TEST_MOD.search(text, cursor)
        if match is None:
            out.append(text[cursor:])
            return "".join(out)
        out.append(text[cursor : match.start()])
        depth = 1
        index = match.end()
        while index < len(text) and depth:
            char = text[index]
            if char == "{":
                depth += 1
            elif char == "}":
                depth -= 1
            index += 1
        cursor = index


def production_text(path: Path) -> str:
    return strip_test_modules(strip_comments(path.read_text(encoding="utf-8")))


def production_files(root: Path) -> list[Path]:
    crates = root / "crates"
    if not crates.is_dir():
        raise GateError(f"no crates directory under {root}")
    return sorted(
        path
        for path in crates.glob("*/src/**/*.rs")
        if not is_test_path(path.relative_to(crates))
    )


def declared_items(root: Path, files: list[Path]) -> list[Item]:
    items: list[Item] = []
    for tree in SCANNED_TREES:
        base = root / tree
        if not base.is_dir():
            raise GateError(f"scanned tree is missing: {tree}")
        for path in files:
            if base in path.parents:
                rel = path.relative_to(root).as_posix()
                for match in _PUBLIC_ITEM.finditer(production_text(path)):
                    items.append(Item(match.group(1), rel))
    return items


def find_orphans(root: Path = ROOT) -> list[Item]:
    files = production_files(root)
    tokens: Counter[str] = Counter()
    declarations: Counter[str] = Counter()
    method_tokens: Counter[str] = Counter()
    method_declared: Counter[str] = Counter()
    method_base = root / METHOD_TREE
    for path in files:
        text = production_text(path)
        names = _IDENT.findall(text)
        tokens.update(names)
        if method_base in path.parents:
            method_declared.update(names)
        else:
            method_tokens.update(names)
        for match in _PUBLIC_ITEM.finditer(text):
            declarations[match.group(1)] += 1
    orphans = [
        item
        for item in declared_items(root, files)
        if tokens[item.name] <= declarations[item.name]
    ]
    for method in NAMED_METHODS:
        if method_declared[method] and method_tokens[method] == 0:
            orphans.append(Item(method, METHOD_TREE))
    return sorted(set(orphans), key=lambda item: (item.path, item.name))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument("--json", action="store_true", help="print a JSON report")
    args = parser.parse_args(argv)
    try:
        orphans = find_orphans(args.root)
    except (GateError, OSError, UnicodeDecodeError) as exc:
        print(f"decision reachability gate could not run: {exc}", file=sys.stderr)
        return 2
    if args.json:
        report = {
            "schema": SCHEMA,
            "orphans": [{"name": o.name, "path": o.path} for o in orphans],
        }
        print(json.dumps(report, indent=2, sort_keys=True))
    for orphan in orphans:
        print(
            f"ORPHAN_DECISION_ITEM: {orphan.path}: {orphan.name} is named by "
            "no non-test code under crates/*/src/",
            file=sys.stderr,
        )
    if orphans:
        print(f"{len(orphans)} unreachable decision item(s)", file=sys.stderr)
        return 1
    print("decision reachability: every public decision item is reachable")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
