"""Answer provenance and zero copy for pack sources (MEDULLA-FEDERATED-QUERY-R722.5).

The journey of ``tests/connector_competency/test_contract_kinds_served.py`` serves
``ConnectorPack.Import``, ``ApproveSource`` and the bounded ``ForeignRead*`` lifecycle,
then answers ``FOREIGN SCAN`` through the served ``Uql``. This test reads that answer
back: every row names its connector, contract IRI, mapping IRI and fetch time, the read
counted once against the source's catalog asset, and the store holds no source row.
"""

from __future__ import annotations

import json
import sys
import time
import uuid
from pathlib import Path
from typing import Any

import msgpack
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "connector_competency"))

from conftest import TEST_AGENT_ID
from test_contract_kinds_served import (
    RECORDING_ITEMS,
    SOURCE_ID,
    _contract_iri,
    _run_journey,
)


def _usage_reads(client: Any, asset: str) -> int:
    """The test principal's served reads of `asset`.

    The usage ledger is graph-scoped and shared by every principal, so it is read
    through the served `CatalogUsage` answer (MEDULLA-UNIFIED-DATA-PLANE-R764.8);
    `TsRange` reads only the caller's own private series.
    """
    try:
        answer = client._send("CatalogUsage", {"asset": asset, "top_n": None}, None)
    except Exception as refused:
        if "CATALOG_ASSET_NOT_FOUND" in str(refused):
            return 0
        raise
    if isinstance(answer, (bytes, bytearray)):
        answer = msgpack.unpackb(answer, raw=False)
    return sum(
        int(user["reads"])
        for user in answer["frequent_users"]
        if user["principal"] == TEST_AGENT_ID
    )


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R722.5")
def test_rows_carry_contract_provenance_and_nothing_is_copied(
    clean_graph, tmp_path
) -> None:
    before_usage = _usage_reads(clean_graph, SOURCE_ID)
    started_ms = int(time.time() * 1000)

    # The journey asserts the zero-copy store audit (the graph is the same
    # before and after the served query) and returns the served answer.
    connector = f"ck-mcp_tool-{uuid.uuid4().hex[:8]}"
    answer = _run_journey("mcp_tool", clean_graph, tmp_path, connector=connector)

    rows = {row["id"]: row for row in answer["rows"]}
    assert set(rows) == {item["id"] for item in RECORDING_ITEMS}, answer
    for row in rows.values():
        provenance = row["provenance"]
        assert provenance["connector"] == connector, row
        assert provenance["contract_iri"] == _contract_iri(connector), row
        assert provenance["mapping_iri"] == _contract_iri(connector), row
        fetched_ms = int(provenance["fetch_time_ms"])
        assert started_ms <= fetched_ms <= int(time.time() * 1000), row
        # The host's own provenance entries ride along, never invented.
        carried = json.loads(provenance["provenance"])
        assert _contract_iri(connector) in carried, row

    # The read counted once against the source's catalog asset.
    assert _usage_reads(clean_graph, SOURCE_ID) == before_usage + 1


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R722.6")
def test_uql_joins_native_fact_to_live_connector_rows_with_zero_copy(
    clean_graph, tmp_path
) -> None:
    """One native ``Note`` joins the connector rows; only its own row survives."""
    connector = f"ck-mcp_tool-{uuid.uuid4().hex[:8]}"
    native_id = RECORDING_ITEMS[0]["id"]
    text = f"LET native = MATCH (:Note); LET live = FOREIGN SCAN '{SOURCE_ID}'; JOIN native, live"
    # The journey asserts the stored nodes are exactly the native one before and after.
    answer = _run_journey(
        "mcp_tool",
        clean_graph,
        tmp_path,
        connector=connector,
        native_id=native_id,
        text=text,
    )
    rows = answer["rows"]
    assert [row["id"] for row in rows] == [native_id], answer
    provenance = rows[0]["provenance"]
    assert provenance["connector"] == connector, rows[0]
    assert provenance["contract_iri"] == _contract_iri(connector), rows[0]
    assert _stored_nodes_of(clean_graph) == [native_id]


def _stored_nodes_of(client: Any) -> list[str]:
    return sorted(client.nodes.ids())
