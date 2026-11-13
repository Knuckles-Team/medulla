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
    series = f"asset_usage.reads|principal={TEST_AGENT_ID}|asset={asset}"
    rows = client._send("TsRange", {"series_id": series, "from": 0, "to": 2**62}, None)
    if isinstance(rows, (bytes, bytearray)):
        rows = msgpack.unpackb(rows, raw=False)
    return sum(int(sum(values)) for _ts, values in rows)


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R722.5")
def test_rows_carry_contract_provenance_and_nothing_is_copied(
    clean_graph, tmp_path
) -> None:
    before_usage = _usage_reads(clean_graph, SOURCE_ID)
    started_ms = int(time.time() * 1000)

    # The journey asserts the zero-copy store audit (the graph is the same
    # before and after the served query) and returns the served answer.
    answer = _run_journey("mcp_tool", clean_graph, tmp_path)

    connector = "ck-mcp_tool"
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
