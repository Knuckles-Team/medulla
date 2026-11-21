"""Spec rows are requirement records with caller and proof edges (MEDULLA-SDLC-R012.2).

L4: the engine projects each requirements row of a spec directory into one
requirement node (ID, owner repository, title, derived state) and derives the
`dependsOn`, `implementedBy` and `provedBy` edges from the row text. The
projection is `project_requirements` in
`crates/medulla-h-ingest/src/source_ingestion/spec_projection.rs`; it writes
through the mutation gateway as one batch. This test drives the engine
projection through the scoped crate test and asserts the nodes and edges it
reports for the fixture rows.
"""

from __future__ import annotations

import asyncio
import os
import re
import subprocess
import uuid
from pathlib import Path

import pytest
from conftest import TEST_AGENT_ID, TEST_TENANT, request_context

from medulla.client import MedullaClient

# The R012.2 projection tests run in the scoped crate test, not the shared engine.
# The R012.1 served test below needs the engine and carries no marker.

ROOT = Path(__file__).resolve().parents[2]
PROJECTION_RS = (
    ROOT
    / "crates"
    / "medulla-h-ingest"
    / "src"
    / "source_ingestion"
    / "spec_projection.rs"
)
PROJECT_TEST = (
    "source_ingestion::spec_projection::tests::"
    "spec_rows_become_requirement_nodes_with_edges"
)
REFUSAL_TEST = (
    "source_ingestion::spec_projection::tests::"
    "unknown_depends_id_refuses_and_writes_nothing"
)


def _run_engine_test(name: str) -> str:
    proc = subprocess.run(
        [
            "cargo",
            "test",
            "-p",
            "medulla-h-ingest",
            "--lib",
            "--",
            "--exact",
            name,
            "--nocapture",
        ],
        cwd=ROOT,
        env={**os.environ, "CARGO_TERM_COLOR": "never"},
        capture_output=True,
        text=True,
        timeout=1800,
        check=False,
    )
    if proc.returncode != 0:
        raise AssertionError(
            f"engine projection test {name} failed:\n{proc.stdout}\n{proc.stderr}"
        )
    return proc.stdout


@pytest.mark.no_engine
@pytest.mark.spec("MEDULLA-SDLC-R012.2")
def test_spec_rows_become_requirement_nodes_with_edges() -> None:
    source = PROJECTION_RS.read_text(encoding="utf-8")
    assert "pub fn project_requirements" in source
    assert "REQUIREMENT_UNRESOLVED" in source

    out = _run_engine_test(PROJECT_TEST)
    nodes = {
        row_id: (repo, title, state)
        for row_id, repo, title, state in re.findall(
            r"^REQ_NODE\t(\S+)\t(\S+)\t([^\t]+)\t(\S+)$", out, flags=re.MULTILINE
        )
    }
    edges = set(re.findall(r"^REQ_EDGE\t(\S+)\t(\S+)\t(\S+)$", out, flags=re.MULTILINE))

    # spec: MEDULLA-SDLC-R012.2
    assert nodes == {
        "DEMO-R001": ("demo", "Base.", "VERIFIED"),
        "DEMO-R002": ("demo", "Second.", "SPECIFIED"),
        "DEMO-R003": ("demo", "Third.", "SPECIFIED"),
    }
    assert ("DEMO-R002", "dependsOn", "requirement:demo:DEMO-R001") in edges
    assert ("DEMO-R003", "dependsOn", "requirement:demo:DEMO-R002") in edges
    assert ("DEMO-R001", "implementedBy", "symbol:serve_base") in edges
    assert (
        "DEMO-R002",
        "provedBy",
        "test:crates/demo/tests/second.rs::runs",
    ) in edges


@pytest.mark.no_engine
def test_unresolved_requirement_id_refuses() -> None:
    # spec: MEDULLA-SDLC-R012.2
    _run_engine_test(REFUSAL_TEST)


