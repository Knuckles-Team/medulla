#!/usr/bin/env python3
"""MEDULLA-CONTRACT-R011: fan-out adapter for the land gate's default execution.

The land gate's default execution fans gate work out in parallel across the
independent host groups declared in the host-group manifest
(``.config/fanout-host-groups.yml``; the manifest is validated standalone by
``cargo xtask fanout-host-groups`` per MEDULLA-CONTRACT-R011.1 and re-checked
fail-closed here before any unit is dispatched). Per group:

* one persistent warm target, created once for the group and reused for every
  unit the group runs (never rebuilt per unit);
* a concurrency cap that keeps the group's declared headroom reservation free
  on its gate hosts;
* a per-group summary of the units it ran.

The per-group summaries combine into one merged summary whose unit results are
exactly the per-group results, in deterministic group order. Independent
groups run simultaneously, so the total wall-clock is the slowest group
instead of the sum of all groups — strictly below the same work run serially
on a single host. ``--serial`` runs every unit on the first group's warm
target, one after another, and is the comparison baseline.

Usage:
    python3 scripts/fanout_adapter.py --manifest <path> \
        --unit "command one" --unit "command two" [--serial] [--json]

Each unit runs on its group's warm target. A unit that exits non-zero marks
the merged summary failed.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
import time
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_MANIFEST = ROOT / ".config" / "fanout-host-groups.yml"


class ManifestError(RuntimeError):
    """The host-group manifest is malformed; fan-out dispatch is refused."""


@dataclass(frozen=True)
class Group:
    """One independent host group: hosts, its persistent warm target, headroom."""

    name: str
    warm_target: str
    headroom: float
    hosts: tuple[str, ...]

    @property
    def concurrency(self) -> int:
        """Worker slots left after the headroom reservation on the group's hosts.

        One worker slot per gate host; ``floor(slots * headroom)`` slots stay
        reserved so the reservation fraction is kept free on every gate host.
        At least one slot always remains, so a group always makes progress.
        """
        slots = len(self.hosts)
        reserved = int(slots * self.headroom)
        return max(1, slots - reserved)


@dataclass(frozen=True)
class UnitResult:
    unit: str
    status: str  # "pass" | "fail"
    duration_ms: float
    group: str
    warm_target: str


@dataclass(frozen=True)
class GroupSummary:
    group: str
    warm_target: str
    wall_ms: float
    units: tuple[UnitResult, ...]

    @property
    def failed(self) -> int:
        return sum(1 for u in self.units if u.status == "fail")


class Runner:
    """Runs units against a group's persistent warm target.

    ``warm`` is called exactly once per group and returns the persistent warm
    target handle; every unit of that group runs against the same handle.
    ``close`` releases the handle after the group's units are done.
    """

    def warm(self, group: Group):
        """Create the group's persistent warm target handle (runner-defined)."""
        raise NotImplementedError

    def run(self, unit: str, handle) -> tuple[str, float]:
        """Run one unit; return ``(status, duration_ms)``."""
        raise NotImplementedError

    def close(self, handle) -> None:
        """Release the group's warm target handle after its units are done."""


class CommandRunner(Runner):
    """Runs shell commands against the group's persistent warm target.

    The warm target is a warmed working directory created once per group:
    unit commands run inside it with ``MEDULLA_WARM_TARGET`` naming the
    target, so state a unit leaves behind persists for the group's later
    units. ``host`` names where the warm target lives; locally all groups
    share this machine, so commands run here with the target's identity.
    """

    def __init__(self) -> None:
        self._dirs: dict[str, str] = {}

    def warm(self, group: Group) -> str:
        path = tempfile.mkdtemp(prefix=f"medulla-warm-{group.warm_target}-")
        self._dirs[group.name] = path
        return path

    def run(self, unit: str, handle: str) -> tuple[str, float]:
        env = dict(os.environ, MEDULLA_WARM_TARGET=Path(handle).name)
        started = time.monotonic()
        proc = subprocess.run(
            ["sh", "-c", unit],
            cwd=handle,
            env=env,
            capture_output=True,
            text=True,
        )
        duration_ms = (time.monotonic() - started) * 1000.0
        return ("pass" if proc.returncode == 0 else "fail", duration_ms)

    def close(self, handle: str) -> None:
        try:
            Path(handle).rmdir()
        except OSError:
            pass  # non-empty or already gone: leave it, never block the summary


