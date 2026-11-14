"""Behaviour tests for scripts/engine_parity_bench_gate.py.

spec: MEDULLA-ENGINE-PARITY-R005.2
spec: MEDULLA-ENGINE-PARITY-R005.3
"""

from __future__ import annotations

import json
import pathlib
import subprocess
import sys

import pytest

pytestmark = pytest.mark.no_engine

_GATE = (
    pathlib.Path(__file__).resolve().parents[1]
    / "scripts"
    / "engine_parity_bench_gate.py"
)
_WORKLOADS = (
    "single_row_commit",
    "concurrent_64",
    "shards_8",
    "bulk_load",
    "conflict_heavy",
)


def _record(name: str, p99: int = 1000, throughput: float = 1000.0, **extra) -> dict:
    record = {
        "workload": name,
        "host": "soak-1",
        "revision": "abc123",
        "data_digest": "d" * 64,
        "p50_us": p99 // 2,
        "p99_us": p99,
        "throughput": throughput,
    }
    record.update(extra)
    return record


def _run(tmp_path: pathlib.Path, records: list[dict], baseline: dict | None):
    records_path = tmp_path / "records.jsonl"
    records_path.write_text("".join(json.dumps(r) + "\n" for r in records))
    baseline_path = tmp_path / "baseline.json"
    if baseline is not None:
        baseline_path.write_text(json.dumps(baseline))
    return subprocess.run(
        [
            sys.executable,
            str(_GATE),
            "--records",
            str(records_path),
            "--baseline",
            str(baseline_path),
        ],
        capture_output=True,
        text=True,
        check=False,
    )


def _baseline() -> dict:
    return {
        "workloads": {n: {"p99_us": 1000, "throughput": 1000.0} for n in _WORKLOADS}
    }


def _full() -> list[dict]:
    return [_record(n) for n in _WORKLOADS]


def test_full_run_within_baseline_passes(tmp_path):
    done = _run(tmp_path, _full(), _baseline())
    assert done.returncode == 0, done.stderr


# spec: MEDULLA-ENGINE-PARITY-R005.2
def test_missing_workload_fails(tmp_path):
    records = [r for r in _full() if r["workload"] != "shards_8"]
    done = _run(tmp_path, records, _baseline())
    assert done.returncode == 1
    assert "BENCH_WORKLOAD_MISSING" in done.stderr
    assert "shards_8" in done.stderr


# spec: MEDULLA-ENGINE-PARITY-R005.2
def test_regression_over_ten_percent_fails(tmp_path):
    records = _full()
    records[0] = _record("single_row_commit", p99=1101)
    done = _run(tmp_path, records, _baseline())
    assert done.returncode == 1
    assert "BENCH_REGRESSION" in done.stderr
    records[0] = _record("single_row_commit", p99=1100)
    assert _run(tmp_path, records, _baseline()).returncode == 0
    records[0] = _record("single_row_commit", throughput=899.0)
    slow = _run(tmp_path, records, _baseline())
    assert slow.returncode == 1
    assert "BENCH_REGRESSION" in slow.stderr


@pytest.mark.parametrize("field", ["host", "revision", "data_digest"])
def test_record_without_claim_field_is_ineligible(tmp_path, field):
    records = _full()
    del records[1][field]
    done = _run(tmp_path, records, _baseline())
    assert done.returncode == 1
    assert "BENCH_RECORD_INELIGIBLE" in done.stderr
    assert field in done.stderr


def test_missing_baseline_refuses(tmp_path):
    done = _run(tmp_path, _full(), None)
    assert done.returncode == 1
    assert "BENCH_BASELINE_MISSING" in done.stderr


_BASELINE_FILE = (
    pathlib.Path(__file__).resolve().parents[1]
    / "benches"
    / "baselines"
    / "engine_parity.json"
)


# spec: MEDULLA-ENGINE-PARITY-R005.3
def test_baseline_file_matches_records_schema(tmp_path):
    """spec: MEDULLA-ENGINE-PARITY-R005.3

    The committed baseline names exactly the five required workloads, each with
    the numeric metrics a record carries, and the gate accepts records equal to it.
    """
    baseline = json.loads(_BASELINE_FILE.read_text())
    assert set(baseline) == {"workloads"}
    assert set(baseline["workloads"]) == set(_WORKLOADS)
    for name, metrics in baseline["workloads"].items():
        assert set(metrics) == {"p99_us", "throughput"}, name
        assert metrics["p99_us"] > 0 and metrics["throughput"] > 0, name
    records = [
        _record(n, p99=m["p99_us"], throughput=m["throughput"])
        for n, m in baseline["workloads"].items()
    ]
    records_path = tmp_path / "records.jsonl"
    records_path.write_text("".join(json.dumps(r) + "\n" for r in records))
    done = subprocess.run(
        [
            sys.executable,
            str(_GATE),
            "--records",
            str(records_path),
            "--baseline",
            str(_BASELINE_FILE),
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    assert done.returncode == 0, done.stderr
