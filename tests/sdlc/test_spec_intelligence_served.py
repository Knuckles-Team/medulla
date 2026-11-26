"""The spec intelligence report (MEDULLA-SDLC-R014.13 and R014.14).

L4: every call goes through the running engine. The report is the alternative body
``spec_report`` of ``UnifiedQuery``: it clusters the spec rows and the glossary
terms they use, reports the clusters that span repositories, and checks each row
against the row shapes of the spec standard. It carries no plan and writes nothing.

The R014.14 journey ingests real git repositories: each history goes in through
``ImportGitHistory`` and each tree through ``IndexRepository``, the one ingest path the
engine serves. Nothing of the journey is written node by node.
"""

from __future__ import annotations

import asyncio
import hashlib
import os
import subprocess
import uuid
from pathlib import Path

import msgpack
import pytest
from conftest import TEST_AGENT_ID, TEST_TENANT, request_context

from medulla.client import MedullaClient
from medulla.generated.index_repository import IndexRepositoryScope


def _node(node_id: str, node_type: str, **props: str) -> dict:
    return {
        "op": "upsert_node",
        "id": node_id,
        "properties": {"type": node_type, **props},
    }


def _edge(source: str, relationship: str, target: str) -> dict:
    return {
        "op": "upsert_edge",
        "source": source,
        "target": target,
        "properties": {"relationship": relationship},
    }


def _row(repo: str, row: str, status: str = "SPECIFIED") -> dict:
    return _node(
        f"requirement:{repo}:{row}",
        "Requirement",
        repo=repo,
        rowId=row,
        rowStatus=status,
    )


def _bound(repo: str, row: str) -> list[dict]:
    test = f"testfile:{repo}:tests/test_{row}.py"
    return [
        _node(test, "TestFile", repo=repo, path=f"tests/test_{row}.py"),
        _edge(f"requirement:{repo}:{row}", "boundTest", test),
    ]


def _terms(repo: str, row: str, *names: str) -> list[dict]:
    seed: list[dict] = []
    for name in names:
        seed.append(_node(f"term:{name}", "GlossaryTerm", name=name))
        seed.append(_edge(f"requirement:{repo}:{row}", "usesTerm", f"term:{name}"))
    return seed


def _ordered(seed: list[dict]) -> list[dict]:
    """Nodes before edges, so no edge names a node a later write adds."""
    return [op for op in seed if op["op"] == "upsert_node"] + [
        op for op in seed if op["op"] == "upsert_edge"
    ]


async def _with_graph(seed: list[dict], body):
    graph = f"sdlc-intel-{uuid.uuid4().hex[:8]}"
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=60,
    )
    try:
        await client.tenants.create(graph, "Global")
        # The report and the plan queries read the client's graph.
        client._graph_name = graph
        await client._send(
            "BatchUpdate",
            {"operations_msgpack": msgpack.packb(_ordered(seed), use_bin_type=True)},
            graph,
        )
        try:
            return await body(client, graph)
        finally:
            await client.tenants.delete(graph)
    finally:
        await client.close()


def _overlap_seed() -> list[dict]:
    """Two rows of two repositories that write the same two terms, and one row of a
    third repository that shares nothing with them."""
    seed = [_row("a", "A-R001"), _row("b", "B-R001"), _row("c", "C-R001")]
    seed += _bound("a", "A-R001") + _bound("b", "B-R001") + _bound("c", "C-R001")
    seed += _terms("a", "A-R001", "UnifiedQuery", "row_id")
    seed += _terms("b", "B-R001", "UnifiedQuery", "row_id")
    seed += _terms("c", "C-R001", "ProjectMode")
    return seed


# spec: MEDULLA-SDLC-R014.13
@pytest.mark.spec("MEDULLA-SDLC-R014.13")
def test_overlap_pair_across_repositories_is_reported():
    import asyncio

    async def body(client, graph):
        return await client.query.spec_report()

    report = asyncio.run(_with_graph(_overlap_seed(), body))
    assert report.rows == 3
    assert report.violations == []
    assert len(report.overlaps) == 1
    overlap = report.overlaps[0]
    assert overlap.row_ids == ["A-R001", "B-R001"]
    assert overlap.repositories == ["a", "b"]
    assert overlap.shared_terms == ["UnifiedQuery", "row_id"]
    assert overlap.cluster_id.startswith("l1:")


# spec: MEDULLA-SDLC-R014.13
@pytest.mark.spec("MEDULLA-SDLC-R014.13")
def test_report_never_writes():
    import asyncio

    async def body(client, graph):
        before = await client._send(
            "GetNodesByLabel",
            {"label": "Requirement", "after": None, "limit": 100},
            graph,
        )
        # A report that carries a plan, or another alternative body, is refused.
        with pytest.raises(Exception, match="REPORT_READ_ONLY"):
            await client._send(
                "UnifiedQuery",
                {
                    "plan": {"ops": [{"Scan": {"label": "Requirement"}}]},
                    "spec_report": {},
                },
                graph,
            )
        # A resolution outside the range is refused before anything is read.
        with pytest.raises(Exception, match="SPEC_REPORT_INVALID"):
            await client.query.spec_report(resolution_micros=1)
        report = await client.query.spec_report(seed=7)
        after = await client._send(
            "GetNodesByLabel",
            {"label": "Requirement", "after": None, "limit": 100},
            graph,
        )
        return report, before, after

    report, before, after = asyncio.run(_with_graph(_overlap_seed(), body))
    assert report.rows == 3
    # The report wrote no node and no edge.
    assert sorted(entry[0] for entry in before) == sorted(entry[0] for entry in after)