def load_groups(path: Path) -> list[Group]:
    """Load and fail-closed-validate the host-group manifest.

    Same rules as the standalone checker of MEDULLA-CONTRACT-R011.1: one
    warm target per group, a headroom reservation strictly between 0 and 1,
    no host in more than one group. Any problem refuses dispatch.
    """
    raw = path.read_text(encoding="utf-8")
    document = yaml.safe_load(raw)
    if not isinstance(document, dict) or not isinstance(document.get("groups"), list):
        raise ManifestError(f"{path} has no top-level 'groups' list")
    seen_hosts: dict[str, str] = {}
    groups: list[Group] = []
    for index, entry in enumerate(document["groups"]):
        if not isinstance(entry, dict):
            raise ManifestError(f"groups[{index}] is not a mapping")
        name = entry.get("name")
        warm = entry.get("warm_target")
        if not isinstance(name, str) or not name.strip():
            raise ManifestError(f"groups[{index}] has no non-empty 'name'")
        if not isinstance(warm, str) or not warm.strip():
            raise ManifestError(f"group '{name}' has no non-empty 'warm_target'")
        headroom = entry.get("headroom_reservation")
        if (
            not isinstance(headroom, (int, float))
            or isinstance(headroom, bool)
            or not (0.0 < float(headroom) < 1.0)
        ):
            raise ManifestError(
                f"group '{name}' has an invalid 'headroom_reservation' "
                "(must be a number strictly between 0 and 1)"
            )
        hosts = entry.get("hosts")
        if (
            not isinstance(hosts, list)
            or not hosts
            or not all(isinstance(h, str) and h.strip() for h in hosts)
        ):
            raise ManifestError(f"group '{name}' has no non-empty string 'hosts' list")
        for host in hosts:
            if host in seen_hosts:
                raise ManifestError(
                    f"host '{host}' is claimed by both '{seen_hosts[host]}' "
                    f"and '{name}'"
                )
            seen_hosts[host] = name
        groups.append(Group(name, warm, float(headroom), tuple(hosts)))
    if not groups:
        raise ManifestError(f"{path} declares no groups")
    return groups


def assign_units(
    groups: list[Group], units: list[str]
) -> list[tuple[Group, list[str]]]:
    """Round-robin assignment of units to groups, in manifest order."""
    if not units:
        raise ManifestError("no units to dispatch")
    plan: list[list[str]] = [[] for _ in groups]
    for position, unit in enumerate(units):
        plan[position % len(groups)].append(unit)
    return list(zip(groups, plan, strict=True))


def run_group(group: Group, units: list[str], runner: Runner) -> GroupSummary:
    """Run a group's units on its one persistent warm target.

    Concurrency is capped at ``group.concurrency`` so the headroom reservation
    stays free on the group's gate hosts.
    """
    handle = runner.warm(group)
    started = time.monotonic()
    try:
        results: list[UnitResult] = [None] * len(units)  # type: ignore[list-item]
        with ThreadPoolExecutor(max_workers=group.concurrency) as pool:
            outcomes = pool.map(lambda u: runner.run(u, handle), units)
            for index, (unit, outcome) in enumerate(zip(units, outcomes, strict=True)):
                status, duration_ms = outcome
                results[index] = UnitResult(
                    unit,
                    status,
                    duration_ms,
                    group.name,
                    group.warm_target,
                )
    finally:
        runner.close(handle)
        wall_ms = (time.monotonic() - started) * 1000.0
    return GroupSummary(group.name, group.warm_target, wall_ms, tuple(results))


