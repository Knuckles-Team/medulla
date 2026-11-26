"""Store audit of the competency journeys (MEDULLA-CQ-CONN-R067.9).

A served read of a non-materialised source stores no row of it, so every
journey compares the node ids the engine holds before and after the read.
"""

from __future__ import annotations

from medulla.client import SyncMedullaClient


def stored_nodes(client: SyncMedullaClient) -> list[str]:
    """The node ids the test's own graph stores, sorted.

    Served SPARQL also reads every visible pack graph of the tenant
    (MEDULLA-MEDIA-VOCAB-R655), so a default-graph pattern sees packs other
    tests imported. A "stores no source row" check names only this graph's
    nodes, and counts nodes, not triples.
    """
    return sorted(client.nodes.ids())
