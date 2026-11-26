from __future__ import annotations

import asyncio

import pytest

from medulla.generated.connector_pack import (
    ConnectorPackStatus,
    ConnectorPackStatusRequest,
)
from medulla.generated.storage import send_connector_pack_status

pytestmark = pytest.mark.no_engine


class _Client:
    def __init__(self) -> None:
        self.sent: tuple[str, object, object, object] | None = None

    async def _send(
        self,
        method: str,
        params: object,
        graph: object,
        *,
        idempotency_key: object,
    ) -> object:
        self.sent = method, params, graph, idempotency_key
        digest = "11" * 32
        return {
            "schema_version": 2,
            "tenant_id": "tenant-a",
            "connector": "connector-a",
            "head": {
                "binding_revision": 4,
                "pack_digest": "22" * 32,
                "catalog": {
                    "configuration_revision": 8,
                    "catalog_generation": 12,
                    "snapshot_digest": digest,
                    "child_connection_generation": 3,
                    "authorization_scope_digest": "33" * 32,
                },
                "server_package_version": "2.0.0",
                "record_id": "pack:4",
                "committed_at_ms": 42,
            },
            "members": {"published": 2, "withdrawn": 0, "retired": 0},
            "warnings": [],
            "projection": {"projection": "none"},
        }


def test_status_query_returns_exact_durable_catalog_binding() -> None:
    client = _Client()
    request = ConnectorPackStatusRequest(tenant_id="tenant-a", connector="connector-a")
    result = asyncio.run(send_connector_pack_status(client, request, "catalog"))

    assert isinstance(result, ConnectorPackStatus)
    assert result.head is not None
    assert result.head.catalog.catalog_generation == 12
    assert result.head.catalog.snapshot_digest == "11" * 32
    assert client.sent == (
        "ConnectorPack",
        {
            "op": {
                "op": "status",
                "request": {
                    "tenant_id": "tenant-a",
                    "connector": "connector-a",
                },
            }
        },
        "catalog",
        None,
    )


class _ReceiptClient:
    def __init__(self, payload: object) -> None:
        self.payload = payload
        self.sent: object = None

    async def _send(
        self,
        method: str,
        params: object,
        graph: object,
        *,
        idempotency_key: object,
    ) -> object:
        self.sent = method, params
        return self.payload


# spec: MEDULLA-TYPED-PACKS-R814.1
def test_catalog_reconciliation_receipt_round_trips_and_none_means_no_receipt() -> None:
    from medulla.generated.connector_pack import (
        McpCatalogReconciliationReceipt,
        McpCatalogReconciliationReceiptRequest,
    )
    from medulla.generated.storage import (
        send_connector_pack_catalog_reconciliation_receipt,
    )

    wire = {
        "tenant_id": "tenant-a",
        "catalog_generation": 5,
        "snapshot_digest": "44" * 32,
        "acknowledged_families": [
            "tools",
            "prompts",
            "resources",
            "resource_templates",
        ],
        "issued_at_ms": 1234,
    }
    client = _ReceiptClient(wire)
    request = McpCatalogReconciliationReceiptRequest(tenant_id="tenant-a")
    receipt = asyncio.run(
        send_connector_pack_catalog_reconciliation_receipt(client, request)
    )
    assert isinstance(receipt, McpCatalogReconciliationReceipt)
    assert receipt.issued_at_ms == 1234
    assert receipt.model_dump(mode="json") == wire
    assert client.sent == (
        "ConnectorPack",
        {
            "op": {
                "op": "catalog_reconciliation_receipt",
                "request": {"tenant_id": "tenant-a"},
            }
        },
    )
    none = asyncio.run(
        send_connector_pack_catalog_reconciliation_receipt(
            _ReceiptClient(None), request
        )
    )
    assert none is None