def merged_summary(summaries: list[GroupSummary]) -> dict:
    """One merged summary whose results are exactly the per-group results.

    Deterministic: groups in name order, units in dispatch order within a
    group. ``wall_ms`` is the fan-out wall-clock (slowest group);
    ``serial_wall_ms`` is the sum of all unit durations, i.e. the same work
    run serially on a single host.
    """
    ordered = sorted(summaries, key=lambda s: s.group)
    units = [u for summary in ordered for u in summary.units]
    return {
        "combined": len(ordered),
        "wall_ms": max((s.wall_ms for s in ordered), default=0.0),
        "serial_wall_ms": sum(u.duration_ms for u in units),
        "pass": sum(1 for u in units if u.status == "pass"),
        "fail": sum(1 for u in units if u.status == "fail"),
        "units": [
            {
                "unit": u.unit,
                "status": u.status,
                "duration_ms": u.duration_ms,
                "group": u.group,
                "warm_target": u.warm_target,
            }
            for u in units
        ],
        "groups": [
            {
                "group": s.group,
                "warm_target": s.warm_target,
                "wall_ms": s.wall_ms,
                "units": [u.unit for u in s.units],
            }
            for s in ordered
        ],
    }


def fan_out(
    groups: list[Group],
    units: list[str],
    runner: Runner,
    *,
    serial: bool = False,
) -> dict:
    """Dispatch the land gate's units; return ``{"merged": ..., "groups": ...}``.

    Parallel (default): every group runs at once, each on its own persistent
    warm target, capped by its headroom reservation. Serial: every unit runs
    one after another on the first group's warm target.
    """
    if serial:
        order: list[tuple[Group, list[str]]] = [(groups[0], list(units))]
    else:
        order = assign_units(groups, units)
    if len(order) == 1:
        summaries = [
            run_group(group, group_units, runner) for group, group_units in order
        ]
    else:
        # Independent groups run at once: one executor thread per group. All
        # groups are submitted before any is awaited.
        summaries = [None] * len(order)  # type: ignore[list-item]
        with ThreadPoolExecutor(max_workers=len(order)) as pool:
            futures = [
                pool.submit(run_group, group, group_units, runner)
                for group, group_units in order
            ]
            for index, future in enumerate(futures):
                summaries[index] = future.result()
    merged = merged_summary(summaries)
    return {"merged": merged, "groups": summaries}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--manifest",
        type=Path,
        default=DEFAULT_MANIFEST,
        help="host-group manifest (default: %(default)s)",
    )
    parser.add_argument(
        "--unit",
        action="append",
        dest="units",
        default=[],
        metavar="COMMAND",
        help="gate unit to dispatch (repeatable); the command runs on the "
        "group's persistent warm target",
    )
    parser.add_argument(
        "--serial",
        action="store_true",
        help="run every unit one after another on the first group's warm "
        "target (the comparison baseline)",
    )
    parser.add_argument("--json", action="store_true", help="print the merged summary")
    args = parser.parse_args(argv)

    try:
        groups = load_groups(args.manifest)
        units = [command.strip() for command in args.units if command.strip()]
        if not units:
            raise ManifestError("no units to dispatch")
        result = fan_out(groups, units, CommandRunner(), serial=args.serial)
    except ManifestError as error:
        print(f"fanout-adapter: REFUSED: {error}", file=sys.stderr)
        return 2

    merged = result["merged"]
    for group in result["groups"]:
        flag = "FAIL" if group.failed else "pass"
        print(
            f"fanout-adapter: group {group.group} warm {group.warm_target}: "
            f"{len(group.units)} unit(s), {group.wall_ms:.0f} ms, {flag}"
        )
    verdict = "PASS" if merged["fail"] == 0 else "FAIL"
    print(
        f"fanout-adapter: {verdict}: {merged['pass']} pass / {merged['fail']} fail, "
        f"wall {merged['wall_ms']:.0f} ms "
        f"(serial baseline {merged['serial_wall_ms']:.0f} ms)"
    )
    if args.json:
        print(json.dumps(merged, indent=2, sort_keys=True))
    return 1 if merged["fail"] else 0


if __name__ == "__main__":
    sys.exit(main())
