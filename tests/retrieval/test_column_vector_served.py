"""SQL column vectorization through the semantic index, served.

Spec row: MEDULLA-FEDERATED-QUERY-R077.2.

A text column binds as a ``SemanticBinding`` whose selector is a ``SqlColumnRef``.
Bind is ``SemanticIndex`` ``admit_binding``; unbind is ``drop_binding``. Admission
reads the table schema through the caller's authorized table.
"""

from __future__ import annotations

import asyncio
import os
import uuid

import pytest
from conftest import request_context

from medulla.client import EngineResponseError, MedullaClient


def _draft(table: str, column: str) -> dict:
    return {
        "binding_id": f"binding:{table}.{column}",
        "tenant_id": "placeholder",
        "actor_scope": "placeholder",
        "effective_actor_scope": "placeholder",
        "purpose_id": "retrieval",
        "policy": {
            "rbac_policy_revision": 1,
            "rbac_policy_digest": "sha256:rbac",
            "row_policy_revision": 1,
            "row_policy_digest": "sha256:row-policy",
            "source_acl_revision": 0,
            "source_acl_digest": "sha256:acl",
        },
        "source_selector": {
            "kind": "sql_column_ref",
            "selector": {
                "catalog_id": "tenant",
                "schema_id": "public",
                "table_id": table,
                "column_id": column,
            },
        },
        "source_schema_digest": "sha256:schema",
        "source_revision": "sha256:revision",
        "source_field_set_digest": f"sha256:{table}-{column}",
        "dimension": 3,
        "metric": "cosine",
        "model": {
            "model_id": "embedding-model",
            "model_revision": "revision-1",
            "preprocess_digest": "sha256:preprocess",
            "model_digest": "sha256:model",
        },
        "generation": 1,
        "maintenance_policy_id": "semantic-maintenance",
        "lexical_index": {
            "analyzer_id": "standard",
            "analyzer_revision": "1",
            "analyzer_config_digest": "sha256:analyzer",
        },
        "ann_index": {"method": "ivf_pq", "parameters_digest": "sha256:ann"},
        "created_at": "2026-11-20T00:00:00Z",
    }


async def _admit(client: MedullaClient, table: str, column: str) -> object:
    draft = _draft(table, column)
    return await client._send(
        "SemanticIndex",
        {
            "op": "admit_binding",
            "binding_id": draft["binding_id"],
            "tenant_id": draft["tenant_id"],
            "draft": draft,
            "idempotency_key": uuid.uuid4().hex,
        },
    )


# proves R077.2: Column vectorization: bind and unbind through the semantic index
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R077.2")
def test_bind_text_refuse_integer(clean_graph):
    """MEDULLA-FEDERATED-QUERY-R077.2"""
    table = f"vec_{uuid.uuid4().hex[:10]}"
    clean_graph.query.sql(
        f"CREATE TABLE {table} (id BIGINT PRIMARY KEY, body TEXT, qty BIGINT)"
    )

    async def run() -> None:
        client = await MedullaClient.connect(
            socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
            verified_context=request_context(),
        )
        try:
            with pytest.raises(EngineResponseError) as integer:
                await _admit(client, table, "qty")
            assert "COLUMN_TYPE_UNSUPPORTED" in str(integer.value)
            with pytest.raises(EngineResponseError) as absent:
                await _admit(client, table, "missing")
            assert "COLUMN_UNKNOWN" in str(absent.value)
        finally:
            await client.close()

    asyncio.run(run())
