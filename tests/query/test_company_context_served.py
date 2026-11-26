"""The governed company-context view over the served UQL wire
(MEDULLA-FEDERATED-QUERY-R078.7).

`COMPANY CONTEXT [CATEGORY name]` returns source-bound references to organization,
role, data
asset and permission nodes and to the latest committed point of a data asset's
series, from
the caller's own tenant snapshot. A node whose `clearance` labels are not all among the
caller's scopes is dropped before any time-series read, and no row carries a value.
"""

from __future__ import annotations

import os
import uuid
from typing import Any

import pytest
from conftest import (
    TEST_TENANT,
    request_context,
    tenant_request_context,
)

from medulla.client import SyncMedullaClient

OTHER_TENANT = "tenant:company-other"
EXTERNAL_VERSION = "https://knuckles-team.github.io/ontology/catalog#externalVersion"
SECOND_NS = 1_000_000_000
FUTURE_NS = 4_000_000_000 * SECOND_NS  # the year 2096, after any snapshot taken now


def _connect(tenant: str, scopes: list[str]) -> SyncMedullaClient:
    context = (
        request_context(scopes=scopes)
        if tenant == TEST_TENANT
        else tenant_request_context(tenant, scopes=scopes)
    )
    return SyncMedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"], verified_context=context
    )


def _seed(client: SyncMedullaClient, tag: str) -> None:
    client.nodes.add(f"org-{tag}", {"type": "Organization", "name": "Acme"})
    client.nodes.add(f"role-{tag}", {"type": "Role", "name": "Steward"})
    client.nodes.add(f"perm-{tag}", {"type": "SecurityClearance", "name": "internal"})
    client.nodes.add(
        f"asset-{tag}",
        {
            "type": "DataAsset",
            "name": "orders",
            "source": "warehouse",
            EXTERNAL_VERSION: 7,
            "series_id": f"series-{tag}",
        },
    )
    client.nodes.add(
        f"secret-{tag}",
        {"type": "DataAsset", "name": "secret", "clearance": ["restricted"]},
    )
    client.nodes.add(
        f"finance-{tag}",
        {
            "type": "DataAsset",
            "name": "ledger",
            "clearance": ["finance"],
            "series_id": f"series-fin-{tag}",
        },
    )
    for series in (f"series-{tag}", f"series-fin-{tag}"):
        client.timeseries.append(series, [(SECOND_NS, [1.0]), (FUTURE_NS, [9.0])])


def _rows(client: SyncMedullaClient, tag: str, text: str) -> dict[str, dict[str, Any]]:
    result = client.query.uql(text)
    return {row["id"]: row for row in result["rows"] if tag in row["id"]}


# proves R078.7: Governed company-context logical view
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R078.7")
def test_two_tenants_two_clearances(clean_graph) -> None:
    """Each tenant reads its own references; a label the caller lacks drops the node
    and its series."""
    tag = uuid.uuid4().hex[:8]
    other_tag = uuid.uuid4().hex[:8]
    seed_a = clean_graph
    seed_b = _connect(OTHER_TENANT, ["*"])
    reader_a = _connect(TEST_TENANT, ["kg:read", "internal"])
    reader_b = _connect(OTHER_TENANT, ["kg:read", "internal", "finance"])
    try:
        _seed(seed_a, tag)
        _seed(seed_b, other_tag)

        a_rows = _rows(reader_a, tag, "COMPANY CONTEXT")
        assert set(a_rows) == {
            f"org-{tag}",
            f"role-{tag}",
            f"perm-{tag}",
            f"asset-{tag}",
            f"ts:series-{tag}:1000",
        }, sorted(a_rows)
        asset = a_rows[f"asset-{tag}"]["provenance"]
        assert asset["category"] == "data_asset"
        assert asset["entity_id"] == f"asset-{tag}"
        assert asset["source_id"] == "warehouse"
        assert asset["source_revision"] == "7"
        assert asset["clearance"] == ""
        org = a_rows[f"org-{tag}"]["provenance"]
        assert org["category"] == "organization"
        assert org["source_id"] == "native"
        assert org["source_revision"].isdigit()
        point = a_rows[f"ts:series-{tag}:1000"]
        assert point["provenance"]["category"] == "time_series"
        assert point["provenance"]["entity_id"] == f"asset-{tag}"
        assert point["provenance"]["series_id"] == f"series-{tag}"
        assert point["provenance"]["timestamp_ms"] == "1000"
        # No value is copied: the row is a reference, never the point's number.
        assert point["score"] is None
        # The over-cleared node and its series are absent, and so is the future point.
        assert f"secret-{tag}" not in a_rows
        assert f"finance-{tag}" not in a_rows
        assert not any(row_id.startswith(f"ts:series-fin-{tag}") for row_id in a_rows)

        # The other tenant's clearance includes `finance`; it reads only its own nodes.
        b_rows = _rows(reader_b, other_tag, "COMPANY CONTEXT")
        assert f"finance-{other_tag}" in b_rows
        assert b_rows[f"finance-{other_tag}"]["provenance"]["clearance"] == "finance"
        assert f"ts:series-fin-{other_tag}:1000" in b_rows
        assert f"secret-{other_tag}" not in b_rows
        assert not _rows(reader_b, tag, "COMPANY CONTEXT")
        assert not _rows(reader_a, other_tag, "COMPANY CONTEXT")

        # Each category names only its own rows.
        for category, expected in (
            ("organization", {f"org-{tag}"}),
            ("role", {f"role-{tag}"}),
            ("data_asset", {f"asset-{tag}"}),
            ("permission", {f"perm-{tag}"}),
            ("time_series", {f"ts:series-{tag}:1000"}),
        ):
            rows = _rows(reader_a, tag, f"COMPANY CONTEXT CATEGORY {category}")
            assert set(rows) == expected, category
            assert {row["provenance"]["category"] for row in rows.values()} == {
                category
            }

        # An unknown category refuses at parse time with its code.
        with pytest.raises(Exception, match="UQL_E_COMPANY_CATEGORY"):
            reader_a.query.uql("COMPANY CONTEXT CATEGORY galaxy")
    finally:
        for client in (seed_b, reader_a, reader_b):
            client.close()
