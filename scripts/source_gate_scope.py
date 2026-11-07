#!/usr/bin/env python3
"""Avoid irrelevant local source scans only with complete, verified Git evidence."""

from __future__ import annotations

import argparse
import os
import re
import stat
import subprocess
import sys
from collections.abc import Callable
from pathlib import Path, PurePosixPath

ROOT_DOCUMENTS = frozenset(
    {"README.md", "CHANGELOG.md", "CONTRIBUTING.md", "AGENTS.md", "CLAUDE.md"}
)
SKILL = "medulla/skills/medulla-development/SKILL.md"
SCANNERS = {
    "durable-table-registration": "check_durable_table_registration.py",
}


def documentation_path(path: str) -> bool:
    """Only non-executable prose and generated spec status can bypass builds."""
    parts = PurePosixPath(path).parts
    if path in ROOT_DOCUMENTS:
        return True
    if len(parts) < 2 or parts[0] not in {"docs", "specs", ".specify", "architecture"}:
        return False
    return path.endswith(".md") or (
        len(parts) == 3 and parts[0] == "specs" and parts[2] == "status.json"
    )


def _raw_entry_is_documentation(
    metadata_field: bytes, path_field: bytes, path_predicate: Callable[[str], bool]
) -> bool:
    """One raw diff entry: a plain-mode A/M/D change of a documentation path."""
    metadata = metadata_field.split()
    if len(metadata) != 5 or not metadata[0].startswith(b":"):
        return False
    modes = {metadata[0][1:], metadata[1]}
    if not modes <= {b"100644", b"000000"} or b"100644" not in modes:
        return False
    if metadata[4] not in {b"A", b"M", b"D"}:
        return False
    try:
        path = path_field.decode("utf-8")
    except UnicodeDecodeError:
        return False
    return path_predicate(path)


def documentation_diff(
    raw: bytes, path_predicate: Callable[[str], bool] = documentation_path
) -> bool:
    """Read --raw -z --no-renames, retaining deletions and executable modes."""
    fields = raw.split(b"\0")
    if fields[-1] != b"" or len(fields) < 3 or len(fields) % 2 != 1:
        return False
    return all(
        _raw_entry_is_documentation(fields[index], fields[index + 1], path_predicate)
        for index in range(0, len(fields) - 1, 2)
    )


def local_document(path: str) -> bool:
    return documentation_path(path) or path == SKILL


def git(*args: str) -> bytes:
    return subprocess.check_output(["git", *args], stderr=subprocess.DEVNULL)


def verified_range() -> list[str]:
    """Require a complete immutable range and its clean checked-out tip."""
    if git("status", "--porcelain", "-z", "--untracked-files=no"):
        raise ValueError("tracked worktree is dirty")
    base = os.environ.get("PRE_COMMIT_FROM_REF", "")
    head = os.environ.get("PRE_COMMIT_TO_REF", "")
    for ref in (base, head):
        if not re.fullmatch(r"[0-9a-f]{40}", ref):
            raise ValueError("unknown range ref")
        git("rev-parse", "--verify", f"{ref}^{{commit}}")
    if git("rev-parse", "HEAD").decode().strip() != head:
        raise ValueError("checkout is not range tip")
    return [base, head]


def verified_diff(scope: str) -> bytes:
    """Explicit commit ranges share push validation; ordinary commits use index."""
    if scope not in {"staged", "push"}:
        raise ValueError("full scan mode")
    has_range = any(
        key in os.environ for key in ("PRE_COMMIT_FROM_REF", "PRE_COMMIT_TO_REF")
    )
    if scope == "push" or has_range:
        refs = verified_range()
    else:
        git("rev-parse", "--verify", "HEAD^{commit}")
        if git("diff", "--raw", "--no-renames", "--ignore-submodules=none", "-z"):
            raise ValueError("index/worktree disagree")
        refs = ["--cached", "HEAD"]
    return git("diff", "--raw", "--no-renames", "-z", *refs)


def ordinary_documents(filenames: list[str]) -> bool:
    for name in filenames:
        if not local_document(name):
            return False
        mode = Path(name).lstat().st_mode
        if not stat.S_ISREG(mode) or mode & 0o111:
            return False
    return bool(filenames)


def matching_documents(raw: bytes, filenames: list[str]) -> bool:
    if not documentation_diff(raw, local_document):
        return False
    fields = raw.split(b"\0")
    if any(fields[i].split()[-1] == b"D" for i in range(0, len(fields) - 1, 2)):
        return False
    paths = {fields[i].decode() for i in range(1, len(fields) - 1, 2)}
    supplied = set(filenames)
    tracked = set(git("ls-files", "-z").decode().rstrip("\0").split("\0"))
    return paths == supplied and supplied != tracked


def can_skip(scope: str, filenames: list[str]) -> bool:
    if any(key in os.environ for key in ("CI", "GITHUB_ACTIONS", "GITHUB_EVENT_NAME")):
        return False
    try:
        if not ordinary_documents(filenames):
            return False
        flags = git("ls-files", "-v", "-z").split(b"\0")
        if any(item and not item.startswith(b"H ") for item in flags):
            return False
        raw = verified_diff(scope)
        return matching_documents(raw, filenames)
    except (OSError, subprocess.CalledProcessError, ValueError, UnicodeError):
        return False


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--scope", required=True, choices=("staged", "push", "manual", "all-files")
    )
    parser.add_argument("--scanner", required=True, choices=SCANNERS)
    parser.add_argument("filenames", nargs="*")
    args = parser.parse_args()
    if can_skip(args.scope, args.filenames):
        print(f"{args.scanner}: verified local docs-only change; scan unnecessary")
        return 0
    scanner = Path(__file__).with_name(SCANNERS[args.scanner])
    return subprocess.call([sys.executable, str(scanner)])


if __name__ == "__main__":
    raise SystemExit(main())
