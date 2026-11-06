"""MEDULLA-PLACEMENT-R611.7: a node opens a tenant graph from the shared root
only with a placement route."""

from __future__ import annotations

import types

import pytest

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
