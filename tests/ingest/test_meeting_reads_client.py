"""MEDULLA-MEDIA-VOCAB-R001.101.7 wire mirror of the meeting reads.

L1: the generated models accept the engine's reply shapes and refuse anything else,
with no engine running.
"""

from __future__ import annotations

import pytest
from pydantic import ValidationError

from medulla.generated import ingestion
from medulla.generated import models as _models

DIGEST = "a" * 64


# spec: MEDULLA-MEDIA-VOCAB-R001.101.7
@pytest.mark.spec("MEDULLA-MEDIA-VOCAB-R001.101.7")
def test_the_read_models_mirror_the_wire_shape() -> None:
    answer = _models.MeetingTranscriptReadAnswer.model_validate(
        {
            "meeting_id": "m1",
            "events": [
                {
                    "sequence": 30,
                    "revision": 3,
                    "op": "finalize",
                    "text_digest": DIGEST,
                    "finalization_digest": DIGEST,
                }
            ],
            "last_sequence": 30,
            "transcript_revision": 3,
            "context_revision": 3,
            "finalization_digest": DIGEST,
        }
    )
    assert answer.events[0].op == _models.TranscriptRevisionOp.FINALIZE
    assert answer.next_seq is None
    with pytest.raises(ValidationError):
        _models.MeetingTranscriptReadRequest.model_validate(
            {"meeting_id": "m1", "cursor": 1}
        )
    summary = _models.MeetingSummaryGetAnswer.model_validate({"summary": None})
    assert summary.summary is None
    record = {
        "summary_id": "s1",
        "stale": False,
        "transcript_digest": DIGEST,
        "context_digest": DIGEST,
        "receipt_id": "r1",
        "receipt_digest": DIGEST,
        "output_digest": DIGEST,
        "provider": "p",
        "model": "m",
        "policy_revision": "v1",
        "claims": [
            {
                "claim_id": "c1",
                "text": "t",
                "citations": [{"span_id": "sp1", "start": 0, "end": 1}],
            }
        ],
    }
    stored = _models.MeetingSummaryGetAnswer.model_validate({"summary": record})
    assert stored.summary is not None and stored.summary.claims[0].claim_id == "c1"
    assert ingestion.send_meeting_transcript_read is not None
    assert ingestion.send_meeting_summary_get is not None
