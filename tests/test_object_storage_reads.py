"""MEDULLA-OBJECT-STORAGE-BACKEND-R002.1: a second engine with an empty local
directory opens the graph from the object-store root, and a corrupted segment
refuses the open by name."""

from __future__ import annotations

import shutil
from pathlib import Path

import pytest
from _loopback_object_store import loopback_object_store, spawn_object_store_engine

pytestmark = pytest.mark.no_engine

ROOT = "eg-root"
NODE_COUNT = 100


def _write_nodes(client, count: int) -> None:
    for index in range(count):
        client.nodes.add(f"n{index:03d}", {"i": index})


@pytest.mark.spec("MEDULLA-OBJECT-STORAGE-BACKEND-R002.1")
def test_cold_open_from_root(tmp_path: Path) -> None:
    with loopback_object_store() as store:
        with spawn_object_store_engine(
            tmp_path / "a", tmp_path / "a" / "persist", store, ROOT
        ) as engine_a:
            _write_nodes(engine_a, NODE_COUNT)
        shutil.rmtree(tmp_path / "a" / "persist")
        with spawn_object_store_engine(
            tmp_path / "b", tmp_path / "b" / "persist", store, ROOT
        ) as engine_b:
            for index in range(NODE_COUNT):
                properties = engine_b.nodes.properties(f"n{index:03d}")
                assert properties is not None, f"n{index:03d} missing after cold open"
                assert properties["i"] == index


@pytest.mark.spec("MEDULLA-OBJECT-STORAGE-BACKEND-R002.1")
def test_corrupt_segment_refuses_open(tmp_path: Path) -> None:
    with loopback_object_store() as store:
        with spawn_object_store_engine(
            tmp_path / "a", tmp_path / "a" / "persist", store, ROOT
        ) as engine_a:
            _write_nodes(engine_a, NODE_COUNT)
        segments = [key for key in store.keys(f"{ROOT}/") if "/segments/" in key]
        assert segments, "engine A published no segment to the root"
        store.corrupt(segments[0])
        with spawn_object_store_engine(
            tmp_path / "b",
            tmp_path / "b" / "persist",
            store,
            ROOT,
            expect_exit=True,
        ) as (returncode, log):
            assert returncode != 0
            assert "OBJECT_SEGMENT_DIGEST_MISMATCH" in log
