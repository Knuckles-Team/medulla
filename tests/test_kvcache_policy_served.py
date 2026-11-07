"""Served proof of the KV-cache tier policy (MEDULLA-DURABLE-KERNEL-R800.3, .5, .6).

Each test drives a real engine built with the answer cache and the KV-cache HTTP
surface, then reads the policy evidence the engine itself serves on
``GET /kv/stats``: the ``medulla_kvcache_policy_total{tier,event}`` counters and the
replayable decision trace. Nothing is mocked; Python only sends authenticated RPCs and
HTTP requests.

Run against a prebuilt full binary with ``MEDULLA_TEST_BINARY``; the module never
builds.
"""

from __future__ import annotations

import contextlib
import json
import secrets
import socket
import urllib.error
import urllib.request

import pytest
from conftest import (
    TEST_AGENT_ID,
    TEST_SIGNER_KEY,
    bootstrap_context,
    find_server_binary,
    request_context,
    server_timeout,
    spawn_server,
    stop_server,
    strict_server_env,
    wait_for_server,
)

from medulla.client import SyncMedullaClient

SECRET = secrets.token_hex(32)
KV_TOKEN = secrets.token_hex(16)
# The smallest legal answer budget (`kvcache.answer_max_bytes` floor: 1 MiB), so a
# handful of kilobyte-scale entries exercise eviction.
BUDGET_BYTES = 1_048_576


def _free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


@pytest.fixture(scope="module")
def policy_engine(tmp_path_factory):
    """One engine with the answer cache, the durable block tier and a 1 MiB budget."""
    binary = find_server_binary()
    assert binary, (
        "supply a prebuilt full MEDULLA_TEST_BINARY; no cold build is attempted"
    )
    runtime = tmp_path_factory.mktemp("kvcache-policy")
    socket_path = str(runtime / "engine.sock")
    persist = runtime / "persist"
    persist.mkdir()
    log_path = runtime / "engine.log"
    port = _free_port()
    env = strict_server_env(
        str(runtime / "security"), auth_secret=SECRET, persist_dir=str(persist)
    )
    env.update(
        MEDULLA_ENCRYPTION_KEY="kvcache-policy-test-encryption-key",
        MEDULLA_KVCACHE_ADDR=f"127.0.0.1:{port}",
        MEDULLA_KVCACHE_TOKEN=KV_TOKEN,
        MEDULLA_KVCACHE_BACKEND="durable",
        MEDULLA_KVCACHE_ANSWER_MAX_BYTES=str(BUDGET_BYTES),
        MEDULLA_KVCACHE_MIN_WORTH="0.001",
    )
    process = spawn_server(
        [binary, "--socket-path", socket_path], env=env, log_path=log_path
    )
    try:
        wait_for_server(
            process, socket_path, name="kvcache policy engine", log_path=log_path
        )
        with SyncMedullaClient.connect(
            socket_path=socket_path,
            auth_secret=SECRET,
            verified_context=bootstrap_context(),
        ) as bootstrap:
            bootstrap.consensus.bootstrap_system_identity(
                agent_id=TEST_AGENT_ID,
                signer_id=TEST_AGENT_ID,
                signer_key=TEST_SIGNER_KEY,
            )
        with SyncMedullaClient.connect(
            socket_path=socket_path,
            auth_secret=SECRET,
            verified_context=request_context(),
            timeout=server_timeout(),
        ) as client:
            yield client, f"http://127.0.0.1:{port}"
    finally:
        stop_server(process, name="kvcache policy engine", log_path=log_path)


def _kv(base: str, method: str, path: str, body: bytes | None = None, **headers):
    request = urllib.request.Request(
        base + path,
        data=body,
        method=method,
        headers={"Authorization": f"Bearer {KV_TOKEN}", **headers},
    )
    try:
        with urllib.request.urlopen(request, timeout=15) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as error:
        return error.code, error.read()


def _policy(base: str) -> dict:
    status, body = _kv(base, "GET", "/kv/stats")
    assert status == 200
    return json.loads(body)["policy"]


def _total(policy: dict, tier: str, event: str) -> int:
    return sum(
        row["value"]
        for row in policy["counters"]
        if row["tier"] == tier and row["event"] == event
    )


@contextlib.contextmanager
def _delta(base: str, tier: str):
    """Counter differences for `tier` across the block, by event name."""
    events = ("admitted", "refused", "evicted", "invalidated", "hit", "miss")
    before = {event: _total(_policy(base), tier, event) for event in events}
    seen: dict[str, int] = {}
    yield seen
    after = _policy(base)
    seen.update({event: _total(after, tier, event) - before[event] for event in events})


