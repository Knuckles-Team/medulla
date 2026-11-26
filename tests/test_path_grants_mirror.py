"""MEDULLA-SCM-R007.1: the Python mirror of the ListPathGrants shapes."""

from __future__ import annotations

import pytest
from pydantic import TypeAdapter, ValidationError

from medulla.generated import models

pytestmark = pytest.mark.no_engine


@pytest.mark.spec("MEDULLA-SCM-R007.1")
def test_list_path_grants_request_and_answer_mirror_the_wire() -> None:
    call = TypeAdapter(models.Method).validate_python(
        {"method": "ListPathGrants", "params": {"request": {"principal": "alice"}}}
    )
    assert isinstance(call, models.MethodListPathGrants)
    assert call.params.request.repo is None
    narrowed = models.ListPathGrantsRequest.model_validate(
        {"principal": "alice", "repo": "org/repo"}
    )
    assert narrowed.repo == "org/repo"
    with pytest.raises(ValidationError):
        models.ListPathGrantsRequest.model_validate(
            {"principal": "alice", "tenant": "t"}
        )
    answer = models.PathGrantList.model_validate(
        {
            "grants": [
                {
                    "repo": "org/repo",
                    "principal": "alice",
                    "prefix": "src/secret",
                    "read": False,
                    "write": False,
                }
            ]
        }
    )
    deny = answer.grants[0]
    assert (deny.read, deny.write) == (False, False)
    assert models.PathGrantList.model_validate({"grants": []}).grants == []
