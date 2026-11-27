#!/usr/bin/env python3
"""Fetch the pinned prebuilt embedded analytical database library.

MEDULLA-CONTRACT-R066.2. Usage::

    python3 scripts/fetch_duckdb_lib.py --target <rust target triple> --out <dir>

The archive named by ``scripts/duckdb-pin.toml`` is downloaded, its SHA-256 is
compared with the pinned value (a mismatch refuses with ``DUCKDB_CHECKSUM_MISMATCH``
and writes nothing), the archive is unpacked into ``<dir>`` and the directory is
printed: it is the value of ``DUCKDB_LIB_DIR`` (and ``DUCKDB_INCLUDE_DIR``).

``--archive <zip>`` verifies and unpacks a local archive instead of downloading it,
and ``--pin <toml>`` reads another pin file; both exist so the refusal is testable
without a network. The script runs on the oldest interpreter a release container
ships, so it uses the standard library only and does not parse TOML with a library.
"""

import argparse
import hashlib
import os
import re
import sys
import urllib.request
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT_PIN = os.path.join(HERE, "duckdb-pin.toml")
MISMATCH = "DUCKDB_CHECKSUM_MISMATCH"
_ASSIGN = re.compile(r'^\s*([A-Za-z0-9_.-]+)\s*=\s*"([^"]*)"\s*(?:#.*)?$')
_SECTION = re.compile(r"^\s*\[([A-Za-z0-9_.-]+)\]\s*$")


def read_pin(path):
    """Return the pin as ``{section: {key: value}}``; the top level is ``""``."""
    pin: dict[str, dict[str, str]] = {"": {}}
    section = ""
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            if not line.strip() or line.lstrip().startswith("#"):
                continue
            match = _SECTION.match(line)
            if match:
                section = match.group(1)
                pin.setdefault(section, {})
                continue
            match = _ASSIGN.match(line)
            if not match:
                raise SystemExit("unreadable pin line: " + line.strip())
            pin[section][match.group(1)] = match.group(2)
    return pin


def platform_for(pin, target):
    platform = pin.get("targets", {}).get(target)
    if platform is None:
        raise SystemExit("no prebuilt library is pinned for target " + target)
    return platform


def sha256_of(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def unpack(archive, out):
    os.makedirs(out, exist_ok=True)
    root = os.path.realpath(out)
    with zipfile.ZipFile(archive) as bundle:
        for name in bundle.namelist():
            if not os.path.realpath(os.path.join(root, name)).startswith(root + os.sep):
                raise SystemExit("archive member escapes the output directory: " + name)
        bundle.extractall(root)


def fetch(pin, target, out, archive=None):
    platform = platform_for(pin, target)
    expected = pin.get("checksums", {}).get(platform)
    if not expected:
        raise SystemExit("no checksum is pinned for " + platform)
    downloaded = None
    if archive is None:
        url = pin[""]["url"].format(version=pin[""]["version"], platform=platform)
        os.makedirs(out, exist_ok=True)
        downloaded = os.path.join(out, "libduckdb-" + platform + ".zip")
        with urllib.request.urlopen(url) as response, open(downloaded, "wb") as sink:
            sink.write(response.read())
        archive = downloaded
    actual = sha256_of(archive)
    if actual != expected:
        if downloaded is not None:
            os.remove(downloaded)
        raise SystemExit(
            f"{MISMATCH}: {platform} is {actual}, the pin holds {expected}"
        )
    unpack(archive, out)
    if downloaded is not None:
        os.remove(downloaded)
    return os.path.abspath(out)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("--target", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--pin", default=DEFAULT_PIN)
    parser.add_argument("--archive")
    args = parser.parse_args(argv)
    print(fetch(read_pin(args.pin), args.target, args.out, args.archive))
    return 0


if __name__ == "__main__":
    sys.exit(main())
