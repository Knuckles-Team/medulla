"""MEDULLA-DECISION-ENGINE-R093.2.3 corpus training through the served fit job.

L4: the exported gold corpus goes to the running engine as the inline dataset of
a ``DecisionFit`` ``submit`` sent through ``DecisionClient.fit``, and
``DecisionClient.fit_status`` reads the job back. Every item of the corpus is
``SOURCE_IN_PROCESS``: nothing produces ``SOURCE_SERVED`` items yet, so this
test claims no served-ground-truth property.
"""

from __future__ import annotations

import asyncio
import os
from typing import Any

import pytest
from _decision_resident_fixture import publication
from conftest import TEST_TENANT, request_context

from medulla import decision_client as dc
from medulla import decision_stat as ds
from medulla.client import MedullaClient
from medulla.testing.synthetic import decision_export as export


async def _fit_the_corpus() -> tuple[dict[str, Any], dict[str, Any], dict[str, Any]]:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(scopes=["*"]),
    )
    try:
        published = await client._send(
            "AgentComponent",
            publication(
                "served-corpus-schema",
                "feature_schema",
                export.ASSEMBLY_FEATURE_SCHEMA,
            ),
        )
        schema = {
            "component_id": "served-corpus-schema",
            "kind": "feature_schema",
            "definition_digest": published["component"]["definition_digest"],
        }
        corpus = export.export_gold_corpus([1, 2])
        request = {
            "tenant_id": TEST_TENANT,
            "idempotency_key": "served-corpus-fit",
            "policy": {"policy": "default"},
            "window": {"from_ms": 0, "to_ms": (1 << 64) - 1},
            "source": {"source": "inline", "dataset": corpus},
            "head_kind": "option_attention",
            "feature_schema": schema,
            "label_regime": {
                "regime": "full_label",
                "gold_set_digest": ds.dataset_digest(corpus),
            },
            "optimiser": {
                "max_iterations": 60,
                "tolerance": {"scale": "q32", "value": 1 << 12},
                "seed": 0,
            },
        }
        decision = dc.DecisionClient(client=client)
        submitted = await decision.fit(request)
        status = await decision.fit_status(
            {"tenant_id": TEST_TENANT, "job_id": submitted["job_id"]}
        )
        return corpus, submitted, status
    finally:
        await client.close()


@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R093.2.3")
def test_served_fit_trains_on_the_exported_corpus(clean_graph):
    # spec: MEDULLA-DECISION-ENGINE-R093.2.3
    corpus, submitted, status = asyncio.run(_fit_the_corpus())

    assert corpus["items"], "the exported corpus has items"
    # Nothing produces served-ground-truth items, so every item is in-process.
    assert {item["label"]["source"] for item in corpus["items"]} == {
        export.SOURCE_IN_PROCESS
    }
    assert status["job_id"] == submitted["job_id"]
    assert status["state"]["state"] == "succeeded", status
    output = status["state"]["output"]
    assert output["output"] == "fit", output
    assert output["training_records_digest"].startswith("sha256:"), output
    assert output["synthetic"] is True, output
