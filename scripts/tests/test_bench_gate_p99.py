"""MEDULLA-FEDERATED-QUERY-R703.100: the bench gate checks p99 ceilings."""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import pytest

pytestmark = pytest.mark.no_engine

_GATE = Path(__file__).resolve().parents[1] / "bench_gate.py"


def _run(
    tmp_path: Path, ceilings: dict[str, int], samples: dict[str, list[float] | None]
):
    tmp_path.mkdir(parents=True)
    thresholds = tmp_path / "thresholds.json"
    thresholds.write_text(
        json.dumps(
            {
                "recall_floor": 0.5,
                "latency_p50_ns_max": {},
                "latency_p99_ns_max": ceilings,
            }
        )
    )
    recall = tmp_path / "recall.json"
    recall.write_text(json.dumps({"recall_at_k": 0.9, "k": 10}))
    criterion = tmp_path / "criterion"
    for name, per_iter in samples.items():
        if per_iter is None:
            continue
        directory = criterion / name / "new"
        directory.mkdir(parents=True)
        iters = [10.0] * len(per_iter)
        (directory / "sample.json").write_text(
            json.dumps({"iters": iters, "times": [p * 10.0 for p in per_iter]})
        )
    return subprocess.run(
        [
            sys.executable,
            str(_GATE),
            "--thresholds",
            str(thresholds),
            "--criterion-dir",
            str(criterion),
            "--recall-json",
            str(recall),
        ],
        capture_output=True,
        text=True,
        timeout=30,
    )


# spec: MEDULLA-FEDERATED-QUERY-R703.100
@pytest.mark.spec("MEDULLA-FEDERATED-QUERY-R703.100")
def test_p99_over_ceiling_fails_and_missing_sample_fails(tmp_path: Path) -> None:
    under = [1000.0] * 100
    # Nearest-rank p99 of 100 samples is the 99th smallest: one outlier is
    # ignored, two outliers above the ceiling fail.
    one_outlier = [1000.0] * 99 + [5_000_000.0]
    two_outliers = [1000.0] * 98 + [5_000_000.0] * 2

    ok = _run(tmp_path / "ok", {"k": 1_000_000}, {"k": under})
    assert ok.returncode == 0, ok.stderr

    tolerated = _run(tmp_path / "one", {"k": 1_000_000}, {"k": one_outlier})
    assert tolerated.returncode == 0, tolerated.stderr

    over = _run(tmp_path / "over", {"k": 1_000_000}, {"k": two_outliers})
    assert over.returncode == 1
    assert "k p99 5000000 ns > ceiling 1000000 ns" in over.stderr

    missing = _run(tmp_path / "missing", {"k": 1_000_000}, {"k": None})
    assert missing.returncode == 1
    assert "k p99 sample.json missing" in missing.stderr

    short = _run(tmp_path / "short", {"k": 1_000_000}, {"k": [1000.0] * 99})
    assert short.returncode == 1
    assert "at least 100 samples, found 99" in short.stderr
