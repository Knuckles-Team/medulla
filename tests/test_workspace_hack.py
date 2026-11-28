"""The workspace-hack crate, its gate, and its effect on single-crate builds.

MEDULLA-CONTRACT-R069 (rollup), R069.1 (the crate), R069.2 (the gate) and R069.3 (the
proof). The gate is `cargo xtask workspace-hack`. The gate tests drive it against a
fixture workspace so they do not depend on the real crate set: two members that use
one outside dependency with different feature sets, a generated hack crate, then a
stale one. They need cargo and the pinned `cargo-hakari` and skip where either is
missing; the gate runs on a build host.
"""

from __future__ import annotations

import shutil
import subprocess
from pathlib import Path

import pytest
import tomllib

pytestmark = pytest.mark.no_engine

ROOT = Path(__file__).resolve().parents[1]
PINNED = "0.9.39"


def _tool(
    *args: str, cwd: Path, check: bool = True
) -> subprocess.CompletedProcess[str]:
    proc = subprocess.run(args, cwd=cwd, capture_output=True, text=True)
    if check:
        assert proc.returncode == 0, f"{args}: {proc.stdout}{proc.stderr}"
    return proc


def _need_hakari() -> None:
    if not shutil.which("cargo"):
        pytest.skip("cargo is unavailable here; this gate runs on a build host")
    probe = subprocess.run(
        ["cargo", "hakari", "--version"], capture_output=True, text=True
    )
    if probe.returncode != 0:
        pytest.skip("cargo-hakari is unavailable here; this gate runs on a build host")


def _gate(fixture: Path) -> subprocess.CompletedProcess[str]:
    return _tool(
        "cargo", "run", "-q", "-p", "medulla-xtask", "--",
        "workspace-hack", "--root", str(fixture),
        cwd=ROOT, check=False,
    )  # fmt: skip


def _member(ws: Path, name: str, features: str) -> None:
    crate = ws / "crates" / name
    (crate / "src").mkdir(parents=True)
    (crate / "src" / "lib.rs").write_text("", encoding="utf-8")
    (crate / "Cargo.toml").write_text(
        f'[package]\nname = "{name}"\nversion = "0.0.0"\nedition = "2021"\n'
        "publish = false\n\n"
        f'[dependencies]\nshared = {{ path = "../../../shared", {features} }}\n',
        encoding="utf-8",
    )


@pytest.fixture
def fixture_workspace(tmp_path: Path) -> Path:
    """Two members share one outside dependency with differing features."""
    _need_hakari()
    shared = tmp_path / "shared"
    (shared / "src").mkdir(parents=True)
    (shared / "src" / "lib.rs").write_text("", encoding="utf-8")
    (shared / "Cargo.toml").write_text(
        '[package]\nname = "shared"\nversion = "0.1.0"\nedition = "2021"\n\n'
        "[features]\nf = []\ng = []\n",
        encoding="utf-8",
    )
    ws = tmp_path / "ws"
    ws.mkdir()
    (ws / "Cargo.toml").write_text(
        '[workspace]\nresolver = "2"\nmembers = ["crates/*"]\n', encoding="utf-8"
    )
    _member(ws, "left", "features = []")
    _member(ws, "right", 'features = ["f"]')
    _tool("cargo", "hakari", "init", "crates/fixture-hack", "--yes", cwd=ws)
    _tool("cargo", "hakari", "generate", cwd=ws)
    _tool("cargo", "hakari", "manage-deps", "--yes", cwd=ws)
    return ws


# spec: MEDULLA-CONTRACT-R069
# spec: MEDULLA-CONTRACT-R069.2
@pytest.mark.spec("MEDULLA-CONTRACT-R069")
@pytest.mark.spec("MEDULLA-CONTRACT-R069.2")
def test_generated_crate_matches_a_fresh_generation(fixture_workspace: Path) -> None:
    fresh = _gate(fixture_workspace)
    assert fresh.returncode == 0, fresh.stdout + fresh.stderr

    # A member asks for a feature the checked-in crate lacks: the gate must fail
    # and show the diff.
    right = fixture_workspace / "crates" / "right" / "Cargo.toml"
    right.write_text(
        right.read_text(encoding="utf-8").replace(
            'features = ["f"]', 'features = ["f", "g"]'
        ),
        encoding="utf-8",
    )
    stale = _gate(fixture_workspace)
    assert stale.returncode != 0
    assert "stale" in stale.stderr
    assert '"g"' in stale.stderr, stale.stderr


