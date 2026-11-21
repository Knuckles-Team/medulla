"""The comparative cost gate (MEDULLA-OBJECT-STORAGE-BACKEND-R005)."""

import importlib.util
import json
from pathlib import Path

import pytest

pytestmark = [
    pytest.mark.no_engine,
    pytest.mark.spec("MEDULLA-OBJECT-STORAGE-BACKEND-R005"),
]

ROOT = Path(__file__).resolve().parent.parent
_spec = importlib.util.spec_from_file_location(
    "object_store_cost_gate", ROOT / "scripts" / "object_store_cost_gate.py"
)
gate = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(gate)

BASELINE = json.loads(
    (ROOT / "benches" / "baselines" / "object_store_cost.json").read_text()
)
PREVIOUS_ACTIONS = 1000.0


def _baseline() -> dict:
    return {**BASELINE, "dimensions": {**BASELINE["dimensions"], "object_actions": PREVIOUS_ACTIONS}}


def _records(actions: float, *, revision: str = "abc123", digest: str | None = None):
    return [
        {
            "schema_version": 1,
            "host": "host-a",
            "revision": revision,
            "lifecycle_digest": digest or BASELINE["lifecycle_digest"],
            "dimension": "object_actions",
            "value": actions,
        }
    ]


def test_regression_guard_and_claim_eligibility(tmp_path):
    # Within the tolerance: the gate passes.
    ok = gate.evaluate(_records(PREVIOUS_ACTIONS * 1.05), _baseline())
    assert ok["eligible"] and ok["failures"] == []

    # A 15 percent growth of open actions fails the release.
    grown = gate.evaluate(_records(PREVIOUS_ACTIONS * 1.15), _baseline())
    assert grown["eligible"]
    assert [failure["code"] for failure in grown["failures"]] == [gate.REGRESSION]

    # An unstamped run is claim-ineligible and is not judged.
    unstamped = gate.evaluate(_records(PREVIOUS_ACTIONS * 2, revision=""), _baseline())
    assert unstamped == {
        "eligible": False,
        "codes": [gate.COST_RUN_UNSTAMPED],
        "failures": [],
    }

    # A run of another lifecycle is unpinned.
    unpinned = gate.evaluate(_records(10.0, digest="0" * 64), _baseline())
    assert unpinned["codes"] == [gate.COST_RUN_UNPINNED]

    # The command exits non-zero for a regression and for an ineligible run.
    baseline_path = tmp_path / "baseline.json"
    baseline_path.write_text(json.dumps(_baseline()))
    for name, records, code in (
        ("grown", _records(PREVIOUS_ACTIONS * 1.15), 1),
        ("clean", _records(PREVIOUS_ACTIONS), 0),
        ("unstamped", _records(PREVIOUS_ACTIONS, revision=" "), 1),
    ):
        path = tmp_path / f"{name}.jsonl"
        path.write_text("\n".join(json.dumps(record) for record in records))
        assert gate.main([str(path), "--baseline", str(baseline_path)]) == code, name
