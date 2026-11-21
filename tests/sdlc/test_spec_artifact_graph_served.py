"""Incremental ingest by file digest (MEDULLA-SDLC-R014.2).

L3: the engine keeps one digest per spec artifact file and re-ingests only the
files whose digest changed. A file that leaves the tree loses its records. The
job is `reingest_changed_artifacts` in
`crates/medulla-h-ingest/src/source_ingestion/spec_projection.rs`; this test
drives it through the scoped crate test, which also asserts the one-second
budget for a one-file change.
"""

from __future__ import annotations

import hashlib
import os
import re
import subprocess
from pathlib import Path
from typing import Any

import pytest

from medulla.generated.index_repository import IndexRepositoryScope

# The reingest tests run in scoped crate tests and own no engine; the wiring test uses the
# shared engine.
no_engine = pytest.mark.no_engine

ROOT = Path(__file__).resolve().parents[2]
PROJECTION_RS = (
    ROOT
    / "crates"
    / "medulla-h-ingest"
    / "src"
    / "source_ingestion"
    / "spec_projection.rs"
)
REINGEST_TEST = (
    "source_ingestion::spec_projection::tests::one_file_change_reingests_only_that_file"
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
            f"engine reingest test {name} failed:\n{proc.stdout}\n{proc.stderr}"
        )
    return proc.stdout


@no_engine
@pytest.mark.spec("MEDULLA-SDLC-R014.2")
def test_one_file_change_reingests_only_that_file_within_budget() -> None:
    assert "pub fn reingest_changed_artifacts" in PROJECTION_RS.read_text(
        encoding="utf-8"
    )
    out = _run_engine_test(REINGEST_TEST)

    # spec: MEDULLA-SDLC-R014.2
    assert re.search(r"^REINGEST\tmore/requirements.md$", out, flags=re.MULTILINE)
    assert re.search(r"^UNCHANGED\t1$", out, flags=re.MULTILINE)
    assert re.search(r"^REMOVED\tmore/requirements.md$", out, flags=re.MULTILINE)


CITE_TEST = (
    "source_ingestion::spec_projection::tests::"
    "index_batch_cites_requirements_once_per_changeset"
)


@no_engine
@pytest.mark.spec("MEDULLA-SDLC-R014.3")
def test_trailers_become_citation_edges() -> None:
    assert "pub fn cite_requirements" in PROJECTION_RS.read_text(encoding="utf-8")
    out = _run_engine_test(CITE_TEST)

    # spec: MEDULLA-SDLC-R014.3
    assert re.search(
        r"^CITE\tchangeset:demo:c1\trequirement:demo:DEMO-R001$", out, flags=re.MULTILINE
    )
    assert re.search(
        r"^CITE_REFUSED\tREQUIREMENT_UNRESOLVED: DEMO-R999 \(cited by changeset c2\)",
        out,
        flags=re.MULTILINE,
    )


_REPO = "repo-spec-artifacts"
_REVISION = "e" * 40
_REQUIREMENTS = (
    b"# Requirements\n\n"
    b"| ID | Requirement | Acceptance |\n|----|----|----|\n"
    b"| `DEMO-R001` | **Serve.** Caller chain: `serve` -> `src/demo.py:1`. | "
    b"Test: `tests/test_demo.py::test_serve` (L3). |\n"
)
_FILES = [
    ("src/demo.py", b"def serve():\n    return 1\n"),
    ("specs/demo/requirements.md", _REQUIREMENTS),
]


def _rows(client: Any, label: str) -> list[dict[str, Any]]:
    return client.query.uql(f"MATCH (n:{label}) RETURN n")["rows"]


@pytest.mark.spec("MEDULLA-SDLC-R014.8")
def test_repository_ingest_includes_spec_artifacts(clean_graph) -> None:
    scope = IndexRepositoryScope.model_validate(
        {
            "repository_id": _REPO,
            "commits": [],
            "refs": [
                {"ref_name": "refs/heads/main", "revision_id": _REVISION, "status": "live"}
            ],
            "file_versions": [
                {
                    "ref_name": "refs/heads/main",
                    "path": path,
                    "blob_digest": "sha256:" + hashlib.sha256(body).hexdigest(),
                }
                for path, body in _FILES
            ],
        }
    )
    try:
        clean_graph.graph.index_repository(_FILES, scope=scope)
    except Exception as exc:
        text = str(exc)
        if "AST feature not enabled" in text or "requires durable persistence" in text:
            pytest.skip("engine built without the ast feature or durable persistence")
        raise

    # spec: MEDULLA-SDLC-R014.8
    # One scoped index pass wrote both the code graph and the spec artifact records.
    assert _rows(clean_graph, "SYMBOL"), "the code graph has no symbol of src/demo.py"
    requirements = [str(row.get("id", "")) for row in _rows(clean_graph, "Requirement")]
    assert requirements == [f"requirement:{_REPO}:DEMO-R001"]
    artifacts = [str(row.get("id", "")) for row in _rows(clean_graph, "SpecArtifactFile")]
    assert any(artifact.endswith("specs/demo/requirements.md") for artifact in artifacts)