_GIT_ENV = {
    "GIT_AUTHOR_NAME": "A",
    "GIT_AUTHOR_EMAIL": "a@example.invalid",
    "GIT_COMMITTER_NAME": "A",
    "GIT_COMMITTER_EMAIL": "a@example.invalid",
}


def _git(cwd: Path, *args: str, stdin: bytes | None = None, n: int = 0) -> bytes:
    env = {
        **os.environ,
        **_GIT_ENV,
        "GIT_AUTHOR_DATE": f"{1_700_000_000 + n * 60} +0000",
        "GIT_COMMITTER_DATE": f"{1_700_000_000 + n * 60} +0000",
    }
    return subprocess.run(
        ["git", *args], cwd=cwd, input=stdin, env=env, check=True, capture_output=True
    ).stdout


def _build_repository(root: Path, name: str) -> tuple[str, bytes]:
    """Two commits on main; returns the head commit and its pack."""
    _git(root, "init", "-q", "-b", "main")
    (root / "src").mkdir()
    (root / "src" / f"{name}.txt").write_text("one\n")
    _git(root, "add", ".")
    _git(root, "commit", "-q", "-m", f"{name} first", n=1)
    (root / "src" / f"{name}.txt").write_text("one\ntwo\n")
    _git(root, "commit", "-q", "-a", "-m", f"{name} second", n=2)
    head = _git(root, "rev-parse", "HEAD").decode().strip()
    pack = _git(root, "pack-objects", "--revs", "--stdout", stdin=f"{head}\n".encode())
    return head, pack


async def _import(client: MedullaClient, graph: str, branch: str, head: str, pack: bytes):
    digest = await client.blob.store(pack)
    return await client._send(
        "ImportGitHistory",
        {
            "request": {
                "repo": graph,
                "branch": branch,
                "pack_digest": digest,
                "head": head,
                "object_format": "sha1",
            }
        },
    )


async def _log(client: MedullaClient, graph: str, branch: str) -> list[str]:
    diff = await client._send(
        "BranchDiff",
        {
            "tenant": TEST_TENANT,
            "graph": graph,
            "branch": branch,
            "from_version": None,
            "to_version": None,
            "offset": 0,
        },
    )
    return [commit["id"] for commit in diff["commits"]]


async def _repositories(workdir: Path) -> None:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=60,
    )
    try:
        stem = uuid.uuid4().hex[:8]
        logs: dict[str, list[str]] = {}
        for name in ("alpha", "beta"):
            graph = f"sdlc-{name}-{stem}"
            repo = workdir / name
            repo.mkdir()
            head, pack = _build_repository(repo, name)
            await client.tenants.create(graph, "Global")
            branch = f"feature/default-{stem}"
            await client._send(
                "CreateBranch", {"tenant": TEST_TENANT, "graph": graph, "name": branch}
            )
            await client._send(
                "SetPathGrant",
                {
                    "request": {
                        "repo": graph,
                        "principal": TEST_AGENT_ID,
                        "prefix": "",
                        "read": True,
                        "write": True,
                    }
                },
            )
            first = await _import(client, graph, branch, head, pack)
            assert first["imported"] == 2
            logs[name] = await _log(client, graph, branch)
            assert len(logs[name]) == 2

            # A repeat ingest adds no duplicate changeset.
            again = await _import(client, graph, branch, head, pack)
            assert again["imported"] == 0
            assert again["skipped"] == 2
            assert await _log(client, graph, branch) == logs[name]
            await client.tenants.delete(graph)
        # Each repository has its own log.
        assert not set(logs["alpha"]) & set(logs["beta"])
    finally:
        await client.close()


# spec: MEDULLA-SDLC-R012.1
@pytest.mark.spec("MEDULLA-SDLC-R012.1")
def test_each_repository_ingests_as_a_changeset_log(tmp_path: Path) -> None:
    asyncio.run(_repositories(tmp_path))
