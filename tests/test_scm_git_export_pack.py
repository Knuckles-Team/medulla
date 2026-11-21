"""MEDULLA-SCM-R014: the served ``GitExportPack`` method, with MEDULLA-SCM-R010 import.

A repository with a merge commit is built with a git client and packed with
``git pack-objects``. The pack is uploaded through the blob methods and imported
with ``ImportGitHistory``; ``GitExportPack`` returns the pack of the chain. A
bare clone indexes that pack: its head commit, its tree hash and its parent
lists (the merge keeps both parents) equal the original, and ``git fsck``
finds no missing object. Narrowing the grant on one path then refuses the whole
export with ``PATH_READ_DENIED``.
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


def _build_repository(root: Path) -> tuple[str, str, bytes]:
    """Two commits on main, two on a side branch, then a merge; returns head, tree, pack."""
    _git(root, "init", "-q", "-b", "main")
    (root / "src").mkdir()
    (root / "secret").mkdir()
    (root / "src" / "a.txt").write_text("one\ntwo\n")
    (root / "secret" / "key.txt").write_text("hidden\n")
    _git(root, "add", ".")
    _git(root, "commit", "-q", "-m", "first", n=1)
    _git(root, "checkout", "-q", "-b", "side")
    (root / "src" / "side.txt").write_text("side\n")
    _git(root, "add", ".")
    _git(root, "commit", "-q", "-m", "side one", n=2)
    (root / "src" / "side.txt").write_text("side\nmore\n")
    _git(root, "commit", "-q", "-a", "-m", "side two", n=3)
    _git(root, "checkout", "-q", "main")
    (root / "src" / "a.txt").write_text("one\ntwo\nthree\n")
    _git(root, "commit", "-q", "-a", "-m", "main two", n=4)
    _git(root, "merge", "-q", "--no-ff", "-m", "merge side", "side", n=5)
    head = _git(root, "rev-parse", "HEAD").decode().strip()
    tree = _git(root, "rev-parse", "HEAD^{tree}").decode().strip()
    pack = _git(root, "pack-objects", "--revs", "--stdout", stdin=f"{head}\n".encode())
    return head, tree, pack


async def _scenario(graph: str, workdir: Path) -> None:
    repo = workdir / "origin"
    repo.mkdir()
    head, tree, pack = _build_repository(repo)
    parents = _git(repo, "rev-list", "--parents", "-n", "1", head).decode().split()

    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=60,
    )
    try:
        await client.tenants.create(graph, "Global")
        branch = f"feature/git-{uuid.uuid4().hex[:8]}"
        await client._send(
            "CreateBranch", {"tenant": TEST_TENANT, "graph": graph, "name": branch}
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
        pack_digest = await client.blob.store(pack)
        imported = await client._send(
            "ImportGitHistory",
            {
                "request": {
                    "repo": graph,
                    "branch": branch,
                    "pack_digest": pack_digest,
                    "head": head,
                    "object_format": "sha1",
                }
            },
        )
        # Three first-parent commits: first, main two, the merge.
        assert imported["imported"] == 3
        assert imported["skipped"] == 0
        again = await client._send(
            "ImportGitHistory",
            {
                "request": {
                    "repo": graph,
                    "branch": branch,
                    "pack_digest": pack_digest,
                    "head": head,
                    "object_format": "sha1",
                }
            },
        )
        assert again["imported"] == 0
        assert again["skipped"] == 3

        exported = await client._send(
            "GitExportPack",
            {"request": {"repo": graph, "branch": branch, "wants": [head], "haves": []}},
        )
        assert exported["object_format"] == "sha1"
        clone = workdir / "clone.git"
        clone.mkdir()
        _git(clone, "init", "-q", "--bare")
        _git(clone, "index-pack", "--stdin", stdin=bytes.fromhex(exported["pack_hex"]))
        _git(clone, "update-ref", "refs/heads/main", head)
        _git(clone, "fsck", "--connectivity-only", "--strict")
        assert _git(clone, "rev-parse", "main^{tree}").decode().strip() == tree
        assert _git(clone, "rev-parse", "main").decode().strip() == head
        assert (
            _git(clone, "rev-list", "--parents", "-n", "1", "main").decode().split()
            == parents
        )

        # One denied path refuses the whole export.
        await client._send("SetPathGrant", grant("secret", False, True))
        with pytest.raises(Exception, match="PATH_READ_DENIED"):
            await client._send(
                "GitExportPack",
                {"request": {"repo": graph, "branch": branch, "wants": [head]}},
            )
        await client.tenants.delete(graph)
    finally:
        await client.close()


# spec: MEDULLA-SCM-R014
# spec: MEDULLA-SCM-R010
@pytest.mark.spec("MEDULLA-SCM-R014")
@pytest.mark.spec("MEDULLA-SCM-R010")
@pytest.mark.skipif(shutil.which("git") is None, reason="needs a git client")
def test_clone_matches_tree_hash_and_denied_path_refuses(tmp_path: Path):
    asyncio.run(_scenario(f"scm-git-{uuid.uuid4().hex[:8]}", tmp_path))
