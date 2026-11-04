"""The engine reports its release version (MEDULLA-CONTRACT-R053.12)."""

from __future__ import annotations

import configparser
import re
from pathlib import Path

import pytest
import tomllib

pytestmark = pytest.mark.no_engine

ROOT = Path(__file__).resolve().parents[1]
CRATES = ROOT / "crates"
VERSION_CRATE = CRATES / "medulla-version"
BUMPVERSION = ROOT / ".config/bumpversion.cfg"

_PKG_VERSION = re.compile(r'env!\s*\(\s*"CARGO_PKG_VERSION"\s*\)')
_BLOCK_COMMENT = re.compile(r"/\*.*?\*/", re.DOTALL)
_LINE_COMMENT = re.compile(r"//[^\n]*")


def _code_uses_pkg_version(source: str) -> list[int]:
    """Line numbers where code (not a comment) expands `env!("CARGO_PKG_VERSION")`."""

    stripped = _BLOCK_COMMENT.sub(lambda m: "\n" * m.group(0).count("\n"), source)
    stripped = _LINE_COMMENT.sub("", stripped)
    return [
        number
        for number, line in enumerate(stripped.splitlines(), start=1)
        if _PKG_VERSION.search(line)
    ]


def _rust_sources() -> list[Path]:
    return sorted(
        path
        for path in CRATES.rglob("*.rs")
        if "target" not in path.relative_to(CRATES).parts
        and not path.is_relative_to(VERSION_CRATE)
    )


def _package_version(manifest: Path) -> str:
    return tomllib.loads(manifest.read_text(encoding="utf-8"))["package"]["version"]


@pytest.mark.spec("MEDULLA-CONTRACT-R053.12")
def test_the_scanner_sees_code_and_skips_comments() -> None:
    source = (
        '/// e.g. `env!("CARGO_PKG_VERSION")`\n'
        '/* env!("CARGO_PKG_VERSION") */\n'
        'const V: &str = env!("CARGO_PKG_VERSION");\n'
    )
    assert _code_uses_pkg_version(source) == [3]


@pytest.mark.spec("MEDULLA-CONTRACT-R053.12")
def test_only_medulla_version_reads_the_package_version() -> None:
    offenders = [
        f"{path.relative_to(ROOT)}:{line}"
        for path in _rust_sources()
        for line in _code_uses_pkg_version(path.read_text(encoding="utf-8"))
    ]
    assert offenders == [], (
        "crates carry a placeholder package version; report the engine release "
        "version with medulla_version::VERSION instead: " + ", ".join(offenders)
    )
    lib = (VERSION_CRATE / "src/lib.rs").read_text(encoding="utf-8")
    assert 'pub const VERSION: &str = env!("CARGO_PKG_VERSION");' in lib


@pytest.mark.spec("MEDULLA-CONTRACT-R053.12")
def test_medulla_version_moves_with_the_engine_release() -> None:
    config = configparser.ConfigParser(interpolation=None)
    config.read(BUMPVERSION, encoding="utf-8")
    current = config["bumpversion"]["current_version"]
    assert _package_version(VERSION_CRATE / "Cargo.toml") == current
    assert _package_version(ROOT / "Cargo.toml") == current
    assert config.has_section("bumpversion:file:crates/medulla-version/Cargo.toml")
    lock_section = "bumpversion:file(medulla-version):Cargo.lock"
    assert config.has_section(lock_section)
    assert 'name = "medulla-version"' in config[lock_section]["search"]
    lock = (ROOT / "Cargo.lock").read_text(encoding="utf-8")
    assert f'name = "medulla-version"\nversion = "{current}"\n' in lock
    members = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    assert "crates/medulla-version" in members["workspace"]["members"]