_JOURNEY_TERMS = "`UnifiedQuery` and `row_id`"
_GIT_ENV = {
    "GIT_AUTHOR_NAME": "A",
    "GIT_AUTHOR_EMAIL": "a@example.invalid",
    "GIT_COMMITTER_NAME": "A",
    "GIT_COMMITTER_EMAIL": "a@example.invalid",
}
_MODE = 0o100644


def _git(cwd: Path, *args: str, n: int = 0, stdin: bytes | None = None) -> bytes:
    env = {
        **os.environ,
        **_GIT_ENV,
        "GIT_AUTHOR_DATE": f"{1_700_000_000 + n * 60} +0000",
        "GIT_COMMITTER_DATE": f"{1_700_000_000 + n * 60} +0000",
    }
    return subprocess.run(
        ["git", *args], cwd=cwd, input=stdin, env=env, check=True, capture_output=True
    ).stdout


def _spec_row(row: str, text: str, tail: str = "") -> str:
    return (
        f"| `{row}` | **{text}** Caller chain: {tail}`serve` -> `src/demo.py:1`. "
        f"| Test: `tests/test_{row}.py::test_row` (L3). |\n"
    )


_TABLE = "# Requirements\n\n| ID | Requirement | Acceptance |\n|----|----|----|\n"


def _tree(name: str) -> dict[str, str]:
    """The files of one fixture repository. Every row has a bound test."""
    files = {"src/demo.py": "def serve():\n    return 1\n"}
    if name in ("a", "b"):
        row = f"{name.upper()}-R001"
        files["specs/demo/requirements.md"] = _TABLE + _spec_row(
            row, "Duplicate.", f"{_JOURNEY_TERMS} "
        )
        files[f"tests/test_{row}.py"] = f"# spec: {row}\n"
    else:
        # C-R002 depends on C-R001 and names the provider side of a seam only.
        files["specs/demo/requirements.md"] = (
            _TABLE
            + _spec_row("C-R001", "Landed.", "`other_thing` ")
            + _spec_row("C-R002", "Seam.", "`third_thing` ").replace(
                "(L3). |", "(L3). Depends: C-R001. |"
            )
        )
        files["specs/demo/contracts/seam-01.md"] = (
            "# Seam\n\nProvider: demo C-R002\n\n## Wire shape\n\nA request.\n"
        )
        files["tests/test_C-R001.py"] = "# spec: C-R001\n"
        files["tests/test_C-R002.py"] = "# spec: C-R002\n"
    return files


def _build_repository(root: Path, name: str) -> str:
    """Two commits on main; the second cites a row for repository a only."""
    _git(root, "init", "-q", "-b", "main")
    for path, text in _tree(name).items():
        target = root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text)
    _git(root, "add", ".")
    _git(root, "commit", "-q", "-m", f"chore: add {name}", n=1)
    (root / "src" / "demo.py").write_text("def serve():\n    return 2\n")
    message = "feat: serve two\n\nSpec: A-R001" if name == "a" else "chore: serve two"
    _git(root, "commit", "-q", "-a", "-m", message, n=2)
    return _git(root, "rev-parse", "HEAD").decode().strip()


def _pack(root: Path, head: str, have: str | None = None) -> bytes:
    revs = f"{head}\n" + (f"^{have}\n" if have else "")
    return _git(root, "pack-objects", "--revs", "--stdout", stdin=revs.encode())


async def _grant(client: MedullaClient, graph: str) -> None:
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


async def _import_history(
    client: MedullaClient, graph: str, branch: str, head: str, pack: bytes
) -> dict:
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


def _files(root: Path) -> list[tuple[str, bytes]]:
    return [
        (str(path.relative_to(root)), path.read_bytes())
        for path in sorted(root.rglob("*"))
        if path.is_file() and ".git" not in path.relative_to(root).parts
    ]


async def _index(
    client: MedullaClient, graph: str, name: str, branch: str, root: Path, head: str
) -> None:
    files = _files(root)
    scope = IndexRepositoryScope.model_validate(
        {
            "repository_id": name,
            "branch": branch,
            "commits": [],
            "refs": [
                {
                    "ref_name": f"refs/heads/{branch}",
                    "revision_id": head,
                    "status": "live",
                }
            ],
            "file_versions": [
                {
                    "ref_name": f"refs/heads/{branch}",
                    "path": path,
                    "blob_digest": "sha256:" + hashlib.sha256(body).hexdigest(),
                }
                for path, body in files
            ],
        }
    )
    await client.graph.index_repository(files, scope=scope, graph=graph)


