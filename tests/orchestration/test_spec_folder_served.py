"""Served spec folder classification (MEDULLA-ORCH-VOCAB-R012.4, R012.5, R012.6).

The entry point is a scoped ``IndexRepository`` over a fixture spec folder. The spec
projection of the repository index classifies each spec file by a named rule, writes one
node per file with its rule, source path and blob digest, and writes a file no rule matches
as a plain ``Document``. The graph is read back through ``Method::Uql``.
"""

from __future__ import annotations

import hashlib
from typing import Any

import pytest

from medulla.generated.index_repository import IndexRepositoryScope

_REVISION = "d" * 40

_FOLDER = [
    ("specs/demo/tasks.md", b"- [ ] T001 write the thing\n"),
    ("specs/demo/goal.md", b"---\ngoal: ship the thing\n---\n# Goal\n"),
    ("specs/demo/spec.md", b"# Demo spec\n"),
    ("specs/demo/memory/lesson.md", b"# Lesson\nKeep it small.\n"),
    (".specify/memory/constitution.md", b"# Constitution\nNo secrets in specs.\n"),
]
_UNKNOWN = ("specs/demo/notes.md", b"loose notes with no rule\n")

_KINDS = {
    "SpecTask": "specs/demo/tasks.md",
    "SpecGoal": "specs/demo/goal.md",
    "Specification": "specs/demo/spec.md",
    "SpecMemory": "specs/demo/memory/lesson.md",
    "SpecPolicy": ".specify/memory/constitution.md",
}
_RULES = {
    "specs/demo/tasks.md": "tasks-file",
    "specs/demo/goal.md": "goal-front-matter",
    "specs/demo/spec.md": "spec-document",
    "specs/demo/memory/lesson.md": "memory-folder",
    ".specify/memory/constitution.md": "constitution-file",
}


def _scope(repository_id: str, files: list[tuple[str, bytes]]) -> IndexRepositoryScope:
    return IndexRepositoryScope.model_validate(
        {
            "repository_id": repository_id,
            "commits": [],
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


def _index(client: Any, repository_id: str, files: list[tuple[str, bytes]]) -> Any:
    try:
        return client.graph.index_repository(
            files, scope=_scope(repository_id, files)
        )
    except Exception as exc:
        text = str(exc)
        if "AST feature not enabled" in text or "requires durable persistence" in text:
            pytest.skip("engine built without the ast feature or durable persistence")
        raise


def _rows(client: Any, label: str) -> list[dict[str, Any]]:
    return client.query.uql(f"MATCH (n:{label}) RETURN n")["rows"]


def _node_id(repository_id: str, path: str) -> str:
    return f"speckind:{repository_id}:{path}"


def _classified(client: Any, repository_id: str) -> dict[str, dict[str, Any]]:
    out: dict[str, dict[str, Any]] = {}
    for label in (*_KINDS, "Document"):
        for row in _rows(client, label):
            node_id = str(row.get("id", ""))
            if node_id.startswith(f"speckind:{repository_id}:"):
                out[node_id] = row
    return out


# Verifies MEDULLA-ORCH-VOCAB-R012.6: Hot-path proof, spec folder yields five kinds.
@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R012.6")
def test_index_repository_on_spec_folder_yields_five_kinds(clean_graph) -> None:
    _index(clean_graph, "repo-spec-kinds", _FOLDER)

    for label, path in _KINDS.items():
        ids = [
            str(row.get("id", ""))
            for row in _rows(clean_graph, label)
            if str(row.get("id", "")).startswith("speckind:repo-spec-kinds:")
        ]
        assert ids == [_node_id("repo-spec-kinds", path)], (label, ids)


# Verifies MEDULLA-ORCH-VOCAB-R012.5: Evidence, each node records its deciding rule.
@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R012.5")
def test_node_records_deciding_rule(clean_graph) -> None:
    _index(clean_graph, "repo-spec-rules", _FOLDER)

    classified = _classified(clean_graph, "repo-spec-rules")
    assert len(classified) == len(_RULES)
    for path, rule in _RULES.items():
        row = classified[_node_id("repo-spec-rules", path)]
        assert row["classifiedByRule"] == rule, path
        assert row["path"] == path
        body = dict(_FOLDER)[path]
        assert row["blob_digest"] == "sha256:" + hashlib.sha256(body).hexdigest()


# Verifies MEDULLA-ORCH-VOCAB-R012.4: Refusal, an unknown file is not guessed.
@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R012.4")
def test_unknown_file_becomes_plain_document(clean_graph) -> None:
    files = [*_FOLDER, _UNKNOWN]
    result = _index(clean_graph, "repo-spec-unknown", files)

    row = _classified(clean_graph, "repo-spec-unknown")[
        _node_id("repo-spec-unknown", _UNKNOWN[0])
    ]
    assert row["path"] == _UNKNOWN[0]
    assert "classifiedByRule" not in row or row["classifiedByRule"] in (None, "")
    outcome = next(o for o in result.file_outcomes if o.file_path == _UNKNOWN[0])
    assert [(d.code, d.message) for d in outcome.diagnostics] == [
        ("SPEC_FILE_UNKNOWN_KIND", _UNKNOWN[0])
    ]
