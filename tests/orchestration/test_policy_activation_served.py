"""Served policy candidates and activation (MEDULLA-ORCH-VOCAB-R012.7.2 to R012.8).

A scoped ``IndexRepository`` over a repository whose constitution holds one scoped
``MUST NOT`` clause, one clause without a scope and one prose list item writes
exactly two ``PolicyCandidate`` nodes and no ``GuardrailRule``. A review of a
candidate set is recorded through ``DecisionLog`` ``learn``, judged by ``Decide``
kind ``promotion`` and moves the pointer
``champion:governed_set:policy:<repository>``; a ``Decide`` that names the
repository records the active revision and candidate-set digest as premises.
"""

from __future__ import annotations

import hashlib
import json
import uuid
from typing import Any

import pytest
from conftest import TEST_TENANT
from decide.test_task_plan_served import (  # noqa: F401  (fixture reused by name)
    authenticated_plan_client,
)
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


_SCOPE = "https://example.invalid/class/Repo"
_PATH = ".specify/memory/constitution.md"
# The graph the engine registers and the candidates are indexed into: the client
# default, the commons graph (every sibling served test names the same one).
_GRAPH = "__commons__"
_REVIEWABLE = (
    b"# Constitution\n\n"
    b"- MUST NOT commit secrets {scope=https://example.invalid/class/Repo}\n"
    b"- MUST NOT force push {scope=https://example.invalid/class/Repo}\n"
)
_REVISED = _REVIEWABLE + (
    b"- SHOULD review every change {scope=https://example.invalid/class/Repo}\n"
)


def _generation(invariants: tuple[tuple[str, str, str], ...] = ()) -> str:
    """The invariant generation: the digest of the sorted invariant lines."""
    lines = sorted("\t".join(invariant) for invariant in invariants)
    return "sha256:" + hashlib.sha256("\n".join(lines).encode()).hexdigest()


def _candidates(client: Any, repo: str) -> list[dict[str, Any]]:
    return [
        row
        for row in _rows(client, "PolicyCandidate")
        if str(row.get("id", "")).startswith(f"policycandidate:{repo}:")
    ]


def _review(
    rows: list[dict[str, Any]],
    seed: str,
    *,
    expected: str | None = None,
    author: str = "alice",
    reviewer: str = "bob",
) -> dict[str, Any]:
    """The receipt the approval row returns for the candidate set ``rows`` hold."""
    head = rows[0]
    binding: dict[str, Any] = {
        "repository": head["repo"],
        "path": head["path"],
        "commit": head["commit"],
        "blob_digest": head["blob_digest"],
        "candidate_set_digest": head["candidate_set_digest"],
        "review_receipt_digest": "sha256:" + hashlib.sha256(seed.encode()).hexdigest(),
        "invariant_generation": _generation(),
    }
    if expected is not None:
        binding["expected_policy_revision"] = expected
    return {"binding": binding, "author": author, "reviewer": reviewer}


def _record(client: Any, review: dict[str, Any]) -> dict[str, Any]:
    from medulla import decision_client as dc

    return client._send(
        "DecisionLog",
        {
            "op": dc.learn_op(
                TEST_TENANT, "record_policy_review", graph=_GRAPH, review=review
            )
        },
    )


def _subject(repo: str) -> dict[str, Any]:
    return {"kind": "governed_set", "scope_ref": f"policy:{repo}"}


def _decide(client: Any, review: dict[str, Any], **extra_params: str) -> dict[str, Any]:
    from _decision_resident_fixture import publish, schema_body

    from medulla import decision_client as dc

    binding = review["binding"]
    schema = publish(
        client,
        f"policy-schema-{uuid.uuid4().hex[:8]}",
        "feature_schema",
        schema_body(),
    )
    params = [
        dc.param("mode", "text", "promote"),
        dc.param("receipt_digest", "text", binding["review_receipt_digest"]),
        dc.param("subject", "text", json.dumps(_subject(binding["repository"]))),
    ]
    params += [dc.param(name, "text", value) for name, value in extra_params.items()]
    asked = dc.decide_request(
        TEST_TENANT,
        ("promote.policy", "promotion", "ordinary"),
        dc.declared(
            [dc.DeclaredOption(f"policyset:{binding['candidate_set_digest']}")]
        ),
        schema,
        params=params,
    )
    return client._send("Decide", {"request": asked})["records"][0]


