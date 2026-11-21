"""MEDULLA-DECISION-ENGINE-R829.3 served proof: a 1,449-candidate synthetic
catalog ranks the named system and object first through `Decide` with
`catalog.operation_rank` and `CandidateSource::RankCatalog`.

The catalog is generated in this repository (`fixtures/capability_catalog.py`);
no fixture of another repository is read. The test publishes the feature schema,
a fitted head and the advisory policy, then asks the whole catalog with no
prefilter.
"""

from __future__ import annotations

import math
import os
import sys
import time
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent / "fixtures"))
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "retrieval"))

import pytest
from _decision_resident_fixture import job_requests, publish
from capability_catalog import catalog_options, id_of
from conftest import (
    TEST_AGENT_ID,
    TEST_SIGNER_KEY,
    TEST_TENANT,
    bootstrap_context,
    find_server_binary,
    spawn_server,
    stop_server,
    strict_server_env,
    wait_for_server,
)
from expansion_harness import RUST_DIR, SECRET, connect
from test_ranked_decide_served import _publish_policy

from medulla import decision_client as dc
from medulla import decision_stat as ds
from medulla.client import EngineResponseError, SyncMedullaClient

Q32 = 1 << 32
QUESTION_ID = "catalog.operation_rank"
SCHEMA_ID = f"decide.schema.{QUESTION_ID}"
FEATURES = (
    "name_overlap",
    "description_overlap",
    "term_rarity_weighted_overlap",
    "name_coverage",
    "semantic_similarity",
)
QUERY = "list pending media requests in quillbox"
VERBS_OTHER_THAN_LIST = (
    "get",
    "create",
    "update",
    "delete",
    "search",
    "approve",
    "decline",
    "export",
)


def _schema_body() -> dict[str, Any]:
    return ds.feature_schema_body(
        [ds.feature(n, ds.number(f"rank.{n}"), impute=0) for n in FEATURES]
    )


def _catalog_dataset(body: dict[str, Any]) -> dict[str, Any]:
    """400 items over three candidates; only the rarity feature carries signal."""
    ids = ["cand-0", "cand-1", "cand-2"]
    rarity = FEATURES.index("term_rarity_weighted_overlap")
    items = []
    for i in range(400):
        gold = i % 3
        values = [0] * (3 * len(FEATURES))
        for c in range(3):
            values[c * len(FEATURES) + rarity] = (
                Q32 * (10 + i % 10) if c == gold else Q32 * (i % 4)
            )
        items.append(
            dict(
                item_id=f"gold-{i:04}",
                recorded_at_ms=1000,
                class_key="rank",
                candidate_ids=ids,
                features=values,
                label={
                    "label": "gold",
                    "acceptable": [f"cand-{gold}"],
                    "source": "synthetic_construction",
                },
                audit_inclusion=None,
            )
        )
    return dict(
        schema_version=1,
        feature_schema_digest=ds.encode_body(body)[0],
        feature_names=list(FEATURES),
        scale="q32",
        items=items,
        synthetic=True,
    )


def _publish_all(client: Any) -> tuple[dict[str, Any], dict[str, Any], dict[str, Any]]:
    """The schema, head and policy the catalog question is asked with."""
    body = _schema_body()
    schema = publish(client, SCHEMA_ID, "feature_schema", body)
    data = _catalog_dataset(body)
    fit = job_requests(schema, data)
    fit["op"]["request"]["head_kind"] = "listwise_logistic"
    fitted = client._send("DecisionFit", fit)
    assert fitted["state"]["state"] == "succeeded", fitted
    draft = fitted["state"]["output"]
    evaluated = client._send("DecisionEval", job_requests(schema, data, draft))
    assert evaluated["state"]["state"] == "succeeded", evaluated
    receipt = evaluated["state"]["output"]["receipt"]
    assert receipt["passed"], receipt["failed_gates"]
    head = publish(
        client,
        "catalog-rank-head",
        "decision_head",
        draft["draft"],
        receipt["receipt_digest"],
    )
    return schema, head, _publish_policy(client)


def _ask(client, schema, head, policy, options) -> dict[str, Any]:
    request = dc.decide_request(
        TEST_TENANT,
        (QUESTION_ID, "rank", "ordinary"),
        dc.rank_catalog(options),
        schema,
        head=head,
        policy={"policy": "pinned", "component": policy},
        params=[dc.param("query", "text", QUERY)],
    )
    request["belief_as_of"] = []
    return client._send("Decide", {"request": request})["records"][0]


def _logged(client) -> int:
    listed = client._send(
        "DecisionLog", {"op": {"op": "list", "tenant_id": TEST_TENANT, "limit": 200}}
    )
    return len(listed["records"])


