"""AgentAssemble template_key client vector (MEDULLA-PLACEMENT-R001.12).

Static client check; no engine. The request body carries the optional
``template_key`` string and still refuses a field the contract does not name.
"""

from __future__ import annotations

import pytest
from pydantic import ValidationError

from medulla.generated.storage import AgentAssembleRequest

pytestmark = pytest.mark.no_engine

REQUEST = {
    "tenant_id": "tenant-a",
    "requirements": {},
    "candidates": {"kinds": ["tool"]},
    "policy": {"policy": "default"},
}


@pytest.mark.spec("MEDULLA-PLACEMENT-R001.12")
def test_assembly_request_carries_template_key() -> None:
    body = {"request": {**REQUEST, "template_key": "deployment:compose-host"}}
    parsed = AgentAssembleRequest.model_validate(body)
    assert parsed.request.template_key == "deployment:compose-host"
    assert (
        AgentAssembleRequest.model_validate({"request": REQUEST}).request.template_key
        is None
    )
    with pytest.raises(ValidationError):
        AgentAssembleRequest.model_validate({"request": {**REQUEST, "template": "x"}})
