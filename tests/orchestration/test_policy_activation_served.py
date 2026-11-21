"""Served policy candidates (MEDULLA-ORCH-VOCAB-R012.7.2).

A scoped ``IndexRepository`` over a repository whose constitution holds one scoped
``MUST NOT`` clause, one clause without a scope and one prose list item writes exactly two
``PolicyCandidate`` nodes and no ``GuardrailRule``.
"""

from __future__ import annotations

import pytest
from test_spec_folder_served import _index, _rows

_CONSTITUTION = (
    b"# Constitution\n\n"
    b"- MUST NOT commit secrets {scope=https://example.invalid/class/Repo}\n"
    b"- SHOULD review every change\n"
    b"- A prose item that states no rule\n"
)


# Verifies MEDULLA-ORCH-VOCAB-R012.7.2: indexing yields inactive candidates with spans.
@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R012.7.2")
def test_index_yields_inactive_candidates_with_spans(clean_graph) -> None:
    repo = "repo-policy-candidates"
    _index(clean_graph, repo, [(".specify/memory/constitution.md", _CONSTITUTION)])

    rows = [
        row
        for row in _rows(clean_graph, "PolicyCandidate")
        if str(row.get("id", "")).startswith(f"policycandidate:{repo}:")
    ]
    assert len(rows) == 2, rows
    by_status = {row["status"]: row for row in rows}
    proposed, inactive = by_status["proposed"], by_status["inactive"]
    assert (int(proposed["start_line"]), int(proposed["end_line"])) == (3, 3)
    assert proposed["severity"] == "block"
    assert proposed["scope"] == "https://example.invalid/class/Repo"
    assert inactive["reason"] == "CLAUSE_UNPARSED"
    assert (int(inactive["start_line"]), int(inactive["end_line"])) == (4, 4)
    assert proposed["candidate_set_digest"] == inactive["candidate_set_digest"]
    assert int(proposed["grammar_revision"]) == 1
    assert [
        row
        for row in _rows(clean_graph, "GuardrailRule")
        if repo in str(row.get("id", ""))
    ] == []