def _commit(client: Any, record: dict[str, Any]) -> str:
    from medulla import decision_client as dc

    return client._send("DecisionLog", {"op": dc.commit_op(record)})["record_id"]


def _move(client: Any, review: dict[str, Any], record_id: str) -> dict[str, Any]:
    from medulla import decision_client as dc

    binding = review["binding"]
    return client._send(
        "DecisionLog",
        {
            "op": dc.learn_op(
                TEST_TENANT,
                "move_pointer",
                pointer={
                    "pointer": "champion",
                    "subject": _subject(binding["repository"]),
                },
                movement={
                    "movement": "activate",
                    "target": f"policyset:{binding['candidate_set_digest']}",
                    "receipt_digest": record_id,
                },
            )
        },
    )


def _active(client: Any, repo: str) -> str | None:
    from medulla import decision_client as dc

    try:
        pointer = client._send(
            "DecisionLog",
            {"op": dc.pointer_op(TEST_TENANT, f"champion:governed_set:policy:{repo}")},
        )
    except Exception as exc:
        if "POINTER_NOT_FOUND" in str(exc):
            return None
        raise
    return str(pointer["revision_ref"])


def _activate(client: Any, review: dict[str, Any]) -> None:
    evidence = _record(client, review)
    assert evidence["recorded"] == "policy_review", evidence
    record = _decide(client, review)
    assert "promotion.verdict=promote" in [p["fact"] for p in record["premises"]]
    moved = _move(client, review, _commit(client, record))
    assert moved["recorded"] == "pointer", moved


# Verifies MEDULLA-ORCH-VOCAB-R012.8: a graph edit never rewrites the source.
@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R012.8")
def test_graph_mutation_leaves_source_bytes_unchanged(
    authenticated_plan_client,  # noqa: F811
) -> None:
    client = authenticated_plan_client
    repo = f"repo-policy-bind-{uuid.uuid4().hex[:8]}"
    try:
        _index(client, repo, [(_PATH, _REVIEWABLE)])
        rows = _candidates(client, repo)
        assert len(rows) == 2, rows
        review = _review(rows, f"bind-{repo}")
        _activate(client, review)
        # The review names the digest of the committed bytes; no file was rewritten.
        assert (
            review["binding"]["blob_digest"] == hashlib.sha256(_REVIEWABLE).hexdigest()
        )
        # A graph edit beside the candidates does not move the recorded binding or the
        # source digest, and no operation rewrites the file.
        client.nodes.add(
            f"policynote:{repo}", {"node_type": "Note", "tenant_id": TEST_TENANT}
        )
        assert (
            _active(client, repo)
            == f"policyset:{review['binding']['candidate_set_digest']}"
        )
        assert {r["blob_digest"] for r in _candidates(client, repo)} == {
            review["binding"]["blob_digest"]
        }
    finally:
        client.close()


# Verifies MEDULLA-ORCH-VOCAB-R012.8: a missing or modified review keeps policy.
@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R012.8")
def test_missing_or_modified_review_preserves_prior_policy(
    authenticated_plan_client,  # noqa: F811
) -> None:
    client = authenticated_plan_client
    repo = f"repo-policy-review-{uuid.uuid4().hex[:8]}"
    try:
        _index(client, repo, [(_PATH, _REVIEWABLE)])
        first = _review(_candidates(client, repo), f"first-{repo}")
        _activate(client, first)
        prior = _active(client, repo)
        assert prior == f"policyset:{first['binding']['candidate_set_digest']}"

        # A source change is indexed again; its candidates have no recorded review.
        _index(client, repo, [(_PATH, _REVISED)])
        revised = _review(_candidates(client, repo), f"second-{repo}", expected=prior)
        unreviewed = _decide_or_error(client, revised)
        assert "PROMOTION_UNEVALUATED" in unreviewed, unreviewed
        assert _active(client, repo) == prior

        # A modified review (another reviewer under the recorded receipt) is refused.
        _record(client, revised)
        modified = json.loads(json.dumps(revised))
        modified["reviewer"] = "mallory"
        with pytest.raises(Exception, match="POLICY_REVIEW_MISMATCH"):
            _record(client, modified)
        # The first source's review no longer holds for the re-indexed source.
        with pytest.raises(Exception, match="POLICY_REVIEW_STALE"):
            _record(client, _review(_candidates_of(first), f"stale-{repo}"))
        assert _active(client, repo) == prior
    finally:
        client.close()


