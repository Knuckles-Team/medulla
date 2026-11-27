"""MEDULLA-IDENTITY-R752 wire mirror of the idp link resolve read."""

from __future__ import annotations

import pytest
from pydantic import TypeAdapter, ValidationError

from medulla.generated import models as _models


# spec: MEDULLA-IDENTITY-R752
@pytest.mark.spec("MEDULLA-IDENTITY-R752")
def test_the_resolve_link_op_and_reply_mirror_the_wire_shape() -> None:
    op = _models.IdentityOpIdpResolveLink.model_validate(
        {
            "family": "idp",
            "op": "resolve_link",
            "request": {"idp_id": "github", "subject": "octocat"},
        }
    )
    assert op.request.subject == "octocat"
    reply = TypeAdapter(_models.IdentityReply)
    found = reply.validate_python(
        {"kind": "linked_principal", "value": {"principal_id": "usr:alice"}}
    )
    assert isinstance(found, _models.IdentityReplyLinkedPrincipal)
    assert found.value.principal_id == "usr:alice"
    absent = reply.validate_python({"kind": "linked_principal", "value": {}})
    assert isinstance(absent, _models.IdentityReplyLinkedPrincipal)
    assert absent.value.principal_id is None
    with pytest.raises(ValidationError):
        reply.validate_python(
            {"kind": "linked_principal", "value": {"principal_id": None, "x": 1}}
        )
