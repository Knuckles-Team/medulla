"""Served post-commit repository enrichment and ecosystem facts.

MEDULLA-REPO-INGEST-R600.3: a scoped ``IndexRepository`` commits its index
envelope with one durable follow-on work item; the in-process consumer claims
it, reads its inputs from the committed index graph by digest and commits one
``repository-enrich:<digest>`` envelope, whose run node is read back here.

MEDULLA-REPO-INGEST-R801: packages and architecture layers are typed nodes
with dependency edges, read through ``Method::Uql``; a layer cycle refuses the
batch with ``LAYER_CYCLE``.
"""

from __future__ import annotations

import hashlib
import time
from typing import Any

import pytest

from medulla.generated.index_repository import IndexRepositoryScope

_REVISION = "c" * 40
_WAIT_S = 30.0


def _scope(
    repository_id: str,
    files: list[tuple[str, bytes]],
    commits: list[dict[str, Any]] | None = None,
) -> IndexRepositoryScope:
    return IndexRepositoryScope.model_validate(
        {
            "repository_id": repository_id,
            "commits": commits or [],
            "refs": [
                {
                    "ref_name": "refs/heads/main",
                    "revision_id": _REVISION,
                    "status": "live",
                }
            ],
            "file_versions": [
                {
                    "ref_name": "refs/heads/main",
                    "path": path,
                    "blob_digest": "sha256:" + hashlib.sha256(body).hexdigest(),
                }
                for path, body in files
            ],
        }
    )


def _index(
    client: Any,
    repository_id: str,
    files: list[tuple[str, bytes]],
    commits: list[dict[str, Any]] | None = None,
) -> Any:
    try:
        return client.graph.index_repository(
            files, scope=_scope(repository_id, files, commits)
        )
    except Exception as exc:
        text = str(exc)
        if "AST feature not enabled" in text or "requires durable persistence" in text:
            pytest.skip("engine built without the ast feature or durable persistence")
        raise


def _ids(client: Any, query: str) -> list[str]:
    return sorted(row["id"] for row in client.query.uql(query)["rows"])


def _wait_for_runs(client: Any, count: int) -> list[str]:
    deadline = time.monotonic() + _WAIT_S
    runs = _ids(client, "MATCH (:RepositoryEnrichmentRun)")
    while len(runs) < count and time.monotonic() < deadline:
        time.sleep(0.25)
        runs = _ids(client, "MATCH (:RepositoryEnrichmentRun)")
    return runs


_CALLS_A = [
    (
        "pkg/a.py",
        b"def ping():\n    return pong()\n\ndef pong():\n    return ping()\n",
    ),
]
_CALLS_B = [
    (
        "pkg/b.py",
        b"def left():\n    return right()\n\ndef right():\n    return left()\n",
    ),
]


# Verifies MEDULLA-REPO-INGEST-R600.3: Wiring.
@pytest.mark.spec("MEDULLA-REPO-INGEST-R600.3")
def test_enrich_runs_once_after_index_commit(clean_graph) -> None:
    first = _index(clean_graph, "repo-enrich", _CALLS_A)
    assert first.files_parsed == 1

    runs = _wait_for_runs(clean_graph, 1)
    assert len(runs) == 1, f"one enrichment envelope per index envelope, got {runs}"
    assert runs[0].startswith("repository-enrich-run:")

    # Replaying the same index batch commits nothing new: the follow-on work
    # item and its envelope are keyed by the index digest.
    _index(clean_graph, "repo-enrich", _CALLS_A)
    time.sleep(2.0)
    assert _ids(clean_graph, "MATCH (:RepositoryEnrichmentRun)") == runs

    # A second, different index envelope gets exactly one more run.
    _index(clean_graph, "repo-enrich", _CALLS_B)
    second = _wait_for_runs(clean_graph, 2)
    assert len(second) == 2, f"one run per index envelope, got {second}"
    assert set(runs) < set(second)

    # The run read the committed call graph: each mutual-call pair is one
    # feature (an R011 community), found from the committed rows only.
    features = _ids(clean_graph, "MATCH (:CodeFeature)")
    assert len(features) >= 2, features


_WORKSPACE = [
    ("crates/core/Cargo.toml", b'[package]\nname = "core"\n'),
    (
        "services/api/pyproject.toml",
        b'[project]\nname = "api"\ndependencies = ["core>=1"]\n',
    ),
]