async def _ids(client: MedullaClient, graph: str, label: str) -> list[str]:
    ids: list[str] = []
    after = None
    while True:
        page = await client._send(
            "GetNodesByLabel", {"label": label, "after": after, "limit": 1000}, graph
        )
        if not page:
            return sorted(ids)
        ids.extend(entry[0] for entry in page)
        after = page[-1][0]


async def _journey(workdir: Path) -> None:
    graph = f"sdlc-intel-{uuid.uuid4().hex[:8]}"
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=300,
    )
    try:
        await client.tenants.create(graph, "Global")
        await _grant(client, graph)
        repositories: dict[str, tuple[Path, str, str]] = {}
        for name in ("a", "b", "c"):
            root = workdir / name
            root.mkdir()
            head = _build_repository(root, name)
            branch = f"repo-{name}"
            await client._send(
                "CreateBranch",
                {"tenant": TEST_TENANT, "graph": graph, "name": branch},
            )
            first = await _import_history(
                client, graph, branch, head, _pack(root, head)
            )
            assert first["imported"] == 2
            repositories[name] = (root, branch, first["branch_head"])

        # One native changeset on repository c cites C-R001: the row lands with no git
        # commit behind it.
        root, branch, branch_head = repositories["c"]
        blob = await client.blob.store(b"def serve():\n    return 3\n")
        native = await client._send(
            "SubmitChangeset",
            {
                "request": {
                    "repo": graph,
                    "branch": branch,
                    "base_head": branch_head,
                    "message": "feat: land the first row\n\nSpec: C-R001",
                    "paths": [
                        {
                            "path": "src/demo.py",
                            "op": "modify",
                            "blob": {"digest": blob, "size": 28},
                            "mode": _MODE,
                            "renamed_from": None,
                        }
                    ],
                    "trailers": None,
                    "run_trace_id": None,
                }
            },
            idempotency_key=f"{graph}-native",
        )
        (root / "src" / "demo.py").write_bytes(b"def serve():\n    return 3\n")
        repositories["c"] = (root, branch, native["branch_head"])

        for name, (root, branch, _) in repositories.items():
            revision = _git(root, "rev-parse", "HEAD").decode().strip()
            await _index(client, graph, name, branch, root, revision)

        # The ordinary UnifiedQuery returns row to commit, row to test and row to
        # depends-on row.
        client._graph_name = graph
        commits = await client.query.unified(
            [
                {"Scan": {"label": "Requirement"}},
                {"Traverse": {"rel": "implementedByCommit", "min": 1, "max": 1}},
            ]
        )
        tests = await client.query.unified(
            [
                {"Scan": {"label": "Requirement"}},
                {"Traverse": {"rel": "boundTest", "min": 1, "max": 1}},
            ]
        )
        depends = await client.query.unified(
            [
                {"Scan": {"label": "Requirement"}},
                {"Traverse": {"rel": "dependsOnRow", "min": 1, "max": 1}},
            ]
        )
        report = await client.query.spec_report()

        # A second ingest after one new commit on repository a reads that commit only.
        root, branch, _ = repositories["a"]
        before = [c for c in await _ids(client, graph, "Changeset") if ":a:" in c]
        (root / "src" / "demo.py").write_text("def serve():\n    return 4\n")
        _git(root, "commit", "-q", "-a", "-m", "chore: serve four", n=3)
        new_head = _git(root, "rev-parse", "HEAD").decode().strip()
        previous = _git(root, "rev-parse", "HEAD~1").decode().strip()
        second = await _import_history(
            client, graph, branch, new_head, _pack(root, new_head, previous)
        )
        await _index(client, graph, "a", branch, root, new_head)
        after = [c for c in await _ids(client, graph, "Changeset") if ":a:" in c]
        return_values = (commits, tests, depends, report, second, before, after)
    finally:
        await client.tenants.delete(graph)
        await client.close()

    commits, tests, depends, report, second, before, after = return_values
    # spec: MEDULLA-SDLC-R014.14
    # Only the git commit that cites A-R001 is a commit of a row; the native
    # changeset of C-R001 carries no commit hash.
    assert len([row["id"] for row in commits]) == 1
    assert commits[0]["id"].startswith("changeset:a:")
    assert "testfile:a:tests/test_A-R001.py" in [row["id"] for row in tests]
    assert [row["id"] for row in depends] == ["requirement:c:C-R001"]
    assert report.rows == 4
    overlaps = [(o.row_ids, o.repositories) for o in report.overlaps]
    assert overlaps == [(["A-R001", "B-R001"], ["a", "b"])]
    assert [(v.row_id, v.code) for v in report.violations] == [
        ("C-R001", "LANDED_WITHOUT_COMMIT"),
        ("C-R002", "SEAM_SIDE_MISSING"),
    ]
    # The second ingest read exactly one commit, in the history and in the index.
    assert (second["imported"], second["skipped"]) == (1, 2)
    assert len(after) == len(before) + 1


# spec: MEDULLA-SDLC-R014.14
@pytest.mark.spec("MEDULLA-SDLC-R014.14")
def test_fixture_journey_ingests_queries_and_reports(tmp_path: Path) -> None:
    asyncio.run(_journey(tmp_path))
