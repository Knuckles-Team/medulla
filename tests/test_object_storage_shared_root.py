"""MEDULLA-PLACEMENT-R611.7: a node opens a tenant graph from the shared root
only with a placement route.

MEDULLA-OBJECT-STORAGE-BACKEND-R004.1: two engines share one root; a second
engine with an empty local directory reads what the first wrote, and rejoins
after the first writes more."""

from __future__ import annotations

import shutil
import types
from pathlib import Path

import pytest
from _loopback_object_store import loopback_object_store, spawn_object_store_engine

from medulla.placement import (
    PLACEMENT_NOT_GRANTED,
    PlacementDirectory,
    PlacementNotGrantedError,
    TenantPool,
)

pytestmark = pytest.mark.no_engine


def _engine(route: dict) -> types.SimpleNamespace:
    async def answer(tenant: str, partition: str, client_epoch: int = 0) -> dict:
        return route

    return types.SimpleNamespace(placement=types.SimpleNamespace(route=answer))


@pytest.mark.spec("MEDULLA-PLACEMENT-R611.7")
@pytest.mark.asyncio
async def test_open_without_route_refuses() -> None:
    fetched: list[str] = []

    async def factory(tenant: str) -> str:
        fetched.append(tenant)  # the segment fetch of the shared root
        return tenant

    pool: TenantPool[str] = TenantPool(factory, capacity=4)
    unplaced = PlacementDirectory(
        _engine({"epoch": 2, "stale": False, "placed": False, "endpoints": []})  # type: ignore[arg-type]
    )
    with pytest.raises(PlacementNotGrantedError) as refused:
        await pool.open_routed(unplaced, "tenant-a", "p0")
    assert refused.value.code == PLACEMENT_NOT_GRANTED
    assert fetched == [], "nothing is fetched before the refusal"

    placed = PlacementDirectory(
        _engine(
            {
                "epoch": 2,
                "stale": False,
                "placed": True,
                "endpoints": ["tcp://n1:9100"],
            }
        )  # type: ignore[arg-type]
    )
    assert await pool.open_routed(placed, "tenant-a", "p0") == "tenant-a"
    assert fetched == ["tenant-a"]


ROOT = "eg-shared-root"


def _read_all(client, names: list[str]) -> dict[str, object]:
    return {name: client.nodes.properties(name) for name in names}


@pytest.mark.spec("MEDULLA-OBJECT-STORAGE-BACKEND-R004.1")
def test_multi_node_cold_open_and_rejoin(tmp_path: Path) -> None:
    first = [f"s{index:03d}" for index in range(50)]
    later = [f"s{index:03d}" for index in range(50, 60)]
    everything = first + later
    with loopback_object_store() as store:
        with spawn_object_store_engine(
            tmp_path / "a", tmp_path / "a" / "persist", store, ROOT
        ) as engine_a:
            for index, name in enumerate(first):
                engine_a.nodes.add(name, {"i": index})
            expected_first = _read_all(engine_a, first)
            assert all(value is not None for value in expected_first.values())
            with spawn_object_store_engine(
                tmp_path / "b", tmp_path / "b" / "persist", store, ROOT
            ) as engine_b:
                assert _read_all(engine_b, first) == expected_first
            shutil.rmtree(tmp_path / "b" / "persist")
            for index, name in enumerate(later, start=50):
                engine_a.nodes.add(name, {"i": index})
            expected = _read_all(engine_a, everything)
            assert all(value is not None for value in expected.values())
            with spawn_object_store_engine(
                tmp_path / "b", tmp_path / "b" / "persist", store, ROOT
            ) as engine_b:
                assert _read_all(engine_b, everything) == expected
