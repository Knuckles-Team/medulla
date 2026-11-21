"""MEDULLA-OBJECT-STORAGE-BACKEND-R003.1: the ack follows the `HEAD` update.

The engine is killed between the segment upload and the `HEAD` update, then
restarted on the same local directory and the same root. Every acked write is
present and the unacked write is absent."""

from __future__ import annotations

import json
import threading
from pathlib import Path

import pytest
from _loopback_object_store import LoopbackObjectStore, spawn_object_store_engine

pytestmark = pytest.mark.no_engine

ROOT = "eg-commit-root"


@pytest.mark.spec("MEDULLA-OBJECT-STORAGE-BACKEND-R003.1")
def test_ack_follows_head_update(tmp_path: Path) -> None:
    acked = [f"a{index:02d}" for index in range(5)]
    persist = tmp_path / "persist"
    store = LoopbackObjectStore()
    try:
        with spawn_object_store_engine(tmp_path / "first", persist, store, ROOT) as engine:
            for index, name in enumerate(acked):
                engine.nodes.add(name, {"i": index})
            hold = store.hold_head_update()
            outcome: dict[str, object] = {}

            def add_unacked() -> None:
                try:
                    engine.nodes.add("unacked", {"i": 99})
                    outcome["acked"] = True
                except Exception as error:  # the engine is killed under it
                    outcome["error"] = error

            writer = threading.Thread(target=add_unacked, daemon=True)
            writer.start()
            assert hold.reached.wait(timeout=60), "the commit never reached the HEAD fence"
            # The segment and the next manifest are in the root; HEAD is not advanced.
            head = json.loads(store.get(hold.key))
            graph_dir = hold.key.removesuffix("HEAD")
            manifests = store.keys(f"{graph_dir}manifests/")
            assert len(manifests) == head["version"] + 2, "the unacked manifest is not published"
            store.kill_engine()
            hold.release()
            writer.join(timeout=60)
            assert "acked" not in outcome, "the commit was acked before the HEAD update"

        with spawn_object_store_engine(tmp_path / "second", persist, store, ROOT) as engine:
            for index, name in enumerate(acked):
                properties = engine.nodes.properties(name)
                assert properties is not None, f"{name} was acked and is missing after the kill"
                assert properties["i"] == index
            assert engine.nodes.properties("unacked") is None
    finally:
        store.stop()
