"""Helpers of the composed execution-answer proof (MEDULLA-CQ-ORCH-R008.5).

The inputs are the files of ``fixtures/composed/``. ``ConnectorPack.Import``
(R008.4) lands the seed in the connector's pack graph; the answer reads the
request graph, so the test loads the same seed there explicitly.
"""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
from typing import Any

from _decision_resident_fixture import publication

from medulla.generated.index_repository import IndexRepositoryScope

COMPOSED = Path(__file__).parent / "fixtures" / "composed"
SEED_PARTS = ("identity", "catalog", "references")
REPO_ID = "repo-composed"
REVISION = "b" * 40


def load_seed_into_request_graph(client: Any) -> dict[str, int]:
    """Load the three seed parts into the connection's graph, the request graph
    the answer reads; returns the summed ``LoadReport``."""
    total = {"triples": 0, "multivalue": 0, "dropped_multivalue": 0}
    for part in SEED_PARTS:
        text = (COMPOSED / f"seed_{part}.ttl").read_text(encoding="utf-8")
        report = client.rdf.add_triples(turtle=text)
        for key in total:
            total[key] += int(report.get(key, 0))
    return total


def _publish_tool(
    client: Any, component_id: str, summary: str, capabilities: list[str]
) -> dict[str, str]:
    payload = publication(component_id, "tool", {}, summary=summary)
    payload["op"]["request"]["component"]["declared_capabilities"] = capabilities
    entry = client._send("AgentComponent", payload)["component"]
    return {
        "component_id": component_id,
        "kind": "tool",
        "definition_digest": entry["definition_digest"],
    }


def publish_composed_library(client: Any, suffix: str = "") -> list[dict[str, str]]:
    """One ``tool`` component per entry of ``templates.json``, declaring the
    capability the entry provides. ``suffix`` keeps ids unique across runs."""
    templates = json.loads((COMPOSED / "templates.json").read_text(encoding="utf-8"))
    return [
        _publish_tool(
            client, f"{entry['agent_id']}{suffix}", entry["role"], [entry["provides"]]
        )
        for entry in templates["templates"]
    ]


def publish_tool_components(
    client: Any, count: int, prefix: str = "tool"
) -> list[dict[str, str]]:
    """``count`` ``tool`` components named ``<prefix>-<n>`` with no capability."""
    return [
        _publish_tool(client, f"{prefix}-{n}", f"{prefix}-{n}", [])
        for n in range(count)
    ]


def index_composed_repo(client: Any, graph: str) -> Any:
    """Index the three files of ``fixtures/composed/repo/`` into ``graph``."""
    root = COMPOSED / "repo"
    files = [
        (path.relative_to(root).as_posix(), path.read_bytes())
        for path in sorted(root.rglob("*"))
        if path.is_file()
    ]
    scope = IndexRepositoryScope.model_validate(
        {
            "repository_id": REPO_ID,
            "commits": [],
            "refs": [
                {
                    "ref_name": "refs/heads/main",
                    "revision_id": REVISION,
                    "status": "live",
                }
            ],
            "file_versions": [
                {
                    "ref_name": "refs/heads/main",
                    "path": path,
                    "blob_digest": "sha256:" + hashlib.sha256(body).hexdigest(),
                }
                for path, body in files
            ],
        }
    )
    try:
        client.tenants.create(graph, "Agent")
    except Exception:  # the graph may already exist
        pass
    return client.graph.index_repository(files, scope=scope, graph=graph)