@pytest.mark.spec("MEDULLA-DURABLE-KERNEL-R800.3")
@pytest.mark.spec("MEDULLA-DURABLE-KERNEL-R800")
def test_answer_entry_invalidated_when_dependency_advances(policy_engine) -> None:
    client, base = policy_engine
    client.graph.clear()
    client.nodes.add("served-dep-1", {"type": "ServedDep", "title": "first"})
    query = "MATCH (:ServedDep)"

    with _delta(base, "answer") as first:
        client.query.uql(query)
    assert first["admitted"] == 1, (
        "the first execution is admitted into the answer tier"
    )

    with _delta(base, "answer") as repeat:
        again = client.query.uql(query)
    assert repeat["hit"] == 1, "an unchanged dependency serves the cached answer"
    assert repeat["admitted"] == 0
    assert [row["id"] for row in again["rows"]] == ["served-dep-1"]

    # A write advances the dependency: the next call recomputes and is admitted again.
    client.nodes.add("served-dep-2", {"type": "ServedDep", "title": "second"})
    with _delta(base, "answer") as advanced:
        recomputed = client.query.uql(query)
    assert advanced["hit"] == 0, "a stale answer is never served"
    assert advanced["admitted"] == 1
    assert sorted(row["id"] for row in recomputed["rows"]) == [
        "served-dep-1",
        "served-dep-2",
    ]


@pytest.mark.spec("MEDULLA-DURABLE-KERNEL-R800.5")
def test_stats_report_policy_counters_and_trace(policy_engine) -> None:
    _client, base = policy_engine

    def put(key: str, size: int, cost: int) -> int:
        status, _ = _kv(
            base,
            "PUT",
            f"/kv/{key}",
            b"k" * size,
            **{"X-MEDULLA-Rebuild-Cost-Ms": str(cost)},
        )
        return status

    keys = {name: secrets.token_hex(16) for name in "ABCD"}
    with _delta(base, "block") as seen:
        # Three 400 KB blocks over the 1 MiB budget: the lowest worth (A) leaves.
        assert put(keys["A"], 400_000, 4_000) == 201
        assert put(keys["B"], 400_000, 8_000) == 201
        assert put(keys["C"], 400_000, 40_000) == 201
        # A block claiming no rebuild cost is under the worth floor: accepted, not
        # stored.
        assert put(keys["D"], 100, 0) == 202
        assert _kv(base, "GET", f"/kv/{keys['C']}")[0] == 200
        assert _kv(base, "GET", f"/kv/{keys['A']}")[0] == 404

    assert seen["admitted"] == 3
    assert seen["evicted"] == 1
    assert seen["refused"] == 1
    assert seen["hit"] == 1
    assert seen["miss"] == 1

    policy = _policy(base)
    assert policy["counter"] == "medulla_kvcache_policy_total"
    assert 0.0 < policy["hit_ratio"] < 1.0
    assert policy["trace_capacity"] == 1024
    trace = policy["trace"]
    assert 0 < len(trace) <= 1024
    assert [entry["seq"] for entry in trace] == sorted(entry["seq"] for entry in trace)
    refused = [entry for entry in trace if entry["event"] == "refused"][-1]
    assert refused["reason"] == "below_min_worth"
    assert refused["worth"] == 0.0
    # Digests only: no key and no tenant name appear anywhere in the trace.
    text = json.dumps(trace)
    assert not any(key in text for key in keys.values())
    assert "shared" not in text


@pytest.mark.spec("MEDULLA-DURABLE-KERNEL-R800.6")
def test_policy_admits_hot_answer_and_refuses_cold_one(policy_engine) -> None:
    client, base = policy_engine
    client.graph.clear()
    client.nodes.add("served-hot-1", {"type": "ServedHot", "title": "small"})
    # An answer row carries the node id, not its properties, so the cold answer
    # is made large through long ids: about 120 KB for a few milliseconds of work.
    padding = "x" * 2_000
    for index in range(60):
        client.nodes.add(f"served-cold-{index}-{padding}", {"type": "ServedCold"})

    with _delta(base, "answer") as hot:
        client.query.uql("MATCH (:ServedHot)")
    assert hot["admitted"] == 1 and hot["refused"] == 0

    # The cold answer is large and cheap to rebuild: its worth stays under the floor.
    with _delta(base, "answer") as cold:
        result = client.query.uql("MATCH (:ServedCold)")
    assert len(result["rows"]) == 60
    assert cold["refused"] == 1 and cold["admitted"] == 0

    refusal = [
        entry
        for entry in _policy(base)["trace"]
        if entry["tier"] == "answer" and entry["event"] == "refused"
    ][-1]
    assert refusal["reason"] == "below_min_worth"

    # A refusal is a miss: the next call recomputes rather than serving a stored answer.
    with _delta(base, "answer") as again:
        client.query.uql("MATCH (:ServedCold)")
    assert again["hit"] == 0 and again["refused"] == 1