# spec: MEDULLA-CONTRACT-R069.2
def test_a_different_tool_version_is_refused() -> None:
    _need_hakari()
    version = subprocess.run(
        ["cargo", "hakari", "--version"], capture_output=True, text=True
    ).stdout.split()[-1]
    if version != PINNED:
        gate = _gate(ROOT)
        assert gate.returncode != 0
        assert "HAKARI_VERSION_MISMATCH" in gate.stderr
    else:
        pytest.skip("the tool is the pinned version; the Rust unit test covers refusal")


def test_the_pin_is_one_value_everywhere() -> None:
    boot = (ROOT / "scripts" / "bootstrap.sh").read_text(encoding="utf-8")
    gate = (ROOT / "tools" / "xtask" / "src" / "workspace_hack.rs").read_text(
        encoding="utf-8"
    )
    assert f"hakari_version={PINNED}" in boot
    assert f'PINNED_VERSION: &str = "{PINNED}"' in gate
    assert "HAKARI_VERSION_MISMATCH" in gate


HACK = "medulla-workspace-hack"
RELEASE_TRIPLES = {
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
    "aarch64-pc-windows-msvc",
}


def _members() -> dict[str, Path]:
    """Every workspace crate manifest: the facade, `tools/*` and every `crates/*`."""
    manifests = [ROOT / "Cargo.toml", *sorted((ROOT / "tools").glob("*/Cargo.toml"))]
    manifests += sorted((ROOT / "crates").glob("*/Cargo.toml"))
    found: dict[str, Path] = {}
    for manifest in manifests:
        package = tomllib.loads(manifest.read_text(encoding="utf-8")).get("package")
        if package:
            found[package["name"]] = manifest
    return found


# spec: MEDULLA-CONTRACT-R069.1
@pytest.mark.spec("MEDULLA-CONTRACT-R069.1")
def test_every_member_depends_on_the_workspace_hack() -> None:
    config = tomllib.loads(
        (ROOT / ".config" / "hakari.toml").read_text(encoding="utf-8")
    )
    assert config["hakari-package"] == HACK
    assert config["dep-format-version"] == "4"
    assert config["resolver"] == "2"
    assert config["exact-versions"] is True
    assert set(config["platforms"]) == RELEASE_TRIPLES
    boot = (ROOT / "scripts" / "bootstrap.sh").read_text(encoding="utf-8")
    assert f"hakari_version={PINNED}" in boot

    members = _members()
    assert HACK in members, "the generated crate is a workspace member"
    assert len(members) > 100, "the member universe must not be empty"
    hack = tomllib.loads(members[HACK].read_text(encoding="utf-8"))
    assert hack["package"]["publish"] is False
    assert (members[HACK].parent / "src" / "lib.rs").is_file()
    missing = [
        name
        for name, manifest in members.items()
        if name != HACK
        and HACK
        not in tomllib.loads(manifest.read_text(encoding="utf-8")).get(
            "dependencies", {}
        )
    ]
    assert not missing, f"members without the workspace-hack dependency: {missing}"


def _features(*cargo_tree_args: str) -> dict[str, frozenset[str]]:
    """Enabled features of every third-party dependency in one `cargo tree` closure.

    Keyed `name vX`. Dev-dependency edges are left out (the workspace-hack crate
    unifies normal and build dependencies), and so are workspace crates (their own
    features are not unified by it).
    """
    out = _tool(
        "cargo", "tree", "--locked", "-e", "normal,build", "--prefix", "none",
        "--format", "{p}|{f}", *cargo_tree_args,
        cwd=ROOT,
    ).stdout  # fmt: skip
    found: dict[str, frozenset[str]] = {}
    for line in out.splitlines():
        package, _, enabled = line.partition("|")
        fields = package.split()
        if len(fields) < 2 or "(/" in package:
            continue
        name, version = fields[:2]
        enabled = enabled.removesuffix(" (*)").removesuffix(" (proc-macro)")
        found[f"{name} {version}"] = frozenset(filter(None, enabled.split(",")))
    return found


# spec: MEDULLA-CONTRACT-R069.3
@pytest.mark.spec("MEDULLA-CONTRACT-R069.3")
def test_single_crate_build_uses_the_workspace_feature_set() -> None:
    _need_hakari()
    low = _features("-p", "medulla-types")
    high = _features("-p", "medulla-h-graph")
    workspace = _features("--workspace")
    shared = sorted(
        dep for dep in low.keys() & high.keys() if dep not in {f"{HACK} v0.1.0"}
    )
    assert len(shared) > 20, "the two closures must share dependencies"
    differing = [
        dep for dep in shared if not low[dep] == high[dep] == workspace.get(dep)
    ]
    assert not differing, f"feature sets differ for: {differing}"
