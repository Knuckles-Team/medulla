"""Write a representative store with the last production release.

Spec: MEDULLA-DURABLE-KERNEL-R070.4. `test_store_upgrade_from_production.py`
runs this file under the interpreter that holds the production client, so it
imports nothing from this checkout. It starts the production engine on an
empty data directory and writes, through authenticated requests:

* one tenant graph per name in `GRAPHS`, which spreads the writes over every
  graph shard file;
* in each graph, committed transactions that add nodes and an edge, each with
  its own idempotency key (every commit persists a signed authority record);
* one content-addressed blob.

It stops the engine cleanly and prints a JSON manifest of what it wrote.

Usage: python _production_store_writer.py <pin.json> <server> <root> <settings.json>
"""

from __future__ import annotations

import importlib
import json
import os
import subprocess
import sys
import time

GRAPHS = [f"upgrade-g{index:02d}" for index in range(32)]
COMMITS_PER_GRAPH = 2
STARTUP_TIMEOUT_S = 120
BLOB = b"medulla upgrade proof blob\n" * 64


def _environment(pin: dict, settings: dict, persist: str, state: str) -> dict:
    prefix = pin["env_prefix"]
    inherited = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(("GRAPH_SERVICE_", "MEDULLA_", "EPISTEMIC_GRAPH_"))
    }
    signers = {
        settings["agent"]: {
            "key": settings["signer_key"],
            "allowed_roles": ["test"],
            "may_grant_system": True,
        }
    }
    return {
        **inherited,
        "GRAPH_SERVICE_AUTH_SECRET": settings["auth_secret"],
        "GRAPH_SERVICE_PERSIST_DIR": persist,
        f"{prefix}REQUIRE_OIDC": "false",
        f"{prefix}AUDIENCE": settings["audience"],
        f"{prefix}TENANT": settings["tenant"],
        f"{prefix}POLICY_VERSION": settings["policy_version"],
        f"{prefix}SECURITY_STATE_DIR": state,
        f"{prefix}ENCRYPTION_KEY": settings["encryption_key"],
        f"{prefix}SIGNER_KEYS_JSON": json.dumps(signers),
    }


def _start(server: str, root: str, env: dict) -> tuple[subprocess.Popen, str]:
    socket = os.path.join(root, "production.sock")
    log = open(os.path.join(root, "production-engine.log"), "w", encoding="utf-8")
    process = subprocess.Popen(
        [server, "--socket-path", socket], env=env, stdout=log, stderr=subprocess.STDOUT
    )
    deadline = time.monotonic() + STARTUP_TIMEOUT_S
    while not os.path.exists(socket):
        if process.poll() is not None or time.monotonic() > deadline:
            process.kill()
            sys.exit(f"the production engine did not start; see {log.name}")
        time.sleep(0.1)
    return process, socket


def _write_graph(client, graph: str) -> dict:
    nodes, edges = {}, []
    for commit in range(COMMITS_PER_GRAPH):
        source, target = f"{graph}-s{commit}", f"{graph}-t{commit}"
        txn = client.txn.begin(graph=graph)
        for node in (source, target):
            nodes[node] = {"kind": "UpgradeProof", "commit": commit}
            client.txn.add_node(txn, node, nodes[node], graph=graph)
        client.txn.add_edge(txn, source, target, {"commit": commit}, graph=graph)
        if not client.txn.commit(txn, idempotency_key=f"{graph}-commit-{commit}"):
            sys.exit(f"the production engine refused commit {commit} on {graph}")
        edges.append([source, target])
    return {"nodes": nodes, "edges": edges}


def main() -> None:
    pin_path, server, root, settings_path = sys.argv[1:5]
    with open(pin_path, encoding="utf-8") as handle:
        pin = json.load(handle)
    with open(settings_path, encoding="utf-8") as handle:
        settings = json.load(handle)
    persist, state = os.path.join(root, "persist"), os.path.join(root, "security")
    os.makedirs(persist, exist_ok=True)
    client_class = getattr(
        importlib.import_module(pin["client_module"]), pin["client_class"]
    )
    process, socket = _start(server, root, _environment(pin, settings, persist, state))
    try:
        bootstrap = client_class.connect(
            socket_path=socket,
            auth_secret=settings["auth_secret"],
            verified_context={
                **settings["context"],
                "roles": [],
                "scopes": ["security:bootstrap"],
            },
        )
        bootstrap.consensus.bootstrap_system_identity(
            agent_id=settings["agent"],
            signer_id=settings["agent"],
            signer_key=settings["signer_key"],
        )
        bootstrap.close()
        client = client_class.connect(
            socket_path=socket,
            auth_secret=settings["auth_secret"],
            verified_context=settings["context"],
        )
        for graph in GRAPHS:
            client.tenants.create(graph, "Agent")
        written = {graph: _write_graph(client, graph) for graph in GRAPHS}
        blob_digest = client.blob.store(BLOB)
        client.close()
    finally:
        process.terminate()
        process.wait(STARTUP_TIMEOUT_S)
    print(
        json.dumps({"version": pin["version"], "graphs": written, "blob": blob_digest})
    )


if __name__ == "__main__":
    main()
