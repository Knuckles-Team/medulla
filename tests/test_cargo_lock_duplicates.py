"""MEDULLA-CONTRACT-R065: one dependency family, one TLS backend, reasoned duplicates.

`Cargo.lock` holds one version of each columnar-array crate, selects `ring` and no
`aws-lc-sys`, keeps the vendored security patches as path packages, and holds every
other multi-version package only when `scripts/cargo_lock_duplicates.toml` names it
with the pinning dependents and a reason. The allow-list only shrinks: a stale entry
fails, and an entry the merge base did not hold fails.
"""

from __future__ import annotations

import shutil
import subprocess
from collections import defaultdict
from pathlib import Path

import pytest
import tomllib

pytestmark = pytest.mark.no_engine

ROOT = Path(__file__).resolve().parents[1]
LOCK = ROOT / "Cargo.lock"
ALLOW = ROOT / "scripts" / "cargo_lock_duplicates.toml"

ARRAY_CRATES = (
    "arrow",
    "arrow-arith",
    "arrow-array",
    "arrow-buffer",
    "arrow-cast",
    "arrow-data",
    "arrow-ord",
    "arrow-row",
    "arrow-schema",
    "arrow-select",
    "arrow-string",
    "arrow-ipc",
    "parquet",
)
DUCKDB_VERSION = "1.10506.1"
PATCHED = (
    "apache-avro",
    "arrow-ipc",
    "iceberg",
    "parquet",
    "polars-parquet",
    "tantivy-sstable",
    "opendal-core",
    "opendal-service-s3",
    "iceberg-storage-opendal",
)
# The Kafka client's C library has no other TLS library. The second route is the
# build-time download of `ort` (pulled by `piper-rs`, whose own manifest keeps the
# default `tls-native`), which only the default-off `medulla-tts-piper` crate reaches.
OPENSSL_DEPENDENTS = {"rdkafka-sys", "native-tls", "openssl"}
NATIVE_TLS_DEPENDENTS = {"ureq"}


def _cargo_usable() -> bool:
    """Cargo runs here (a host without cargo, or a refusing shim, skips the closure)."""
    if shutil.which("cargo") is None:
        return False
    probe = subprocess.run(["cargo", "--version"], capture_output=True, text=True)
    return probe.returncode == 0


def _packages(text: str) -> list[dict]:
    return tomllib.loads(text)["package"]


def _versions(packages: list[dict]) -> dict[str, set[str]]:
    found: dict[str, set[str]] = defaultdict(set)
    for package in packages:
        found[package["name"]].add(package["version"])
    return found


def _lock_packages() -> list[dict]:
    return _packages(LOCK.read_text(encoding="utf-8"))


def _duplicates(packages: list[dict]) -> dict[str, set[str]]:
    return {n: v for n, v in _versions(packages).items() if len(v) > 1}


def _dependents(packages: list[dict], name: str) -> set[str]:
    out: set[str] = set()
    for package in packages:
        for dep in package.get("dependencies", []):
            if dep.split(" ")[0] == name:
                out.add(package["name"])
    return out


def _allow_list() -> dict[str, dict]:
    return tomllib.loads(ALLOW.read_text(encoding="utf-8"))


def check_duplicates(
    packages: list[dict], allow: dict[str, dict], base_names: set[str] | None
) -> list[str]:
    """Return the failures of the three duplicate rules (R065.4)."""
    problems: list[str] = []
    dups = _duplicates(packages)
    for name in sorted(set(dups) - set(allow)):
        problems.append(
            f"duplicate package outside the allow-list: {name} {sorted(dups[name])}"
        )
    for name in sorted(set(allow) - set(dups)):
        problems.append(f"stale allow-list entry (no longer a duplicate): {name}")
    if base_names is not None:
        for name in sorted(set(allow) - base_names):
            problems.append(f"allow-list entry added since the merge base: {name}")
    return problems


def _base_allow_names() -> set[str] | None:
    """Entry names at the merge base, or None when the base has no list yet."""
    try:
        base = subprocess.run(
            ["bash", str(ROOT / "scripts" / "gate_base_ref.sh")],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
        shown = subprocess.run(
            ["git", "show", f"{base}:scripts/cargo_lock_duplicates.toml"],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
        ).stdout
    except (subprocess.CalledProcessError, FileNotFoundError):
        return None
    return set(tomllib.loads(shown))


# spec: MEDULLA-CONTRACT-R065.1
def test_one_columnar_array_family() -> None:
    packages = _lock_packages()
    versions = _versions(packages)
    for name in ARRAY_CRATES:
        assert len(versions.get(name, set())) == 1, (
            f"{name} must have exactly one version: {versions.get(name)}"
        )
    (arrow,) = versions["arrow"]
    for name in ("parquet", "arrow-ipc"):
        assert versions[name] == {arrow}, (
            f"{name} {versions[name]} must equal arrow {arrow}"
        )
    assert versions["duckdb"] == {DUCKDB_VERSION}, versions["duckdb"]


# spec: MEDULLA-CONTRACT-R065.2
def test_one_tls_and_crypto_backend() -> None:
    packages = _lock_packages()
    versions = _versions(packages)
    assert "ring" in versions, "ring is the one TLS and crypto backend"
    assert "aws-lc-sys" not in versions, "aws-lc-sys must not be in the lock"
    assert "aws-lc-rs" not in versions, "aws-lc-rs must not be in the lock"
    if "openssl-sys" in versions:
        dependents = _dependents(packages, "openssl-sys")
        assert dependents <= OPENSSL_DEPENDENTS, (
            f"openssl-sys dependents: {sorted(dependents)}"
        )
        native = _dependents(packages, "native-tls")
        assert native <= NATIVE_TLS_DEPENDENTS, sorted(native)
    if not _cargo_usable():
        return
    for crate in ("openssl-sys", "aws-lc-sys"):
        for features in ("full", "default"):
            args = ["cargo", "tree", "--locked", "-e", "normal", "-i", crate]
            if features == "full":
                args += ["--features", "full"]
            proc = subprocess.run(args, cwd=ROOT, capture_output=True, text=True)
            assert proc.returncode != 0 and "did not match" in proc.stderr, (
                f"{crate} is in the {features} closure:\n{proc.stdout[:800]}"
            )
