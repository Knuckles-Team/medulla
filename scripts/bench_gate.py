#!/usr/bin/env python3
"""CONCEPT:MEDULLA-KG.query.perf-recall-ci-gate — the perf/recall regression GATE for
the
medulla-plan hybrid_queries bench.

Runs AFTER `cargo bench -p medulla-plan --features …` has produced its criterion output.
It
reads two artifacts and fails (exit 1) on a regression:

  1. **recall** — the `medulla_plan_recall.json` the bench writes (measured recall@k of
  the
     vector leg vs the brute-force oracle). Must stay >= `recall_floor`. The bench ALSO
     asserts this internally, so a recall regression already fails the bench binary;
     this
     is the redundant, reported check.

  2. **latency** — criterion's per-bench `estimates.json`
     (`target/criterion/<bench>/new/estimates.json`), the MEDIAN (p50) point estimate in
     nanoseconds. Must stay <= the committed per-bench ceiling. Ceilings are generous
     (see thresholds.json) so this is a catastrophic-regression net, not a p50 chase —
     it
     never flakes on machine variance. A bench with no estimates (feature not built) is
     skipped.

  3. **p99 latency** — `latency_p99_ns_max` in thresholds.json, keyed by criterion
     benchmark id. The gate reads `target/criterion/<id>/new/sample.json`, takes each
     sample's per-iteration time `times[i] / iters[i]` (ns) and its nearest-rank 99th
     percentile. Fewer than 100 samples or a missing `sample.json` is a failure, never
     a skip.

Usage:
  python3 scripts/bench_gate.py \
      [--thresholds crates/medulla-plan/benches/thresholds.json] \
      [--criterion-dir target/criterion] \
      [--recall-json target/medulla_plan_recall.json]
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


def load_json(path: Path):
    with path.open() as fh:
        return json.load(fh)


def _build_arg_parser() -> argparse.ArgumentParser:
    ap = argparse.ArgumentParser(description=__doc__)
    repo = Path(__file__).resolve().parent.parent
    ap.add_argument(
        "--thresholds",
        type=Path,
        default=repo / "crates" / "medulla-plan" / "benches" / "thresholds.json",
    )
    ap.add_argument("--criterion-dir", type=Path, default=repo / "target" / "criterion")
    ap.add_argument(
        "--finance-report",
        type=Path,
        default=repo / "target" / "finance-r613-report.json",
    )
    ap.add_argument(
        "--recall-json", type=Path, default=repo / "target" / "medulla_plan_recall.json"
    )
    return ap


def _check_recall(
    recall_json: Path, recall_floor: float
) -> tuple[list[str], list[str]]:
    """Recall gate: report + failures for the measured recall@k, if present."""
    if not recall_json.exists():
        return [], [
            f"recall artifact missing: {recall_json} "
            "(did the bench run with --features query?)"
        ]
    rec = load_json(recall_json)
    measured = float(rec["recall_at_k"])
    k = rec.get("k", "?")
    ok = measured >= recall_floor
    reported = [
        f"  recall@{k}: {measured:.4f}  (floor {recall_floor:.4f})  "
        f"{'OK' if ok else 'FAIL'}"
    ]
    failures = [] if ok else [f"recall@{k} {measured:.4f} < floor {recall_floor:.4f}"]
    return reported, failures


def _check_one_latency(
    criterion_dir: Path, name: str, ceiling: float
) -> tuple[str, str | None]:
    """Latency gate for a single bench: one report line, plus a failure if over."""
    est = criterion_dir / name / "new" / "estimates.json"
    if not est.exists():
        return f"  {name}: (skipped — no estimates; feature not built)", None
    data = load_json(est)
    p50 = float(data["median"]["point_estimate"])  # nanoseconds
    ok = p50 <= float(ceiling)
    line = (
        f"  {name}: p50 {p50 / 1e6:.3f} ms  (ceiling {float(ceiling) / 1e6:.1f} ms)  "
        f"{'OK' if ok else 'FAIL'}"
    )
    if ok:
        return line, None
    return (
        line,
        f"{name} p50 {p50 / 1e6:.3f} ms > ceiling {float(ceiling) / 1e6:.1f} ms",
    )


def _check_latency(
    criterion_dir: Path, ceilings: dict[str, float]
) -> tuple[list[str], list[str]]:
    """Latency gate: report + failures across every bench with a committed ceiling."""
    reported: list[str] = []
    failures: list[str] = []
    for name, ceiling in ceilings.items():
        line, failure = _check_one_latency(criterion_dir, name, ceiling)
        reported.append(line)
        if failure is not None:
            failures.append(failure)
    return reported, failures


def _check_finance_p99(report_path: Path, section: dict) -> tuple[list[str], list[str]]:
    """MEDULLA-FINANCE-PRIMITIVES-R613.6: p99 budget and 10 percent regression gate.

    A group over its absolute p99 budget fails; a group more than the tolerance
    above its stored baseline fails. A missing report is skipped (the bench is
    feature-gated), as the latency gate skips a bench that was not built.
    """
    if not report_path.exists():
        return [f"  finance r613: (skipped — no report at {report_path.name})"], []
    groups = load_json(report_path)["groups"]
    tolerance = float(section["regression_tolerance"])
    baselines = section.get("baseline_p99_ms", {})
    reported: list[str] = []
    failures: list[str] = []
    for name, budget in section["p99_ms_max"].items():
        if name not in groups:
            failures.append(f"{name} missing from {report_path.name}")
            continue
        p99 = float(groups[name]["p99_ms"])
        base = baselines.get(name)
        over_budget = p99 > float(budget)
        regressed = base is not None and p99 > float(base) * (1.0 + tolerance)
        ok = not (over_budget or regressed)
        reported.append(
            f"  {name}: p99 {p99:.3f} ms  (budget {float(budget):.1f} ms)  "
            f"{'OK' if ok else 'FAIL'}"
        )
        if over_budget:
            failures.append(f"{name} p99 {p99:.3f} ms > budget {float(budget):.1f} ms")
        if regressed:
            failures.append(
                f"{name} p99 {p99:.3f} ms regressed over {tolerance:.0%} above "
                f"baseline {float(base):.3f} ms"
            )
    return reported, failures


MIN_P99_SAMPLES = 100


def _nearest_rank_p99(per_iter_ns: list[float]) -> float:
    """Nearest-rank 99th percentile: the value at rank ceil(0.99 * n)."""
    ordered = sorted(per_iter_ns)
    rank = -(-99 * len(ordered) // 100)
    return ordered[rank - 1]


def _check_one_p99(
    criterion_dir: Path, name: str, ceiling: float
) -> tuple[str, str | None]:
    """p99 gate for a single bench. A missing or short sample.json is a failure."""
    sample = criterion_dir / name / "new" / "sample.json"
    if not sample.exists():
        return (
            f"  {name}: p99 FAIL (no sample.json)",
            f"{name} p99 sample.json missing (ceiling {float(ceiling):.0f} ns)",
        )
    data = load_json(sample)
    iters = [float(i) for i in data["iters"]]
    times = [float(t) for t in data["times"]]
    if len(times) != len(iters) or len(times) < MIN_P99_SAMPLES:
        return (
            f"  {name}: p99 FAIL ({len(times)} samples)",
            f"{name} p99 needs at least {MIN_P99_SAMPLES} samples, found {len(times)}",
        )
    p99 = _nearest_rank_p99([t / i for t, i in zip(times, iters, strict=True)])
    ok = p99 <= float(ceiling)
    line = (
        f"  {name}: p99 {p99:.0f} ns  (ceiling {float(ceiling):.0f} ns)  "
        f"{'OK' if ok else 'FAIL'}"
    )
    if ok:
        return line, None
    return line, f"{name} p99 {p99:.0f} ns > ceiling {float(ceiling):.0f} ns"


def _check_p99(
    criterion_dir: Path, ceilings: dict[str, float]
) -> tuple[list[str], list[str]]:
    """p99 gate: report + failures across every bench with a committed p99 ceiling."""
    reported: list[str] = []
    failures: list[str] = []
    for name, ceiling in ceilings.items():
        line, failure = _check_one_p99(criterion_dir, name, ceiling)
        reported.append(line)
        if failure is not None:
            failures.append(failure)
    return reported, failures


def _emit_result(reported: list[str], failures: list[str]) -> int:
    print(
        "medulla-plan perf/recall gate (CONCEPT:MEDULLA-KG.query.perf-recall-ci-gate):"
    )
    print("\n".join(reported))

    if failures:
        print("\nREGRESSION — the gate FAILED:", file=sys.stderr)
        for f in failures:
            print(f"  - {f}", file=sys.stderr)
        return 1
    print("\nall perf/recall thresholds held.")
    return 0


def main() -> int:
    args = _build_arg_parser().parse_args()

    thresholds = load_json(args.thresholds)
    recall_floor = float(thresholds["recall_floor"])
    ceilings: dict[str, float] = thresholds["latency_p50_ns_max"]

    recall_reported, recall_failures = _check_recall(args.recall_json, recall_floor)
    latency_reported, latency_failures = _check_latency(args.criterion_dir, ceilings)

    finance_reported, finance_failures = (
        _check_finance_p99(args.finance_report, thresholds["finance_r613"])
        if "finance_r613" in thresholds
        else ([], [])
    )
    p99_reported, p99_failures = _check_p99(
        args.criterion_dir, thresholds.get("latency_p99_ns_max", {})
    )

    return _emit_result(
        recall_reported + latency_reported + finance_reported + p99_reported,
        recall_failures + latency_failures + finance_failures + p99_failures,
    )


if __name__ == "__main__":
    raise SystemExit(main())
