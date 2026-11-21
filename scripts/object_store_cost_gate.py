#!/usr/bin/env python3
"""Comparative cost gate of the object-store backend (MEDULLA-OBJECT-STORAGE-BACKEND-R005).

Reads the JSON lines of ``benches/object_store_cost.rs`` and compares them with the previous
release record in ``benches/baselines/object_store_cost.json``. The gate fails the release when
the object actions of one cold lifecycle grow more than the baseline's tolerance (10 percent).

A run is claim-eligible only when it is stamped with a revision and pinned to the same
lifecycle as the baseline. ``COST_RUN_UNSTAMPED`` and ``COST_RUN_UNPINNED`` mark a run
claim-ineligible: it never enters the evidence, and the gate does not judge it.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

COST_RUN_UNSTAMPED = "COST_RUN_UNSTAMPED"
COST_RUN_UNPINNED = "COST_RUN_UNPINNED"
REGRESSION = "COST_REGRESSION"
GUARDED_DIMENSION = "object_actions"
DEFAULT_BASELINE = Path(__file__).resolve().parent.parent / "benches" / "baselines" / (
    "object_store_cost.json"
)
REQUIRED_FIELDS = (
    "schema_version",
    "host",
    "revision",
    "lifecycle_digest",
    "dimension",
    "value",
)


def load_records(path: Path) -> list[dict]:
    records = []
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line:
            continue
        record = json.loads(line)
        missing = [field for field in REQUIRED_FIELDS if field not in record]
        if missing:
            raise ValueError(f"cost record lacks {missing}: {line}")
        records.append(record)
    return records


def evaluate(records: list[dict], baseline: dict) -> dict:
    """The verdict of one run: eligibility, and the failures when it is eligible."""

    ineligible: list[str] = []
    if any(not str(record["revision"]).strip() for record in records) or not records:
        ineligible.append(COST_RUN_UNSTAMPED)
    if any(record["lifecycle_digest"] != baseline["lifecycle_digest"] for record in records):
        ineligible.append(COST_RUN_UNPINNED)
    if ineligible:
        return {"eligible": False, "codes": ineligible, "failures": []}
    tolerance = float(baseline["regression_tolerance"])
    failures = []
    for record in records:
        dimension = record["dimension"]
        if dimension != GUARDED_DIMENSION:
            continue
        previous = baseline["dimensions"].get(dimension)
        if previous is None:
            continue
        if float(record["value"]) > float(previous) * (1 + tolerance):
            failures.append(
                {
                    "code": REGRESSION,
                    "dimension": dimension,
                    "previous": previous,
                    "value": record["value"],
                    "tolerance": tolerance,
                }
            )
    return {"eligible": True, "codes": [], "failures": failures}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("records", type=Path, help="JSON lines of the cost bench")
    parser.add_argument("--baseline", type=Path, default=DEFAULT_BASELINE)
    args = parser.parse_args(argv)
    baseline = json.loads(args.baseline.read_text(encoding="utf-8"))
    verdict = evaluate(load_records(args.records), baseline)
    print(json.dumps(verdict, sort_keys=True))
    if not verdict["eligible"] or verdict["failures"]:
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
