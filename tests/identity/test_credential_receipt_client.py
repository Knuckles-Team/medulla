"""MEDULLA-IDENTITY-R753 wire mirror of the credential receipt record."""

from __future__ import annotations

import pytest
from pydantic import ValidationError

from medulla.generated import ingestion
from medulla.generated import models as _models

RECEIPT = {
    "tenant": "t1",
    "client": "agent-a",
    "connector": "github",
    "resource": "repo:x",
    "operation": "read",
    "outcome": "allowed",
    "binding_digest": "sha256:" + "a" * 64,
    "at_ms": 5,
}


# spec: MEDULLA-IDENTITY-R753
@pytest.mark.spec("MEDULLA-IDENTITY-R753")
def test_the_receipt_models_mirror_the_wire_shape() -> None:
    method = _models.MethodCredentialReceipt.model_validate(
        {
            "method": "CredentialReceipt",
            "params": {"op": {"op": "record", "receipt": RECEIPT}},
        }
    )
    assert method.params.op.receipt.client == "agent-a"
    recorded = _models.CredentialReceiptRecorded.model_validate(
        {"receipt_id": "credential-receipt:" + "b" * 64}
    )
    assert recorded.receipt_id.startswith("credential-receipt:")
    with pytest.raises(ValidationError):
        _models.CredentialReceipt.model_validate({**RECEIPT, "secret": "x"})
    assert ingestion.send_credential_receipt is not None
