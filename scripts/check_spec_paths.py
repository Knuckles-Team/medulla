#!/usr/bin/env python3
"""Fail when a spec row names a missing repository file (MEDULLA-CONTRACT-R056.10).

``spec_rows_name_existing_files`` walks ``specs/*/requirements.md`` and
``specs/*/test-spec.md`` and returns one finding per backticked repository
path (``crates/``, ``scripts/``, ``tests/``, ``src/``, ``benches/``,
``python/``, ``.github/``; a trailing ``:line`` or ``::test`` is stripped)
that does not exist. A path the row itself marks as new (``(new``
directly after it) is a planned file and is not checked. A move that renames
a file must rewrite every row naming it in the same change.

Paths are recognised by the same rule ``scripts/check_public_specs.py``
uses for repository paths: a parent path segment marks a repository path.

Usage: check_spec_paths.py [--root PATH]. Exit status 0 when every path exists.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SPEC_FILES = ("requirements.md", "test-spec.md")
REPO_PATH = re.compile(
    r"`((?:crates|scripts|tests|src|benches|python|\.github)/[A-Za-z0-9_./-]+?)"
    r"(?::\d+)?(?:::[A-Za-z0-9_:]+)?`"
)
NEW_MARKER = re.compile(r"\s*\(new\b")


def spec_rows_name_existing_files(root: Path = ROOT) -> list[str]:
    """R056.10: every repository path a spec row names exists."""
    # spec: MEDULLA-CONTRACT-R056.10
    findings: list[str] = []
    for spec_file in sorted((root / "specs").glob("*/*.md")):
        if spec_file.name not in SPEC_FILES:
            continue
        rel_spec = spec_file.relative_to(root)
        lines = spec_file.read_text(encoding="utf-8").splitlines()
        for lineno, line in enumerate(lines, start=1):
            for match in REPO_PATH.finditer(line):
                if NEW_MARKER.match(line, match.end()):
                    continue
                path = match.group(1).rstrip("/.")
                if not (root / path).exists():
                    findings.append(f"{rel_spec}:{lineno}: names missing file {path}")
    return findings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--root", type=Path, default=ROOT)
    args = parser.parse_args(argv)
    findings = spec_rows_name_existing_files(args.root)
    for finding in findings:
        print(f"FAIL {finding}")
    if findings:
        print(f"{len(findings)} spec path(s) name missing files")
        return 1
    print("spec paths: every named file exists")
    return 0


if __name__ == "__main__":
    sys.exit(main())
