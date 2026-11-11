"""Served worker claim: forged routing fields are refused at decode.

spec: MEDULLA-PLACEMENT-R002.5

Routing is engine-side only. The claim request type (`JobOp::WorkerClaim`,
decoded for `handle_worker_claim`) carries no lane or partition field and
uses `deny_unknown_fields`, so a claim that sets either is refused with
`INVALID_ARGUMENT` at request decode — before any worker-grant or registry
lookup, so no registered worker is needed for the refusal.

The typed client enforces the same closed contract before sending (the
generated models are `extra="forbid"`; the native body codec decodes the
frame as the typed `Request`), so a forged field reaches the engine's own
decode boundary only through a hand-packed frame.
"""

import uuid

import msgpack
import pytest


def _claim_frame(req_id: int, claim: dict) -> bytes:
    """One raw `AnalyticsJob` request frame, packed without the client's
    canonical body codec, so the engine's decode boundary is what decides."""
    return msgpack.packb(
        {
            "id": req_id,
            "graph": "",
            "auth_token": "",
            "agent_id": None,
            "method": "AnalyticsJob",
            "params": {"op": {"WorkerClaim": claim}},
        },
        use_bin_type=True,
    )


@pytest.mark.spec("MEDULLA-PLACEMENT-R002.5")
def test_forged_lane_field_is_refused(clean_graph):
    nonce = uuid.uuid4().hex
    claim = {
        "worker_instance": f"slot-{nonce}",
        "capabilities": [],
        "lease_ms": 60_000,
    }
    base = int(nonce[:8], 16)

    # The un-forged claim passes decode: whatever the engine answers next
    # (an auth refusal on the empty token, not a decode refusal) it is not
    # the decode refusal this row installs.
    clean = clean_graph._roundtrip(
        _claim_frame(base, claim), req_id=base, method="AnalyticsJob"
    )
    assert "invalid request encoding" not in (clean.get("error") or "")

    # A forged lane or partition field is refused at decode, with no job
    # queued and no worker registered.
    for offset, forged_field in enumerate(("lane", "partition")):
        forged = {**claim, forged_field: "hot-spare"}
        response = clean_graph._roundtrip(
            _claim_frame(base + 1 + offset, forged),
            req_id=base + 1 + offset,
            method="AnalyticsJob",
        )
        error = response.get("error") or ""
        assert "INVALID_ARGUMENT" in error, (
            f"forged {forged_field!r} must be refused at decode; got: {error!r}"
        )
