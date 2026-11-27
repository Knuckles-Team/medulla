"""MEDULLA-INGEST-WORKER-R001.18 wire mirror of the ingest worker status read.

L1: the generated models accept the engine's reply shape and refuse anything else,
with no engine running.
"""

from __future__ import annotations

import pytest
from pydantic import ValidationError

from medulla.generated import cluster
from medulla.generated import models as _models


# spec: MEDULLA-INGEST-WORKER-R001.18
@pytest.mark.spec("MEDULLA-INGEST-WORKER-R001.18")
def test_the_status_report_mirrors_the_wire_shape() -> None:
    report = _models.IngestWorkerStatusReport.model_validate(
        {
            "series": [
                {
                    "name": "medulla_lane_depth",
                    "labels": {"lane": "general", "queue_class": "fast"},
                    "value": 3.0,
                },
                {"name": "medulla_ingest_jobs_total", "labels": {}, "value": 0.0},
            ]
        }
    )
    assert report.series[0].labels["lane"] == "general"
    with pytest.raises(ValidationError):
        _models.IngestWorkerStatusReport.model_validate(
            {"series": [{"name": "x", "labels": {}, "value": 1, "extra": 1}]}
        )
    # The request carries no field.
    cluster.IngestWorkerStatusRequest.model_validate({})
    with pytest.raises(ValidationError):
        cluster.IngestWorkerStatusRequest.model_validate({"tenant": "t"})
