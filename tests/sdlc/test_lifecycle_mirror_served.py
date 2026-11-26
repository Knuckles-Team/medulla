"""MEDULLA-SDLC-R031: defect and lifecycle calls through the generated senders."""

from __future__ import annotations

import asyncio
import os
import uuid

import pytest
from conftest import TEST_AGENT_ID, request_context

from medulla.client import MedullaClient
from medulla.generated import ingestion
from medulla.generated.models import DefectFileOutcome, DefectStatus


async def _scenario(graph: str) -> None:
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=request_context(),
        timeout=60,
    )
    try:
        await client.tenants.create(graph, "Global")
        await client._send(
            "SetPathGrant",
            {
                "request": {
                    "repo": graph,
                    "principal": TEST_AGENT_ID,
                    "prefix": "",
                    "read": True,
                    "write": True,
                }
            },
        )

        def filing(source: str) -> dict:
            return {
                "request": {
                    "repo": graph,
                    "intake": "direct",
                    "title": "mirror probe",
                    "source_record_id": source,
                    "observed_at": 1000,
                    "text": "panic in the mirror probe",
                }
            }

        first = ingestion.decode_file_defect(
            await ingestion.send_file_defect(client, filing("record-1"))
        )
        assert first.outcome is DefectFileOutcome.CREATED
        assert first.status is DefectStatus.NEW
        again = ingestion.decode_file_defect(
            await ingestion.send_file_defect(client, filing("record-2"))
        )
        assert again.outcome is DefectFileOutcome.ABSORBED
        assert (again.defect_id, again.symptoms) == (first.defect_id, 2)

        got = ingestion.decode_get_defect(
            await ingestion.send_get_defect(
                client, {"request": {"repo": graph, "defect_id": first.defect_id}}
            )
        )
        assert got.node_id == first.node_id
        assert [symptom.source_record_id for symptom in got.defect.symptoms] == [
            "record-1",
            "record-2",
        ]

        moved = ingestion.decode_transition_defect(
            await ingestion.send_transition_defect(
                client,
                {
                    "request": {
                        "repo": graph,
                        "defect_id": first.defect_id,
                        "to": "triaged",
                    }
                },
            )
        )
        assert (moved.from_, moved.to) == (DefectStatus.NEW, DefectStatus.TRIAGED)
        with pytest.raises(Exception, match="DEFECT_TRANSITION_ILLEGAL"):
            await ingestion.send_transition_defect(
                client,
                {
                    "request": {
                        "repo": graph,
                        "defect_id": first.defect_id,
                        "to": "verified",
                    }
                },
            )

        trace = ingestion.decode_lifecycle_trace(
            await ingestion.send_lifecycle_trace(
                client,
                {"request": {"anchor": first.node_id, "depth": 1}},
                graph,
            )
        )
        assert isinstance(trace.truncated, bool)
        with pytest.raises(Exception, match="SPEC_TREE_ABSENT|SPEC_STATUS_REV_UNKNOWN"):
            await ingestion.send_spec_status(
                client, {"repo": graph, "rev": "absent", "graph": graph}
            )
        await client.tenants.delete(graph)
    finally:
        await client.close()


# spec: MEDULLA-SDLC-R031
@pytest.mark.spec("MEDULLA-SDLC-R031")
def test_defects_and_the_lifecycle_are_called_through_the_generated_senders():
    asyncio.run(_scenario(f"sdlc-mirror-{uuid.uuid4().hex[:8]}"))
