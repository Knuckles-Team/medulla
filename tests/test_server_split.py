"""One server executable, packaged in the medulla wheel (MEDULLA-CONTRACT-R053)."""

from __future__ import annotations

import os
import re
from pathlib import Path

import pytest
import tomllib

from medulla import server_binary

REPO = Path(__file__).resolve().parents[1]
pytestmark = pytest.mark.no_engine


def _cargo() -> dict:
    return tomllib.loads((REPO / "Cargo.toml").read_text())


@pytest.mark.spec("MEDULLA-CONTRACT-R053.1")
def test_only_server_and_nemesis_binaries_remain_and_tools_are_dispatched() -> None:
    assert {b["name"] for b in _cargo()["bin"]} == {"medulla-server", "nemesis"}
    dispatcher = (REPO / "src/offline_tools.rs").read_text()
    for name in (
        "restore",
        "migrate-shards",
        "medulla-rf019-observe",
        "lake-fixture-export",
        "raft-flake-budget",
    ):
        assert f'"{name}" =>' in dispatcher
    main = (REPO / "src/main.rs").read_text()
    assert main.index("offline_tools::run_if_requested()") < main.index("Args::parse()")


@pytest.mark.spec("MEDULLA-CONTRACT-R053.4")
def test_server_binary_resolves_the_packaged_executable_only(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    exe = "medulla-server.exe" if os.name == "nt" else "medulla-server"
    packaged = tmp_path / "medulla" / "_bin" / exe
    monkeypatch.setattr(server_binary, "__file__", str(tmp_path / "medulla" / "x.py"))
    assert server_binary.packaged_server_binary() == packaged.resolve()
    # PATH is never consulted, even when it holds a medulla-server.
    on_path = tmp_path / "bin"
    on_path.mkdir()
    (on_path / exe).write_text("")
    (on_path / exe).chmod(0o755)
    monkeypatch.setenv("PATH", str(on_path))
    with pytest.raises(
        server_binary.ServerBinaryNotFoundError, match="platform wheels"
    ):
        server_binary.resolve_server_binary()
    packaged.parent.mkdir(parents=True)
    packaged.write_bytes(b"server")
    if os.name != "nt":
        packaged.chmod(0o644)
        with pytest.raises(
            server_binary.ServerBinaryNotFoundError, match="not executable"
        ):
            server_binary.resolve_server_binary()
        packaged.chmod(0o755)
    assert server_binary.resolve_server_binary() == str(packaged.resolve())
    assert "shutil" not in vars(server_binary)


@pytest.mark.spec("MEDULLA-CONTRACT-R053.5")
def test_release_server_profile_is_the_size_profile() -> None:
    profile = _cargo()["profile"]["release-server"]
    assert profile == {
        "inherits": "release",
        "opt-level": "s",
        "lto": "fat",
        "codegen-units": 1,
        "panic": "unwind",
        "strip": True,
    }


@pytest.mark.spec("MEDULLA-CONTRACT-R053.6")
def test_operator_docs_use_subcommands_not_retired_executables() -> None:
    retired = re.compile(r"^(restore|migrate-shards) --", re.MULTILINE)
    for path in (REPO / "docs").rglob("*.md"):
        assert not retired.search(path.read_text()), path
