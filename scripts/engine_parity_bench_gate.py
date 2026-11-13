#!/usr/bin/env python3
"""Nightly gate over the engine parity benchmark records (MEDULLA-ENGINE-PARITY-R005.2).

Reads `target/engine_parity/records.jsonl` (written by `benches/engine_parity.rs`)
and the stored baseline `benches/baselines/engine_parity.json`, and refuses with a
stable code:

* `BENCH_RECORD_INELIGIBLE`  a record lacks `host`, `revision` or `data_digest`
                             and so cannot back a claim;
* `BENCH_WORKLOAD_MISSING`   any of the five write workloads W1 to W5 is absent;
* `BENCH_BASELINE_MISSING`   the baseline file does not exist;
* `BENCH_BASELINE_INVALID`   the baseline is not
                             `{"workloads": {name: {p99_us, throughput}}}`;
* `BENCH_REGRESSION`         a metric is more than 10 percent worse than its baseline
                             (p99 higher, or throughput lower).

Exit status 0 means every check held.

Usage:
  python3 scripts/engine_parity_bench_gate.py \
      [--records target/engine_parity/records.jsonl] \
      [--baseline benches/baselines/engine_parity.json]
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

REQUIRED_WORKLOADS = (
    "single_row_commit",
    "concurrent_64",
    "shards_8",
    "bulk_load",
    "conflict_heavy",
)
REQUIRED_RECORD_FIELDS = ("host", "revision", "data_digest")
BASELINE_METRICS = ("p99_us", "throughput")
REGRESSION_LIMIT = 0.10


class GateRefusal(Exception):
    """A refusal carrying the stable code and its message."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(f"{code}: {message}")
        self.code = code


def load_records(path: Path) -> list[dict]:
    """Load the JSONL records; the last record of a workload wins."""
    records: list[dict] = []
    with path.open() as fh:
        for number, line in enumerate(fh, start=1):
            line = line.strip()
            if not line:
                continue
            try:
                record = json.loads(line)
            except json.JSONDecodeError as err:
                raise GateRefusal(
                    "BENCH_RECORD_INELIGIBLE", f"{path}:{number} is not JSON ({err})"
                ) from err
            if not isinstance(record, dict) or "workload" not in record:
                raise GateRefusal(
                    "BENCH_RECORD_INELIGIBLE", f"{path}:{number} names no workload"
                )
            records.append(record)
    return records


def load_baseline(path: Path) -> dict[str, dict[str, float]]:
    if not path.is_file():
        raise GateRefusal("BENCH_BASELINE_MISSING", f"{path} does not exist")
    try:
        baseline = json.loads(path.read_text())
    except json.JSONDecodeError as err:
        raise GateRefusal(
            "BENCH_BASELINE_INVALID", f"{path} is not JSON ({err})"
        ) from err
    workloads = baseline.get("workloads") if isinstance(baseline, dict) else None
    if not isinstance(workloads, dict) or not workloads:
        raise GateRefusal("BENCH_BASELINE_INVALID", f"{path} has no `workloads` object")
    for name, metrics in workloads.items():
        if not isinstance(metrics, dict) or any(
            not isinstance(metrics.get(m), (int, float))
            or isinstance(metrics.get(m), bool)
            for m in BASELINE_METRICS
        ):
            raise GateRefusal(
                "BENCH_BASELINE_INVALID",
                f"workload {name} needs numeric {', '.join(BASELINE_METRICS)}",
            )
    return workloads


def check_eligibility(records: list[dict]) -> None:
    for record in records:
        missing = [f for f in REQUIRED_RECORD_FIELDS if not record.get(f)]
        if missing:
            raise GateRefusal(
                "BENCH_RECORD_INELIGIBLE",
                f"workload {record['workload']} lacks {', '.join(missing)}",
            )


def check_workloads(records: list[dict]) -> dict[str, dict]:
    by_name = {r["workload"]: r for r in records}
    absent = [w for w in REQUIRED_WORKLOADS if w not in by_name]
    if absent:
        raise GateRefusal("BENCH_WORKLOAD_MISSING", ", ".join(absent))
    return by_name


def check_regressions(by_name: dict[str, dict], baseline: dict[str, dict]) -> None:
    worse: list[str] = []
    for name, base in baseline.items():
        record = by_name.get(name)
        if record is None:
            raise GateRefusal("BENCH_WORKLOAD_MISSING", name)
        if record["p99_us"] > base["p99_us"] * (1 + REGRESSION_LIMIT):
            worse.append(
                f"{name} p99_us {record['p99_us']} vs baseline {base['p99_us']}"
            )
        if record["throughput"] < base["throughput"] * (1 - REGRESSION_LIMIT):
            got, want = record["throughput"], base["throughput"]
            worse.append(f"{name} throughput {got} vs baseline {want}")
    if worse:
        raise GateRefusal("BENCH_REGRESSION", "; ".join(worse))


def run_gate(records_path: Path, baseline_path: Path) -> None:
    records = load_records(records_path)
    check_eligibility(records)
    by_name = check_workloads(records)
    check_regressions(by_name, load_baseline(baseline_path))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--records", default="target/engine_parity/records.jsonl")
    parser.add_argument("--baseline", default="benches/baselines/engine_parity.json")
    args = parser.parse_args(argv)
    try:
        run_gate(Path(args.records), Path(args.baseline))
    except GateRefusal as refusal:
        print(refusal, file=sys.stderr)
        return 1
    except FileNotFoundError as err:
        print(f"BENCH_WORKLOAD_MISSING: {err}", file=sys.stderr)
        return 1
    print("engine parity benchmark gate held.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
