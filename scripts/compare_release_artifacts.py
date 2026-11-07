#!/usr/bin/env python3
"""Release fixed point: two builds of one tag must give identical artifacts.

MEDULLA-CONTRACT-R053.14. The release workflow builds the sdist and each
per-architecture server image twice (and the wheels twice on a manual run).
This check compares the two builds by artifact name and SHA-256 digest:

    compare_release_artifacts.py PRIMARY_DIR REPRODUCTION_DIR \
        --primary-images PRIMARY_DIGESTS --reproduction-images REPRODUCTION_DIGESTS

Each artifact directory holds the files of one build (``*.whl`` and
``*.tar.gz``). Each image digest file holds one ``<name> <digest>`` line per
image. A missing name, an extra name or a differing digest is a refusal that
prints the artifact name and both digests; the script then exits 1.
"""

from __future__ import annotations

import argparse
import sys
from collections.abc import Sequence
from hashlib import sha256
from pathlib import Path

ARTIFACT_SUFFIXES = (".whl", ".tar.gz")
ABSENT = "<absent>"


def file_digests(directory: Path) -> dict[str, str]:
    """Map each wheel and sdist file name in ``directory`` to its SHA-256."""
    digests: dict[str, str] = {}
    for path in sorted(directory.iterdir()):
        if path.is_file() and path.name.endswith(ARTIFACT_SUFFIXES):
            digests[path.name] = "sha256:" + sha256(path.read_bytes()).hexdigest()
    return digests


def image_digests(path: Path) -> dict[str, str]:
    """Parse ``<name> <digest>`` lines; a malformed or repeated line is refused."""
    digests: dict[str, str] = {}
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if not line.strip():
            continue
        parts = line.split()
        if len(parts) != 2 or parts[0] in digests:
            raise ValueError(
                f"{path.name}:{number}: expected one '<name> <digest>' line per image"
            )
        digests["image:" + parts[0]] = parts[1]
    return digests


def compare(primary: dict[str, str], reproduction: dict[str, str]) -> list[str]:
    """Return one refusal line per artifact whose digests differ or are absent."""
    refusals = []
    for name in sorted(primary.keys() | reproduction.keys()):
        left = primary.get(name, ABSENT)
        right = reproduction.get(name, ABSENT)
        if left != right:
            refusals.append(
                f"release artifact mismatch: {name} primary={left} reproduction={right}"
            )
    return refusals


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("primary", type=Path)
    parser.add_argument("reproduction", type=Path)
    parser.add_argument("--primary-images", type=Path, required=True)
    parser.add_argument("--reproduction-images", type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        primary = file_digests(args.primary) | image_digests(args.primary_images)
        reproduction = file_digests(args.reproduction) | image_digests(
            args.reproduction_images
        )
    except (OSError, ValueError) as error:
        print(f"release fixed point: {error}", file=sys.stderr)
        return 1
    if not primary:
        print("release fixed point: no artifacts to compare", file=sys.stderr)
        return 1
    refusals = compare(primary, reproduction)
    for refusal in refusals:
        print(refusal)
    if refusals:
        return 1
    for name in sorted(primary):
        print(f"identical: {name} {primary[name]}")
    print("release_fixed_point=passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
