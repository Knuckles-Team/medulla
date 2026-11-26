"""Gate: the wire models refuse a field their Rust result no longer carries."""

from __future__ import annotations

from typing import Any

import pytest
from pydantic import ValidationError

from medulla.generated import models

pytestmark = pytest.mark.no_engine


def _health_body() -> dict[str, Any]:
    return {
        "uptime_s": 1,
        "mem_bytes": 2,
        "version": "0.0.0",
        "build_profile": "release",
        "graph_lifecycle": {
            "catalog_graphs": 0,
            "resident_graphs": 0,
            "complete_graphs": 0,
            "partial_graphs": 0,
            "failed_graphs": 0,
            "all_resident_materializations_valid": True,
        },
        "ops": [],
    }


@pytest.mark.spec("MEDULLA-CONTRACT-R062.3")
def test_health_report_refuses_a_status_field() -> None:
    # spec: MEDULLA-CONTRACT-R062.3
    models.HealthReport.model_validate(_health_body())
    with pytest.raises(ValidationError):
        models.HealthReport.model_validate({**_health_body(), "status": "ok"})


def _method_variants() -> dict[str, type[models.BaseModel]]:
    from typing import get_args, get_type_hints

    variants = get_args(get_args(models.Method)[0])
    return {get_args(get_type_hints(cls)["method"])[0]: cls for cls in variants}


@pytest.mark.spec("MEDULLA-CONTRACT-R001.5")
def test_every_wire_method_has_strict_models() -> None:
    # spec: MEDULLA-CONTRACT-R001.5
    variants = _method_variants()
    assert variants, "models.Method has no variants"
    loose: list[str] = []
    for tag, cls in variants.items():
        if cls.model_config.get("extra") != "forbid":
            loose.append(f"{cls.__name__}")
        params = getattr(models, f"Method{tag}Params", None)
        if params is not None and params.model_config.get("extra") != "forbid":
            loose.append(f"{params.__name__}")
    assert not loose, f"models that accept an unknown field: {loose}"

    # One unknown nested field is refused before dispatch; a valid payload reads typed.
    valid = {"tenant": "tenant-a", "work_item_id": "work-1"}
    request = models.MethodGetWorkItem.model_validate(
        {"method": "GetWorkItem", "params": valid}
    )
    assert request.params.work_item_id == "work-1"
    with pytest.raises(ValidationError) as refused:
        models.MethodGetWorkItem.model_validate(
            {"method": "GetWorkItem", "params": {**valid, "nested": {"x": 1}}}
        )
    assert refused.value.errors()[0]["type"] == "extra_forbidden"
    assert refused.value.errors()[0]["loc"][:2] == ("params", "nested")
    typed = models.WorkItemTransitionPage.model_validate(
        {
            "entries": [{"state": "ready", "at_ms": 1, "revision": 1}],
            "next_cursor": None,
        }
    )
    assert typed.entries[0].state == "ready"
    with pytest.raises(ValidationError):
        models.WorkItemTransitionPage.model_validate(
            {"entries": [{"state": "ready", "at_ms": 1, "revision": 1, "x": 0}]}
        )