@pytest.mark.spec("MEDULLA-REPO-INGEST-R801")
def test_packages_and_layers_are_typed_nodes_read_through_uql(clean_graph) -> None:
    _index(clean_graph, "repo-workspace", _WORKSPACE)

    assert _ids(clean_graph, "MATCH (:EcosystemPackage)") == [
        "package:api",
        "package:core",
    ]
    assert _ids(clean_graph, "MATCH (:ArchitectureLayer)") == [
        "layer:crates",
        "layer:services",
    ]
    reached = _ids(
        clean_graph,
        "MATCH (:EcosystemPackage) |> TRAVERSE -[:dependsOnPackage]->{1,1}",
    )
    assert "package:core" in reached
    layers = _ids(
        clean_graph,
        "MATCH (:ArchitectureLayer) |> TRAVERSE -[:dependsOnLayer]->{1,1}",
    )
    assert "layer:crates" in layers

    cyclic = [
        (
            "crates/core/Cargo.toml",
            b'[package]\nname = "core"\n[dependencies]\napi = "1"\n',
        ),
        (
            "services/api/pyproject.toml",
            b'[project]\nname = "api"\ndependencies = ["core"]\n',
        ),
    ]
    with pytest.raises(Exception, match="LAYER_CYCLE"):
        _index(clean_graph, "repo-cyclic", cyclic)


def _commit(number: int, paths: list[str]) -> dict[str, Any]:
    return {
        "digest": f"{number:040x}",
        "author": "alice",
        "timestamp_ms": number,
        "parents": [],
        "changes": [
            {"path": p, "kind": "modified", "added_lines": 2, "removed_lines": 1}
            for p in paths
        ],
    }


_HISTORY = [_commit(n, ["pkg/a.py", "pkg/b.py"]) for n in (1, 2, 3)]


def _wait_for_ids(client: Any, query: str, count: int) -> list[str]:
    deadline = time.monotonic() + _WAIT_S
    ids = _ids(client, query)
    while len(ids) < count and time.monotonic() < deadline:
        time.sleep(0.25)
        ids = _ids(client, query)
    return ids


# Verifies MEDULLA-REPO-INGEST-R601.3: Wiring.
@pytest.mark.spec("MEDULLA-REPO-INGEST-R601.3")
def test_second_hydration_skips_stored_commits(clean_graph) -> None:
    _index(clean_graph, "repo-history", _CALLS_A, _HISTORY)
    applied = _wait_for_ids(clean_graph, "MATCH (:CommitHistory)", 3)
    assert len(applied) == 3, applied
    assert _ids(clean_graph, 'MATCH (:RepositoryHookRun) WHERE commits_added = "3"')

    # The second hydration sends the stored commits again plus one new commit.
    more = [*_HISTORY, _commit(4, ["pkg/a.py"])]
    _index(clean_graph, "repo-history", _CALLS_B, more)
    assert len(_wait_for_ids(clean_graph, "MATCH (:CommitHistory)", 4)) == 4
    skipped = _ids(
        clean_graph,
        'MATCH (:RepositoryHookRun) WHERE commits_skipped = "3"',
    )
    assert len(skipped) == 1, "the stored commits were skipped, not applied twice"


_OWNED = [
    ("services/billing/pyproject.toml", b'[project]\nname = "billing"\n'),
    (
        "services/billing/api.py",
        b'@app.get("/invoices")\ndef ping():\n    return pong()\n\n'
        b"def pong():\n    return ping()\n",
    ),
    (
        "services/billing/main.tf",
        b'resource "aws_s3_bucket" "invoices" {\n}\n',
    ),
]


# Verifies MEDULLA-REPO-INGEST-R602.3: Wiring.
@pytest.mark.spec("MEDULLA-REPO-INGEST-R602.3")
def test_link_pass_runs_after_features(clean_graph) -> None:
    _index(clean_graph, "repo-links", _OWNED)
    assert len(_wait_for_runs(clean_graph, 1)) == 1
    assert _ids(clean_graph, "MATCH (:CodeFeature)"), "features come first"

    for node_type in ("HttpRoute", "IacResource"):
        owners = _ids(
            clean_graph,
            f"MATCH (:{node_type}) |> TRAVERSE -[:ownedByService]->{{1,1}}",
        )
        assert "package:billing" in owners, (node_type, owners)
