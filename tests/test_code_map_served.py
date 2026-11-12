"""Served revision-bound code maps.

MEDULLA-REPO-INGEST-R006.3: ``Method::UnifiedQuery`` with a ``code_map`` field
serves the files, symbols and relations of the repository's current indexed
revision. A request for any other revision, or a cursor minted for an older map
generation, refuses with ``CODE_MAP_STALE`` until the repository is reindexed.
"""

from __future__ import annotations

import hashlib
from typing import Any

import pytest

from medulla.generated.index_repository import IndexRepositoryScope

_REV_A = "a" * 40
_REV_B = "b" * 40
_REPO = "repo-code-map"

_BILLING_V1 = b"def parse_invoice(text):\n    return text.strip()\n"
_BILLING_V2 = b"# parser now trims\ndef parse_invoice(text):\n    return text.strip()\n"
_RENDER = b"def render_page(body):\n    return parse_invoice(body)\n"


def _index(client: Any, revision: str, files: list[tuple[str, bytes]]) -> None:
    scope = IndexRepositoryScope.model_validate(
        {
            "repository_id": _REPO,
            "commits": [],
            "refs": [
                {
                    "ref_name": "refs/heads/main",
                    "revision_id": revision,
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
    try:
        client.graph.index_repository(files, scope=scope)
    except Exception as exc:
        text = str(exc)
        if "AST feature not enabled" in text or "requires durable persistence" in text:
            pytest.skip("engine built without the ast feature or durable persistence")
        raise


def _symbols(page: Any) -> dict[str, Any]:
    return {entry.name: entry for entry in page.entries if entry.kind != "file"}


@pytest.mark.spec("MEDULLA-REPO-INGEST-R006.3")
def test_current_map_locates_behavior_and_refuses_old_generation(clean_graph) -> None:
    _index(
        clean_graph,
        _REV_A,
        [("billing/parse.py", _BILLING_V1), ("ui/render.py", _RENDER)],
    )

    first = clean_graph.query.code_map(
        _REPO, _REV_A, behavior_text="parse invoice", page_limit=50
    )
    parse_a = _symbols(first)["parse_invoice"]
    assert parse_a.path == "billing/parse.py"
    assert parse_a.line == 1
    assert first.inventory_digest.startswith("sha256:")
    assert first.query_digest.startswith("sha256:")
    assert first.auth_scope_digest.startswith("sha256:")
    assert any(
        edge.relation.value == "Contains" and edge.target == parse_a.id
        for edge in first.edges
    )

    full = clean_graph.query.code_map(_REPO, _REV_A, page_limit=50)
    render_a = _symbols(full)["render_page"]

    # A revision the repository has not been indexed at refuses.
    with pytest.raises(Exception, match="CODE_MAP_STALE"):
        clean_graph.query.code_map(_REPO, _REV_B, page_limit=50)

    # One symbol changes and the repository is reindexed at the new revision.
    _index(
        clean_graph,
        _REV_B,
        [("billing/parse.py", _BILLING_V2), ("ui/render.py", _RENDER)],
    )
    with pytest.raises(Exception, match="CODE_MAP_STALE"):
        clean_graph.query.code_map(_REPO, _REV_A, page_limit=50)

    after = clean_graph.query.code_map(_REPO, _REV_B, page_limit=50)
    assert after.map_generation != first.map_generation
    assert after.inventory_digest != first.inventory_digest
    symbols = _symbols(after)
    assert symbols["parse_invoice"].line == 2, "the new span is served"
    assert symbols["render_page"].id == render_a.id, "unchanged logical id"


@pytest.mark.spec("MEDULLA-REPO-INGEST-R006.3")
def test_cursor_binds_generation_and_page_limit_is_bounded(clean_graph) -> None:
    body = b"".join(f"def fn_{n}():\n    return {n}\n".encode() for n in range(5))
    _index(clean_graph, _REV_A, [("pkg/many.py", body)])

    page = clean_graph.query.code_map(_REPO, _REV_A, page_limit=2)
    assert len(page.entries) == 2
    assert page.next_cursor is not None
    seen = {entry.id for entry in page.entries}
    while page.next_cursor is not None:
        page = clean_graph.query.code_map(
            _REPO, _REV_A, page_limit=2, cursor=page.next_cursor
        )
        assert not seen & {entry.id for entry in page.entries}
        seen |= {entry.id for entry in page.entries}
    assert len(seen) == 6, "one file and five symbols"

    first = clean_graph.query.code_map(_REPO, _REV_A, page_limit=2)
    _index(
        clean_graph, _REV_B, [("pkg/many.py", body + b"\ndef extra():\n    return 0\n")]
    )
    with pytest.raises(Exception, match="CODE_MAP_STALE"):
        clean_graph.query.code_map(
            _REPO, _REV_B, page_limit=2, cursor=first.next_cursor
        )

    with pytest.raises(Exception, match="CODE_MAP_INVALID|page_limit"):
        clean_graph.query.code_map(_REPO, _REV_B, page_limit=1001)
