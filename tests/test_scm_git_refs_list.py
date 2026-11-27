"""MEDULLA-SCM-R851: the served ``GitRefsList`` method.

A one-commit repository is packed with ``git pack-objects`` and imported on one
branch. ``GitRefsList`` answers ``refs/heads/<branch>`` with the git commit id of
the head, a branch with no changeset is not listed, and narrowing the grant on
one path refuses the whole listing with ``PATH_READ_DENIED``.
"""

from __future__ import annotations

import asyncio
import os
import shutil
import subprocess
import uuid
from pathlib import Path

import pytest
from conftest import TEST_AGENT_ID, TEST_TENANT, request_context

from medulla.client import MedullaClient

_GIT_ENV = {
    "GIT_AUTHOR_NAME": "A",
    "GIT_AUTHOR_EMAIL": "a@example.invalid",
    "GIT_COMMITTER_NAME": "A",
    "GIT_COMMITTER_EMAIL": "a@example.invalid",
    "GIT_AUTHOR_DATE": "1700000000 +0000",
    "GIT_COMMITTER_DATE": "1700000000 +0000",
}


def _git(cwd: Path, *args: str, stdin: bytes | None = None) -> bytes:
    return subprocess.run(
        ["git", *args],
        cwd=cwd,
        input=stdin,
        env={**os.environ, **_GIT_ENV},
        check=True,
        capture_output=True,
    ).stdout


def _build_repository(root: Path) -> tuple[str, bytes]:
    _git(root, "init", "-q", "-b", "main")
    (root / "secret").mkdir()
    (root / "a.txt").write_text("one\n")
    (root / "secret" / "key.txt").write_text("hidden\n")
    _git(root, "add", ".")
    _git(root, "commit", "-q", "-m", "first")
    head = _git(root, "rev-parse", "HEAD").decode().strip()
    pack = _git(root, "pack-objects", "--revs", "--stdout", stdin=f"{head}\n".encode())
    return head, pack


async def _scenario(graph: str, workdir: Path) -> None:
    repo = workdir / "origin"
    repo.mkdir()
    head, pack = _build_repository(repo)
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=60,
    )
    try:
        await client.tenants.create(graph, "Global")
        imported = f"feature/imported-{uuid.uuid4().hex[:8]}"
        empty = f"feature/empty-{uuid.uuid4().hex[:8]}"
        for name in (imported, empty):
            await client._send(
                "CreateBranch", {"tenant": TEST_TENANT, "graph": graph, "name": name}
            )

        def grant(prefix: str, read: bool, write: bool) -> dict:
            return {
                "request": {
                    "repo": graph,
                    "principal": TEST_AGENT_ID,
                    "prefix": prefix,
                    "read": read,
                    "write": write,
                }
            }

        await client._send("SetPathGrant", grant("", True, True))
        await client._send(
            "ImportGitHistory",
            {
                "request": {
                    "repo": graph,
                    "branch": imported,
                    "pack_digest": await client.blob.store(pack),
                    "head": head,
                    "object_format": "sha1",
                }
            },
        )
        refs = await client._send("GitRefsList", {"request": {"repo": graph}})
        assert refs == [{"ref": f"refs/heads/{imported}", "object_id": head}]

        unknown = await client._send("GitRefsList", {"request": {"repo": "no-such"}})
        assert unknown == []

        await client._send("SetPathGrant", grant("secret", False, True))
        with pytest.raises(Exception, match="PATH_READ_DENIED"):
            await client._send("GitRefsList", {"request": {"repo": graph}})
        await client.tenants.delete(graph)
    finally:
        await client.close()


# spec: MEDULLA-SCM-R851
@pytest.mark.spec("MEDULLA-SCM-R851")
@pytest.mark.skipif(shutil.which("git") is None, reason="needs a git client")
def test_refs_list_names_each_head_and_a_denied_path_refuses(tmp_path: Path):
    asyncio.run(_scenario(f"scm-refs-{uuid.uuid4().hex[:8]}", tmp_path))
