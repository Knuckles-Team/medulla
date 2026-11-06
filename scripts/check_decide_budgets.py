#!/usr/bin/env python3
"""Compare a decision-budget run to its stored baseline.

Spec: MEDULLA-DECISION-ENGINE-R178.2.

A run report and the baseline (`benches/baselines/decide.json`) are keyed by
Criterion group (`decide/<family>`), each with `p50_ms`, `p99_ms` and
`peak_mib`. The check fails when a measured value exceeds the absolute budget
of R178 (p50 at most 20 ms, p99 at most 150 ms, peak resident memory at most
32 MiB per call) or regresses more than the baseline's tolerance (10 %) above
its stored baseline. A group with no stored baseline is checked against the
absolute budgets only.

Usage: check_decide_budgets.py <run-report.json> [--baseline PATH]
Exit status 0 when every group is within budget, 1 otherwise.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_BASELINE = ROOT / "benches" / "baselines" / "decide.json"
METRICS = ("p50_ms", "p99_ms", "peak_mib")


def load(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def check(run: dict[str, Any], baseline: dict[str, Any]) -> list[str]:
    """Return one finding per budget breach or regression; empty means pass."""
    budgets: dict[str, float] = baseline["budgets"]
    tolerance = float(baseline["regression_tolerance"])
    stored: dict[str, dict[str, float]] = baseline.get("groups", {})
    findings: list[str] = []
    groups: dict[str, dict[str, float]] = run.get("groups", {})
    if not groups:
        findings.append("run report has no groups")
    for group in sorted(groups):
        measured = groups[group]
        for metric in METRICS:
            if metric not in measured:
                findings.append(f"{group}: missing {metric}")
                continue
            value = float(measured[metric])
            if value > float(budgets[metric]):
                findings.append(
                    f"{group}: {metric} {value:g} exceeds budget {budgets[metric]:g}"
                )
            base = stored.get(group, {}).get(metric)
            if base is not None and value > float(base) * (1.0 + tolerance):
                pct = (value / float(base) - 1.0) * 100.0
                findings.append(
                    f"{group}: {metric} {value:g} regressed {pct:.1f}% "
                    f"over baseline {float(base):g}"
                )
    return findings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("report", type=Path)
    parser.add_argument("--baseline", type=Path, default=DEFAULT_BASELINE)
    args = parser.parse_args(argv)
    findings = check(load(args.report), load(args.baseline))
    for finding in findings:
        print(f"FAIL {finding}")
    if findings:
        return 1
    print("decide budgets: every group within budget")
    return 0


if __name__ == "__main__":
    sys.exit(main())
