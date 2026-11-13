#!/usr/bin/env python3
"""Fail when a capability-designation bench value exceeds its budget.

Reads the report the decide bench writes (`DECIDE_BUDGETS_REPORT`) and the
group budgets of `benches/baselines/decide.json` and checks, for every
`decide/capability_designation*` group, the unfiltered and filtered read
latencies, the incremental upsert p99, recall@5 (a floor) and the resident
bytes (at most `1.25 * N * dim` for vectors plus 96 bytes per record)
(MEDULLA-DECISION-ENGINE-R172.10). The three generic metrics of every other
group stay with `cargo xtask decide-budgets`.

Usage: check_decide_budgets.py <report.json> [baseline.json]
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

DEFAULT_BASELINE = Path(__file__).resolve().parent.parent / "benches/baselines/decide.json"
PREFIX = "decide/capability_designation"
CEILINGS = ("p50_ms", "p99_ms", "filtered_p50_ms", "filtered_p99_ms", "upsert_p99_ms")


def findings(report: dict, baseline: dict) -> list[str]:
    """One finding per breach; empty means pass."""
    groups = {
        name: value
        for name, value in report.get("groups", {}).items()
        if name.startswith(PREFIX)
    }
    if not groups:
        return [f"report holds no {PREFIX} group"]
    out: list[str] = []
    for name in sorted(groups):
        measured = groups[name]
        budget = baseline.get("group_budgets", {}).get(name)
        if budget is None:
            out.append(f"{name}: no group budget in the baseline")
            continue
        for metric in CEILINGS:
            if metric not in measured:
                out.append(f"{name}: missing {metric}")
            elif measured[metric] > budget[metric]:
                out.append(
                    f"{name}: {metric} {measured[metric]:g} exceeds budget {budget[metric]:g}"
                )
        if "recall_at_5" not in measured:
            out.append(f"{name}: missing recall_at_5")
        elif measured["recall_at_5"] < budget["recall_at_5_min"]:
            out.append(
                f"{name}: recall_at_5 {measured['recall_at_5']:g} is below "
                f"{budget['recall_at_5_min']:g}"
            )
        if not {"resident_bytes", "records", "dim"} <= measured.keys():
            out.append(f"{name}: missing resident_bytes, records or dim")
        else:
            allowed = (
                budget["resident_bytes_vector_factor"] * measured["records"] * measured["dim"]
                + budget["resident_bytes_per_record_overhead"] * measured["records"]
            )
            if measured["resident_bytes"] > allowed:
                out.append(
                    f"{name}: resident_bytes {measured['resident_bytes']:g} exceeds budget "
                    f"{allowed:g}"
                )
    return out


def main(argv: list[str]) -> int:
    if len(argv) not in (2, 3):
        print(__doc__, file=sys.stderr)
        return 2
    report = json.loads(Path(argv[1]).read_text())
    baseline = json.loads(Path(argv[2] if len(argv) == 3 else DEFAULT_BASELINE).read_text())
    problems = findings(report, baseline)
    for problem in problems:
        print(problem)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
