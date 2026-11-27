"""MEDULLA-CONTRACT-R066: the embedded analytical database stays out of dev builds.

The feature `source-duckdb` is outside `default`, `full` and `cluster`; only the
serving and release builds enable it.
"""

from __future__ import annotations

import hashlib
import re
import subprocess
import sys
import zipfile
from pathlib import Path

import pytest
import tomllib

pytestmark = pytest.mark.no_engine

ROOT = Path(__file__).resolve().parents[1]
SERVING_SCRIPTS = (
    "scripts/build_release_server.sh",
    "scripts/promote_engine.sh",
    "scripts/wheel_privacy_gate.sh",
)


def _features() -> dict[str, list[str]]:
    manifest = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    return manifest["features"]


def closure(
    features: dict[str, list[str]], roots: list[str]
) -> tuple[set[str], set[str]]:
    """Return the feature names and the `dep:` entries reachable from `roots`."""
    seen: set[str] = set()
    deps: set[str] = set()
    stack = list(roots)
    while stack:
        name = stack.pop()
        if name in seen:
            continue
        seen.add(name)
        for entry in features.get(name, []):
            if entry.startswith("dep:"):
                deps.add(entry)
            elif "/" not in entry:
                stack.append(entry.removesuffix("?"))
    return seen, deps


# spec: MEDULLA-CONTRACT-R066.1
def test_feature_graph_keeps_duckdb_out_of_the_default_build() -> None:
    features = _features()
    for root in ("default", "full", "node", "cluster"):
        if root not in features:
            continue
        seen, deps = closure(features, [root])
        assert "source-duckdb" not in seen, f"{root} reaches source-duckdb"
        assert "dep:duckdb" not in deps, f"{root} reaches dep:duckdb"
    assert "dep:duckdb" in features["source-duckdb"]
    for script in SERVING_SCRIPTS:
        text = (ROOT / script).read_text(encoding="utf-8")
        assert "source-duckdb" in text, f"{script} must enable source-duckdb"


PIN = ROOT / "scripts" / "duckdb-pin.toml"
FETCH = ROOT / "scripts" / "fetch_duckdb_lib.py"
PLATFORMS = (
    "linux-amd64",
    "linux-arm64",
    "osx-universal",
    "windows-amd64",
    "windows-arm64",
)
TARGETS = {
    "x86_64-unknown-linux-gnu": "linux-amd64",
    "aarch64-unknown-linux-gnu": "linux-arm64",
    "x86_64-apple-darwin": "osx-universal",
    "aarch64-apple-darwin": "osx-universal",
    "x86_64-pc-windows-msvc": "windows-amd64",
    "aarch64-pc-windows-msvc": "windows-arm64",
}


def _duckdb_dependency_lines() -> list[tuple[str, str]]:
    manifests = [ROOT / "Cargo.toml", ROOT / "crates/medulla-plan/Cargo.toml"]
    lines = []
    for manifest in manifests:
        for line in manifest.read_text(encoding="utf-8").splitlines():
            if re.match(r"^duckdb\s*=", line):
                lines.append((manifest.name, line))
    return lines


def _fetch(pin: Path, archive: Path, out: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [
            sys.executable,
            "-I",
            str(FETCH),
            "--target",
            "x86_64-unknown-linux-gnu",
            "--out",
            str(out),
            "--pin",
            str(pin),
            "--archive",
            str(archive),
        ],
        capture_output=True,
        text=True,
        check=False,
    )


# spec: MEDULLA-CONTRACT-R066.2
def test_prebuilt_library_is_pinned_by_version_and_checksum(tmp_path: Path) -> None:
    lines = _duckdb_dependency_lines()
    assert len(lines) == 2, lines
    for name, line in lines:
        assert "bundled" not in line, f"{name} enables bundled: {line}"
        assert 'version = "=1.10506.1"' in line, f"{name}: {line}"
        assert "default-features = false" in line, f"{name}: {line}"
    for manifest in ROOT.glob("**/Cargo.toml"):
        if "target" in manifest.parts or "vendor" in manifest.parts:
            continue
        for line in manifest.read_text(encoding="utf-8").splitlines():
            if line.startswith("duckdb") and "bundled" in line:
                pytest.fail(f"{manifest} enables bundled for duckdb")

    pin = tomllib.loads(PIN.read_text(encoding="utf-8"))
    assert pin["url"] == (
        "https://github.com/duckdb/duckdb/releases/download/v{version}/libduckdb-{platform}.zip"
    )
    assert sorted(pin["checksums"]) == sorted(PLATFORMS)
    for platform, digest in pin["checksums"].items():
        assert re.fullmatch(r"[0-9a-f]{64}", digest), platform
    assert pin["targets"] == TARGETS

    # The crate version `1.<MM><mm><pp>.x` embeds engine `1.<MM>.<pp>`.
    crate = re.search(r'=(\d+)\.(\d+)\.\d+"', lines[0][1])
    assert crate is not None
    major, packed = int(crate.group(1)), int(crate.group(2))
    engine = f"{major}.{(packed // 100) % 100}.{packed % 100}"
    assert pin["version"] == engine == "1.5.6"

    # A tampered archive is refused with the typed text and nothing is unpacked.
    archive = tmp_path / "libduckdb-linux-amd64.zip"
    with zipfile.ZipFile(archive, "w") as bundle:
        bundle.writestr("libduckdb.so", b"tampered")
    out = tmp_path / "out"
    refused = _fetch(PIN, archive, out)
    assert refused.returncode != 0
    assert "DUCKDB_CHECKSUM_MISMATCH" in refused.stderr
    assert not (out / "libduckdb.so").exists()

    # The same archive with its real checksum pinned is unpacked; its directory prints.
    local = tmp_path / "pin.toml"
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    local.write_text(
        PIN.read_text(encoding="utf-8").replace(
            pin["checksums"]["linux-amd64"], digest
        ),
        encoding="utf-8",
    )
    accepted = _fetch(local, archive, out)
    assert accepted.returncode == 0, accepted.stderr
    assert accepted.stdout.strip() == str(out.resolve())
    assert (out / "libduckdb.so").read_bytes() == b"tampered"


# spec: MEDULLA-CONTRACT-R066.2
def test_release_builds_fetch_the_library_before_cargo_build() -> None:
    for script in SERVING_SCRIPTS:
        text = (ROOT / script).read_text(encoding="utf-8")
        assert "fetch_duckdb_lib.py" in text, script
        build = re.search(r"^\s*(cargo|maturin) build ", text, re.MULTILINE)
        assert build is not None, script
        assert text.index("fetch_duckdb_lib.py") < build.start(), script
        assert "DUCKDB_LIB_DIR" in text, script
