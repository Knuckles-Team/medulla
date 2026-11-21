"""Served multimodal embedding spaces (MEDULLA-FEDERATED-QUERY-R039.1.4, R039.1.5, R039.1.7).

A graph's multimodal space is declared through the served ``DeclareEmbeddingSpace`` write,
never an in-process fixture. The session server binds the ``hash`` text embedder
(``builtin:hash``, eight dimensions, revision ``1``), so a space pinned to that tuple binds a
registered encoder.
"""

from __future__ import annotations

import contextlib
import hashlib
import os
import secrets
import uuid
from pathlib import Path
from typing import Any

import msgpack
import pytest
from _round_served import client, hashed, put
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

from medulla.client import EngineResponseError, SyncMedullaClient
from medulla.generated.models import RankedRows, ShortfallReason

ENCODER = "builtin:hash"
DIMENSIONS = 8
REVISION = "1"
_MODALITIES = ("text", "image", "audio", "video")
_TEXT_TO_TEXT = {"query": "text", "target": "text"}


def space(
    revision: str = REVISION, encoder: str = ENCODER, digest: str = "space-digest-a"
) -> dict[str, Any]:
    """A four-modality space pinned to the bound text embedder's identity tuple."""
    return {
        "id": ENCODER,
        "revision": revision,
        "model_hash": "sha256:w",
        "preprocessing_hash": "sha256:p",
        "dimensions": DIMENSIONS,
        "normalize": False,
        "digest": digest,
        "modalities": [{"modality": m, "encoder": encoder} for m in _MODALITIES],
    }


def declare(cl: Any, graph: str, declared: dict[str, Any]) -> str:
    return cl.graph.declare_embedding_space(declared, graph)


def unified(
    cl: Any, graph: str, ops: list[dict[str, Any]], pair: dict[str, str]
) -> RankedRows:
    """The served answer, validated against the generated ``RankedRows`` model."""
    result = cl._send(
        "UnifiedQuery",
        {"plan": {"ops": ops, "report_shortfall": True}, "retrieval": {"modality": pair}},
        graph,
    )
    if isinstance(result, (bytes, bytearray)):
        result = msgpack.unpackb(result, raw=False)
    return RankedRows.model_validate(result)


def _ranked_ops(text: str, k: int) -> list[dict[str, Any]]:
    return [
        {"Scan": {"label": "Doc"}},
        {"RankEmbed": {"text": text}},
        {"Limit": {"k": k}},
    ]


