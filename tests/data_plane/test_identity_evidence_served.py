"""Served identity evidence of `Decide` with `resolve_entity`.

MEDULLA-UNIFIED-DATA-PLANE-R030.701: each evidence kind is a premise
`identity:<kind>`, an opaque token never merges on the name alone, and an
approval without an approver refuses with `LINK_APPROVER_MISSING`.
"""

from __future__ import annotations

import asyncio
import os

import pytest
from _decision_resident_fixture import publish
from conftest import TEST_TENANT, request_context

from medulla import decision_client as dc
from medulla import decision_stat as ds
from medulla.client import MedullaClient

OPAQUE = "x7Qp9ZkL2mWv"


def _schema_body() -> dict:
    return ds.feature_schema_body(
        [ds.feature("similarity", ds.number("similarity"), impute=0)]
    )


def _options(*pairs: tuple[str, str, str | None]) -> list[dc.DeclaredOption]:
    options = []
    for option_id, name, key in pairs:
        texts = {"name": name}
        if key is not None:
            texts["key"] = key
        options.append(dc.DeclaredOption(option_id, texts=texts))
    return options


async def _decide(schema: dict, question_id: str, options, params=()):
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
    )
    try:
        request = dc.decide_request(
            TEST_TENANT,
            (question_id, "resolve_entity", "ordinary"),
            dc.declared(options),
            schema,
            params=list(params),
        )
        return await dc.DecisionClient(client=client).decide(request)
    finally:
        await client.close()


def _facts(payload: dict) -> list[str]:
    return [premise["fact"] for premise in payload["records"][0]["premises"]]


@pytest.mark.spec("MEDULLA-UNIFIED-DATA-PLANE-R030.701")
def test_high_entropy_names_do_not_merge(clean_graph):
    schema = publish(clean_graph, "r030-701-schema", "feature_schema", _schema_body())

    opaque = asyncio.run(
        _decide(
            schema,
            "q:identity:opaque",
            _options(("a", OPAQUE, None), ("b", OPAQUE, None)),
        )
    )
    facts = _facts(opaque)
    assert "identity:entropy_qualified_token" in facts
    assert "identity:normalized_name" not in facts
    assert "identity:decision:no_merge" in facts
    assert "identity:decision:propose_merge" not in facts

    kinds = asyncio.run(
        _decide(
            schema,
            "q:identity:kinds",
            _options(
                ("c", "Ada Lovelace", "ada@x.org"),
                ("d", "ada lovelace", "ada@x.org"),
                ("e", "Report Builder v2", None),
                ("f", "Report Builder v3", None),
            ),
        )
    )
    kind_facts = _facts(kinds)
    assert "identity:exact_key" in kind_facts
    assert "identity:version_variant" in kind_facts
    assert "identity:decision:propose_merge" in kind_facts
    assert "identity:decision:propose_link" in kind_facts

    with pytest.raises(Exception, match="LINK_APPROVER_MISSING"):
        asyncio.run(
            _decide(
                schema,
                "q:identity:approve",
                _options(("a", "Ada", None), ("b", "Ada", None)),
                params=[dc.param("approve_link", "bool", True)],
            )
        )
