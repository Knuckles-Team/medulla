"""``RetrievalContextSearch`` served round trip (MEDULLA-DECISION-ENGINE-R100.100.4).

The journey: store ``Document`` nodes (composed, decomposed and multibyte text), search
them through the generated client method, slice each span out of the original bytes
with its offsets, page with the cursor and meet the typed refusals. Runs against the
shared native engine through the ``clean_graph`` fixture.
"""

import hashlib

import pytest
from conftest import TEST_TENANT

from medulla.client import EngineResponseError

_COMPOSED = "un café noir, un thé vert"
_DECOMPOSED = "le café du matin, puis le café du soir"
_MULTIBYTE = "日本語 naïve café 日本語"


def _seed(gc):
    for node_id, text in (
        ("cs-composed", _COMPOSED),
        ("cs-decomposed", _DECOMPOSED),
        ("cs-multibyte", _MULTIBYTE),
    ):
        gc.nodes.add(node_id, {"type": "Document", "text": text})
    gc.nodes.add("cs-other", {"type": "Note", "text": "café in a note"})


def _edited(cursor):
    last = "1" if cursor.endswith("0") else "0"
    return cursor[:-1] + last


def _slice(text, start, end):
    return text.encode("utf-8")[start:end].decode("utf-8")


# spec: MEDULLA-DECISION-ENGINE-R100.100.4
def test_context_search_round_trip(clean_graph):
    gc = clean_graph
    _seed(gc)
    texts = {
        "cs-composed": _COMPOSED,
        "cs-decomposed": _DECOMPOSED,
        "cs-multibyte": _MULTIBYTE,
    }

    result = gc.graph.retrieval_context_search(
        tenant_id=TEST_TENANT, query="café", ngram_n=1
    )
    spans = result["spans"]
    assert {span["document_id"] for span in spans} == set(texts)
    assert [(s["document_id"], s["match_start"]) for s in spans] == sorted(
        (s["document_id"], s["match_start"]) for s in spans
    )
    for span in spans:
        original = texts[span["document_id"]]
        assert span["text_digest"] == hashlib.sha256(original.encode()).hexdigest()
        matched = _slice(original, span["match_start"], span["match_end"])
        assert matched in ("café", "café")
        context = _slice(original, span["context_start"], span["context_end"])
        assert context == span["context_text"]
        assert matched in context
    assert len(result["index_digest"]) == 64
    assert result["next_cursor"] is None

    # A page of one span, then the rest through the cursor.
    first = gc.graph.retrieval_context_search(
        tenant_id=TEST_TENANT, query="café", ngram_n=1, max_spans=1
    )
    assert len(first["spans"]) == 1
    assert first["next_cursor"]
    seen = list(first["spans"])
    cursor = first["next_cursor"]
    while cursor:
        page = gc.graph.retrieval_context_search(
            tenant_id=TEST_TENANT,
            query="café",
            ngram_n=1,
            max_spans=1,
            cursor=cursor,
        )
        seen.extend(page["spans"])
        cursor = page["next_cursor"]
    assert seen == spans

    # A cursor edited by the caller is refused; so is one for another query.
    with pytest.raises(EngineResponseError, match="CONTEXT_CURSOR_INVALID"):
        gc.graph.retrieval_context_search(
            tenant_id=TEST_TENANT,
            query="café",
            ngram_n=1,
            max_spans=1,
            cursor=_edited(first["next_cursor"]),
        )
    with pytest.raises(EngineResponseError, match="CONTEXT_CURSOR_INVALID"):
        gc.graph.retrieval_context_search(
            tenant_id=TEST_TENANT,
            query="verte",
            ngram_n=1,
            max_spans=1,
            cursor=first["next_cursor"],
        )

    # A write moves the snapshot, so the cursor goes stale.
    gc.nodes.add("cs-late", {"type": "Document", "text": "late café"})
    with pytest.raises(EngineResponseError, match="CONTEXT_CURSOR_STALE"):
        gc.graph.retrieval_context_search(
            tenant_id=TEST_TENANT,
            query="café",
            ngram_n=1,
            max_spans=1,
            cursor=first["next_cursor"],
        )

    # An empty query and another tenant's name are refused with typed codes.
    with pytest.raises(EngineResponseError, match="CONTEXT_QUERY_EMPTY"):
        gc.graph.retrieval_context_search(tenant_id=TEST_TENANT, query="   ")
    with pytest.raises(EngineResponseError, match="ACCESS_DENIED"):
        gc.graph.retrieval_context_search(tenant_id="tenant:other", query="café")
