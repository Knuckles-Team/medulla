"""MEDULLA-PLACEMENT-R611.6: hot-path budgets of the placement client.

Cached route lookup under 5 microseconds and pool acquire p99 under 100
microseconds. Nightly only; a regression above 10 percent fails the nightly
run, never a PR.
"""

from __future__ import annotations

import time
import types

import pytest

from medulla.placement import PlacementDirectory, TenantPool

pytestmark = [pytest.mark.no_engine, pytest.mark.nightly]

LOOKUP_BUDGET_S = 5e-6
ACQUIRE_P99_BUDGET_S = 100e-6
REGRESSION_TOLERANCE = 1.10
ROUNDS = 20_000


async def _route(tenant: str, partition: str, client_epoch: int = 0) -> dict:
    return {
        "tenant_ref": tenant,
        "partition_ref": partition,
        "group": 2,
        "epoch": 3,
        "stale": False,
        "placed": True,
        "endpoints": ["tcp://n2:9100"],
    }


@pytest.mark.spec("MEDULLA-PLACEMENT-R611.6")
@pytest.mark.asyncio
async def test_lookup_and_acquire_budget() -> None:
    engine = types.SimpleNamespace(placement=types.SimpleNamespace(route=_route))
    directory = PlacementDirectory(engine)  # type: ignore[arg-type]
    await directory.route("t", "p")

    started = time.perf_counter()
    for _ in range(ROUNDS):
        assert directory.cached_route("t", "p") is not None
    lookup = (time.perf_counter() - started) / ROUNDS
    assert lookup < LOOKUP_BUDGET_S * REGRESSION_TOLERANCE, lookup

    async def factory(tenant: str) -> object:
        return object()

    pool: TenantPool[object] = TenantPool(factory, capacity=8)
    await pool.acquire("t")
    samples = []
    for _ in range(ROUNDS):
        started = time.perf_counter()
        await pool.acquire("t")
        samples.append(time.perf_counter() - started)
    samples.sort()
    p99 = samples[int(len(samples) * 0.99) - 1]
    assert p99 < ACQUIRE_P99_BUDGET_S * REGRESSION_TOLERANCE, p99
