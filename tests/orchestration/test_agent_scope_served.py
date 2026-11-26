"""MEDULLA-ORCH-VOCAB-R808 served agent knowledge scope.

L4: an agent definition node stores its ``knowledgeScope`` as canonical JSON, and a
scope that names a knowledge set the writer's graph does not hold is refused
``SCOPE_SET_UNKNOWN`` at the write, with one code and one detail whether the set is
absent or another tenant's.
"""

from __future__ import annotations

import json
import sys
import uuid
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "retrieval"))

from _round_served import client, put


@pytest.mark.spec("MEDULLA-ORCH-VOCAB-R808")
def test_scope_stored_and_foreign_set_refused(start_medulla_server) -> None:
    # spec: MEDULLA-ORCH-VOCAB-R808
    graph = f"agent-scope-{uuid.uuid4().hex[:8]}"
    cl = client()
    cl.tenants.create(graph)
    try:
        put(cl, graph, "ks:mine", {"type": "KnowledgeSet"})
        scope = {"knowledge_sets": ["ks:mine"], "source_kinds": ["confluence"]}
        put(
            cl,
            graph,
            "agent:scoped",
            {"type": "AgentDefinition", "knowledgeScope": scope},
        )
        stored = cl.nodes.properties("agent:scoped")
        assert stored is not None
        raw = stored["knowledgeScope"]
        assert (json.loads(raw) if isinstance(raw, str) else raw) == scope

        # An absent set and another tenant's set answer the same.
        messages = []
        for foreign in ("ks:absent", "ks:theirs-in-another-graph"):
            with pytest.raises(Exception, match="SCOPE_SET_UNKNOWN") as refused:
                put(
                    cl,
                    graph,
                    f"agent:refused-{foreign}",
                    {
                        "type": "AgentDefinition",
                        "knowledgeScope": {"knowledge_sets": [foreign]},
                    },
                )
            messages.append(str(refused.value).replace(foreign, "<set>"))
        assert messages[0] == messages[1], messages
        # A refusal writes nothing.
        assert cl.nodes.properties("agent:refused-ks:absent") is None

        # A malformed scope is refused too.
        with pytest.raises(Exception, match="INVALID_ARGUMENT"):
            put(
                cl,
                graph,
                "agent:malformed",
                {"type": "AgentDefinition", "knowledgeScope": {"nope": 1}},
            )
    finally:
        cl.tenants.delete(graph)
        cl.close()
