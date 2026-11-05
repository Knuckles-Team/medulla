"""Concurrent changes merge without registry conflicts (MEDULLA-SDLC-R020)."""

from __future__ import annotations

import os
import re
import subprocess
from pathlib import Path

import pytest

pytestmark = pytest.mark.no_engine
ROOT = Path(__file__).resolve().parents[1]
UNION_REGISTRIES = (
    "crates/medulla-types/src/contract/server_error_code.rs",
    "crates/medulla-types/src/lib.rs",
    "architecture/component-registry.yml",
)


def _union_paths() -> set[str]:
    paths = set()
    for line in (ROOT / ".gitattributes").read_text(encoding="utf-8").splitlines():
        fields = line.split()
        if (
            len(fields) >= 2
            and not line.startswith("#")
            and "merge=union" in fields[1:]
        ):
            paths.add(fields[0])
    return paths


def _git(repo: Path, *args: str) -> subprocess.CompletedProcess[str]:
    env = {
        key: value for key, value in os.environ.items() if not key.startswith("GIT_")
    }
    env.update(
        GIT_AUTHOR_NAME="t",
        GIT_AUTHOR_EMAIL="t@example.invalid",
        GIT_COMMITTER_NAME="t",
        GIT_COMMITTER_EMAIL="t@example.invalid",
        GIT_CONFIG_GLOBAL=os.devnull,
        GIT_CONFIG_NOSYSTEM="1",
    )
    return subprocess.run(
        ["git", *args], cwd=repo, env=env, capture_output=True, text=True, check=False
    )


# spec: MEDULLA-SDLC-R020
def test_append_only_registries_merge_as_union_and_no_exact_method_count(
    tmp_path: Path,
) -> None:
    union = _union_paths()
    for path in UNION_REGISTRIES:
        assert path in union, f"{path} must be marked merge=union in .gitattributes"
        assert (ROOT / path).is_file(), f"{path} is listed but does not exist"

    registry = (
        ROOT / "crates/medulla-capabilities/tests/contract_registry.rs"
    ).read_text(encoding="utf-8")
    assert not re.search(r"variants\.len\(\),\s*\d+", registry), (
        "contract_registry.rs pins an exact method count; the bijection is the census"
    )

    repo = tmp_path / "repo"
    repo.mkdir()
    _git(repo, "init", "-q", "-b", "main")
    (repo / ".gitattributes").write_text("registry.rs merge=union\n", encoding="utf-8")
    (repo / "registry.rs").write_text("enum E {\n    A,\n    B,\n}\n", encoding="utf-8")
    _git(repo, "add", ".")
    assert _git(repo, "commit", "-q", "-m", "base").returncode == 0
    for branch, entry in (("left", "    C,\n"), ("right", "    D,\n")):
        _git(repo, "checkout", "-q", "-b", branch, "main")
        text = (repo / "registry.rs").read_text(encoding="utf-8")
        (repo / "registry.rs").write_text(
            text.replace("    B,\n", "    B,\n" + entry), encoding="utf-8"
        )
        assert _git(repo, "commit", "-q", "-am", branch).returncode == 0
    _git(repo, "checkout", "-q", "left")
    merged = _git(repo, "merge", "-q", "--no-edit", "right")
    assert merged.returncode == 0, merged.stdout + merged.stderr
    text = (repo / "registry.rs").read_text(encoding="utf-8")
    assert "    C,\n" in text and "    D,\n" in text and "<<<<<<<" not in text
