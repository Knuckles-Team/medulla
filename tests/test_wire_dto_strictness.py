"""Gate: the hand-written wire models refuse a field their Rust result no longer carries."""

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