def _digest(text: str) -> str:
    return hashlib.sha256(text.encode()).hexdigest()


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R039.1.4")
def test_missing_embedding_shortfall_and_refusals(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R039.1.4: rows with no embedding are counted as
    missing, an encoder-less binding refuses QUERY_ENCODER_UNAVAILABLE, and a binding
    that names a registered encoder pinned to another identity refuses
    EMBEDDING_SPACE_INCOMPATIBLE, each before anything is ranked."""
    graph = f"multimodal-refusal-{uuid.uuid4().hex[:8]}"
    cl = client()
    cl.tenants.create(graph)
    try:
        # Two rows hold a vector, two do not.
        for index in range(2):
            put(cl, graph, f"with-{index}", {"type": "Doc"}, hashed(f"with-{index}"))
            put(cl, graph, f"without-{index}", {"type": "Doc"})
        assert declare(cl, graph, space()) == "space-digest-a"

        answer = unified(cl, graph, _ranked_ops("entry", 4), _TEXT_TO_TEXT)
        shortfall = answer.shortfall
        assert shortfall is not None
        assert shortfall.requested == 4
        assert shortfall.found == 2
        assert shortfall.missing == 2
        assert shortfall.reason == ShortfallReason.MISSING_EMBEDDING

        # A graph whose text binding names no registered encoder refuses.
        other = f"multimodal-unbound-{uuid.uuid4().hex[:8]}"
        cl.tenants.create(other)
        try:
            put(cl, other, "d", {"type": "Doc"}, hashed("d"))
            declare(cl, other, space(encoder="builtin:unregistered"))
            with pytest.raises(EngineResponseError) as unavailable:
                unified(cl, other, _ranked_ops("entry", 4), _TEXT_TO_TEXT)
            assert "QUERY_ENCODER_UNAVAILABLE" in str(unavailable.value)
        finally:
            cl.tenants.delete(other)

        # A graph whose space is pinned to another revision than the registered encoder.
        mismatched = f"multimodal-mismatch-{uuid.uuid4().hex[:8]}"
        cl.tenants.create(mismatched)
        try:
            put(cl, mismatched, "d", {"type": "Doc"}, hashed("d"))
            declare(cl, mismatched, space(revision="2", digest="space-digest-b"))
            with pytest.raises(EngineResponseError) as incompatible:
                unified(cl, mismatched, _ranked_ops("entry", 4), _TEXT_TO_TEXT)
            assert "EMBEDDING_SPACE_INCOMPATIBLE" in str(incompatible.value)
        finally:
            cl.tenants.delete(mismatched)
    finally:
        cl.tenants.delete(graph)
        cl.close()


@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R039.1.5")
def test_receipt_matches_model_calls(start_medulla_server) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R039.1.5: the receipt's modality evidence names
    the direction, the space, the generation and exactly the encoder calls made."""
    graph = f"multimodal-receipt-{uuid.uuid4().hex[:8]}"
    cl = client()
    cl.tenants.create(graph)
    try:
        put(cl, graph, "a", {"type": "Doc"}, hashed("a"))
        put(cl, graph, "b", {"type": "Doc"}, hashed("b"))
        declare(cl, graph, space())
        answer = unified(cl, graph, _ranked_ops("entry", 2), _TEXT_TO_TEXT)
        assert answer.receipt is not None and answer.receipt.modality is not None
        modality = answer.receipt.modality
        assert modality.modality_direction == "text->text"
        assert modality.space_id == ENCODER
        assert modality.model_revisions == [REVISION]
        assert modality.authorized_shortfall_counts is None
        assert [call.model_dump() for call in modality.encoding_calls] == [
            {"model": ENCODER, "modality": "text", "input_digest": _digest("entry")}
        ]
        assert isinstance(modality.index_generation, int)
    finally:
        cl.tenants.delete(graph)
        cl.close()


@contextlib.contextmanager
def _engine(tmp_path: Path, run: str, *, bootstrap: bool):
    """One engine process on a durable data directory shared across runs."""
    binary = find_server_binary()
    assert binary, "supply a prebuilt full MEDULLA_TEST_BINARY; no cold build is attempted"
    secret = secrets.token_hex(32)
    socket = str(tmp_path / "engine.sock")
    Path(socket).unlink(missing_ok=True)
    persist = tmp_path / "persist"
    persist.mkdir(exist_ok=True)
    env = strict_server_env(
        str(tmp_path / "security"), auth_secret=secret, persist_dir=str(persist)
    )
    env.update(
        PATH=os.environ.get("PATH", ""),
        HOME=str(tmp_path),
        MEDULLA_ENCRYPTION_KEY="declared-space-test-encryption-key",
        MEDULLA_UQL_TEXT_EMBEDDER="hash",
    )
    log = tmp_path / f"{run}.log"
    process = spawn_server(
        [str(Path(binary).resolve()), "--socket-path", socket], env=env, log_path=log
    )
    try:
        wait_for_server(process, socket, name="declared-space engine", log_path=log)
        if bootstrap:
            with SyncMedullaClient.connect(
                socket_path=socket,
                auth_secret=secret,
                verified_context=bootstrap_context(),
            ) as boot:
                boot.consensus.bootstrap_system_identity(
                    agent_id=TEST_AGENT_ID,
                    signer_id=TEST_AGENT_ID,
                    signer_key=TEST_SIGNER_KEY,
                )
        with SyncMedullaClient.connect(
            socket_path=socket,
            auth_secret=secret,
            verified_context=request_context(),
            timeout=server_timeout(),
        ) as cl:
            yield cl
    finally:
        stop_server(process, name="declared-space engine", log_path=log)


@pytest.mark.exact_artifact
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R039.1.7")
def test_declared_space_is_durable_and_refuses_a_different_one(tmp_path) -> None:
    """Verifies MEDULLA-FEDERATED-QUERY-R039.1.7: the declaration is idempotent, a
    different space refuses, and the declaration survives a restart on the same data
    directory, so the tuple still binds its encoder instead of refusing."""
    graph = f"declared-{uuid.uuid4().hex[:8]}"
    with _engine(tmp_path, "first", bootstrap=True) as cl:
        cl.tenants.create(graph)
        put(cl, graph, "doc-0", {"type": "Doc"}, hashed("doc-0"))
        assert declare(cl, graph, space()) == "space-digest-a"
        # The identical declaration again is a no-op success.
        assert declare(cl, graph, space()) == "space-digest-a"
        # Another revision is a different space: refused, nothing replaced.
        with pytest.raises(EngineResponseError) as different:
            declare(cl, graph, space(revision="2", digest="space-digest-b"))
        assert "already declares embedding space" in str(different.value)
        unified(cl, graph, _ranked_ops("entry", 1), _TEXT_TO_TEXT)
    with _engine(tmp_path, "restart", bootstrap=False) as cl:
        with pytest.raises(EngineResponseError) as still_refused:
            declare(cl, graph, space(revision="2", digest="space-digest-b"))
        assert "already declares embedding space" in str(still_refused.value)
        assert declare(cl, graph, space()) == "space-digest-a"
        served = unified(cl, graph, _ranked_ops("entry", 1), _TEXT_TO_TEXT)
        assert served.receipt is not None and served.receipt.modality is not None
        assert served.receipt.modality.space_id == ENCODER
