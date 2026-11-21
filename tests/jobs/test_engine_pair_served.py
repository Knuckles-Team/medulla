"""Served proof for MEDULLA-PLACEMENT-R751.763: two engines over one durable store.

`spawn_engine_pair` starts two engine processes with different sockets and host
identities over one shared store directory. A node written through engine A is
read through engine B.
"""

import pytest

pytestmark = [
    pytest.mark.no_engine,  # The pair owns its two real isolated processes.
    pytest.mark.spec("MEDULLA-PLACEMENT-R751.763"),
]


def test_two_engines_share_one_store_and_each_sees_the_other_write(engine_pair):
    engine_pair.client_a.nodes.add("pair-node-a", {"written_by": "engine-a"})
    seen = engine_pair.client_b.nodes.properties("pair-node-a")
    assert seen is not None and seen["written_by"] == "engine-a"
    engine_pair.client_b.nodes.add("pair-node-b", {"written_by": "engine-b"})
    seen = engine_pair.client_a.nodes.properties("pair-node-b")
    assert seen is not None and seen["written_by"] == "engine-b"