def _assert_first_sets(record: dict[str, Any]) -> None:
    outcome = record["outcome"]
    assert outcome["outcome"] == "advisory", outcome
    ids = [score["option_id"] for score in outcome["scores"]]
    assert outcome["ranked_total"] == 1449
    assert len(ids) == 64
    top = id_of("quillbox_list_pending_request")
    first_three = {
        top,
        id_of("quillbox_list_media"),
        id_of("quillbox_list_request"),
    }
    first_eleven = first_three | {
        id_of(f"quillbox_{v}_pending_request") for v in VERBS_OTHER_THAN_LIST
    }
    assert ids[0] == top
    assert set(ids[:3]) == first_three
    assert set(ids[:11]) == first_eleven


def _semantic_column(record: dict[str, Any]) -> list[int]:
    matrix = record["inputs"]["feature_matrix"]
    names = matrix["feature_names"]
    column = next(i for i, n in enumerate(names) if n.endswith("semantic_similarity"))
    width = len(names)
    return [matrix["values"][r * width + column] for r in range(len(matrix["candidate_ids"]))]


# spec: MEDULLA-DECISION-ENGINE-R829.3
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R829.3")
def test_named_system_and_object_rank_first_in_a_large_catalog(clean_graph):
    schema, head, policy = _publish_all(clean_graph)
    record = _ask(clean_graph, schema, head, policy, catalog_options())
    _assert_first_sets(record)
    # The shared engine binds an embedder: some retained row has a semantic value.
    assert any(_semantic_column(record))


# spec: MEDULLA-DECISION-ENGINE-R829.3
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R829.3")
def test_more_than_two_thousand_candidates_refuse_typed(clean_graph):
    schema, head, policy = _publish_all(clean_graph)
    options = catalog_options() + [
        dc.DeclaredOption(
            f"op-{n:04d}",
            texts={
                "rank.name": f"extra_{n}_list_request",
                "rank.description": f"extra {n} list request",
                "rank.kind": "operation",
            },
        )
        for n in range(1449, 2001)
    ]
    assert len(options) == 2001
    before = _logged(clean_graph)
    with pytest.raises(EngineResponseError, match="CANDIDATE_SET_TOO_LARGE"):
        _ask(clean_graph, schema, head, policy, options)
    assert _logged(clean_graph) == before


# spec: MEDULLA-DECISION-ENGINE-R829.3
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R829.3")
def test_p99_ceiling_holds_on_a_warm_slot(clean_graph):
    schema, head, policy = _publish_all(clean_graph)
    options = catalog_options()
    _ask(clean_graph, schema, head, policy, options)
    times = []
    for _ in range(50):
        started = time.perf_counter()
        _ask(clean_graph, schema, head, policy, options)
        times.append(time.perf_counter() - started)
    p99 = sorted(times)[math.ceil(0.99 * 50) - 1]
    assert p99 <= 0.150, f"p99 {p99 * 1000:.1f} ms exceeds 150 ms"


@contextmanager
def _engine_without_embedder(tmp_path_factory) -> Iterator[str]:
    """A private engine started like `expansion_harness.served_engine`, with no
    text embedder bound."""
    runtime = tmp_path_factory.mktemp("rank-no-embedder")
    sock = str(runtime / "engine.sock")
    persist_dir = str(runtime / "persist")
    os.makedirs(persist_dir, exist_ok=True)
    log_path = runtime / "server.log"
    env = {
        **os.environ,
        **strict_server_env(
            str(runtime / "security"), auth_secret=SECRET, persist_dir=persist_dir
        ),
        "MEDULLA_ENCRYPTION_KEY": "test-medulla-encryption-key",
    }
    for name in list(env):
        if name == "MEDULLA_UQL_TEXT_EMBEDDER" or name.startswith("MEDULLA_EMBED"):
            env.pop(name)
    binary = find_server_binary() or os.path.join(
        RUST_DIR, "target", "debug", "medulla-server"
    )
    proc = spawn_server([binary, "--socket-path", sock], env=env, log_path=log_path)
    wait_for_server(proc, sock, name="rank server", log_path=log_path)
    bootstrap = SyncMedullaClient.connect(
        socket_path=sock, auth_secret=SECRET, verified_context=bootstrap_context()
    )
    try:
        bootstrap.consensus.bootstrap_system_identity(
            agent_id=TEST_AGENT_ID, signer_id=TEST_AGENT_ID, signer_key=TEST_SIGNER_KEY
        )
    finally:
        bootstrap.close()
    try:
        yield sock
    finally:
        stop_server(proc, name="rank server", log_path=log_path)


# spec: MEDULLA-DECISION-ENGINE-R829.3
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R829.3")
def test_stopped_embedding_model_keeps_the_lexical_head(clean_graph, tmp_path_factory):
    # The shared engine binds an embedder: some retained row has a semantic value.
    schema, head, policy = _publish_all(clean_graph)
    shared = _ask(clean_graph, schema, head, policy, catalog_options())
    assert any(_semantic_column(shared))
    with _engine_without_embedder(tmp_path_factory) as sock:
        private = connect(sock)
        try:
            schema, head, policy = _publish_all(private)
            record = _ask(private, schema, head, policy, catalog_options())
        finally:
            private.close()
    _assert_first_sets(record)
    assert not any(_semantic_column(record))