def _candidates_of(review: dict[str, Any]) -> list[dict[str, Any]]:
    """A candidate row carrying the binding of ``review`` (old digests)."""
    binding = review["binding"]
    return [
        {
            "repo": binding["repository"],
            "path": binding["path"],
            "commit": binding["commit"],
            "blob_digest": binding["blob_digest"],
            "candidate_set_digest": binding["candidate_set_digest"],
        }
    ]


def _decide_or_error(client: Any, review: dict[str, Any]) -> str:
    """The abstention a promotion of an unreviewed set answers, as text."""
    record = _decide(client, review)
    return json.dumps(record["outcome"])


# Verifies MEDULLA-ORCH-VOCAB-R012.7.3: review, activation and a decision on it.
@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R012.7.3")
def test_review_activate_and_decide_uses_active_rule(
    authenticated_plan_client,  # noqa: F811
) -> None:
    client = authenticated_plan_client
    repo = f"repo-policy-activate-{uuid.uuid4().hex[:8]}"
    try:
        _index(client, repo, [(_PATH, _REVIEWABLE)])
        review = _review(_candidates(client, repo), f"activate-{repo}")
        # The author cannot approve their own set: the gate abstains and nothing moves.
        selfie = _review(_candidates(client, repo), f"self-{repo}", reviewer="alice")
        _record(client, selfie)
        abstained = _decide(client, selfie)
        assert "promotion.verdict=promote" not in [
            p["fact"] for p in abstained["premises"]
        ]
        assert _active(client, repo) is None

        _activate(client, review)
        target = f"policyset:{review['binding']['candidate_set_digest']}"
        assert _active(client, repo) == target

        # The decision that names the repository reads the active policy.
        record = _decide(client, review, policy_repository=repo)
        facts = [premise["fact"] for premise in record["premises"]]
        assert f"policy.revision={target}" in facts, facts
        assert (
            f"policy.candidate_set_digest={review['binding']['candidate_set_digest']}"
            in facts
        ), facts
    finally:
        client.close()


# Verifies MEDULLA-ORCH-VOCAB-R012.7.4: decisions record the active policy revision.
@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R012.7.4")
def test_decision_records_active_policy_revision(
    authenticated_plan_client,  # noqa: F811
) -> None:
    client = authenticated_plan_client
    repo = f"repo-policy-evidence-{uuid.uuid4().hex[:8]}"
    try:
        _index(client, repo, [(_PATH, _REVIEWABLE)])
        review = _review(_candidates(client, repo), f"evidence-{repo}")
        none = _decide(client, review, policy_repository=repo)
        assert "policy.revision=none" in [p["fact"] for p in none["premises"]]
        _activate(client, review)
        target = f"policyset:{review['binding']['candidate_set_digest']}"
        digests: set[str] = set()
        for _ in range(2):
            record = _decide(client, review, policy_repository=repo)
            by_fact = {p["fact"] for p in record["premises"]}
            assert f"policy.revision={target}" in by_fact
            digests.update(
                f for f in by_fact if f.startswith("policy.candidate_set_digest=")
            )
        # An equal revision reproduces the digest.
        assert digests == {
            f"policy.candidate_set_digest={review['binding']['candidate_set_digest']}"
        }
    finally:
        client.close()
