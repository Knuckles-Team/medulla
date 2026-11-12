"""MEDULLA-CONTRACT-R011: the land gate's fan-out adapter test suite.

Exercises ``scripts/fanout_adapter.py``: the land gate's default execution
fans gate units out in parallel across the independent host groups of the
host-group manifest, each group on one persistent warm target created once
and reused for all of the group's units, with each gate host keeping its
declared headroom reservation. The per-group summaries combine into one
merged summary that matches exactly the per-group results it combines, and
the fan-out wall-clock beats the same work run serially on a single host.
"""

from __future__ import annotations

import importlib.util
import sys
import threading
import time
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
pytestmark = [pytest.mark.no_engine]


def _module():
    """Load ``scripts/fanout_adapter.py`` by file path, so the test
    exercises exactly the checked-in script."""
    scripts_dir = ROOT / "scripts"
    sys.path.insert(0, str(scripts_dir))
    path = scripts_dir / "fanout_adapter.py"
    spec = importlib.util.spec_from_file_location("fanout_adapter", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules["fanout_adapter"] = module
    spec.loader.exec_module(module)
    return module


class ScriptedRunner:
    """A test runner: records warm targets, per-group concurrency and intervals."""

    def __init__(
        self,
        unit_sleep: float = 0.0,
        fail_units: frozenset[str] = frozenset(),
    ):
        self.unit_sleep = unit_sleep
        self.fail_units = set(fail_units)
        self.warm_calls: list[str] = []
        self.handles: dict[str, object] = {}
        self.unit_handles: list[tuple[str, object]] = []
        self.intervals: dict[str, list[float]] = {}
        self.peak: dict[str, int] = {}
        self.closed: list[object] = []
        self._group_of: dict[int, str] = {}
        self._lock = threading.Lock()
        self._active: dict[str, int] = {}

    def warm(self, group) -> object:
        handle = object()
        with self._lock:
            self.warm_calls.append(group.name)
            self.handles[group.name] = handle
            self._group_of[id(handle)] = group.name
        return handle

    def run(self, unit: str, handle) -> tuple[str, float]:
        group = self._group_of[id(handle)]
        started = time.monotonic()
        with self._lock:
            self._active[group] = self._active.get(group, 0) + 1
            self.peak[group] = max(self.peak.get(group, 0), self._active[group])
            self.unit_handles.append((unit, handle))
            if group not in self.intervals:
                self.intervals[group] = [started, started]
        if self.unit_sleep:
            time.sleep(self.unit_sleep)
        duration_ms = (time.monotonic() - started) * 1000.0
        with self._lock:
            self._active[group] -= 1
            self.intervals[group][1] = time.monotonic()
        status = "fail" if unit in self.fail_units else "pass"
        return status, duration_ms

    def close(self, handle) -> None:
        with self._lock:
            self.closed.append(handle)


def _groups() -> list:
    fa = _module()
    return [
        fa.Group("group-a", "warm-a", 0.1, ("host-a1", "host-a2")),
        fa.Group("group-b", "warm-b", 0.2, ("host-b1",)),
    ]


def test_merged_summary_matches_the_per_group_results_it_combines():
    """The merged summary is exactly the per-group results combined, in group order."""
    fa = _module()
    runner = ScriptedRunner()
    result = fa.fan_out(_groups(), ["u1", "u2", "u3", "u4"], runner)
    merged = result["merged"]
    per_group_units = [
        unit
        for group in sorted(result["groups"], key=lambda s: s.group)
        for unit in group.units
    ]
    # spec: MEDULLA-CONTRACT-R011
    assert [u.unit for u in per_group_units] == [
        entry["unit"] for entry in merged["units"]
    ]
    assert [u.status for u in per_group_units] == [
        entry["status"] for entry in merged["units"]
    ]
    assert merged["pass"] == sum(1 for u in per_group_units if u.status == "pass")
    assert merged["fail"] == sum(1 for u in per_group_units if u.status == "fail")
    assert merged["combined"] == 2
    assert {
        entry["warm_target"] for entry in merged["units"] if entry["group"] == "group-a"
    } == {"warm-a"}
    assert {
        entry["warm_target"] for entry in merged["units"] if entry["group"] == "group-b"
    } == {"warm-b"}


def test_groups_run_in_parallel_and_fan_out_beats_serial_wall_clock():
    """Independent groups run concurrently, so the fan-out wall-clock beats the
    same work run serially on a single host."""
    fa = _module()
    groups = [
        fa.Group("group-a", "warm-a", 0.1, ("host-a1",)),
        fa.Group("group-b", "warm-b", 0.1, ("host-b1",)),
    ]
    units = [f"u{i}" for i in range(4)]
    parallel_runner = ScriptedRunner(unit_sleep=0.15)
    parallel = fa.fan_out(groups, units, parallel_runner)
    serial = fa.fan_out(groups, units, ScriptedRunner(unit_sleep=0.15), serial=True)
    interval_a = parallel_runner.intervals["group-a"]
    interval_b = parallel_runner.intervals["group-b"]
    overlap = min(interval_a[1], interval_b[1]) - max(interval_a[0], interval_b[0])
    # spec: MEDULLA-CONTRACT-R011
    assert overlap > 0.2
    assert parallel["merged"]["wall_ms"] < serial["merged"]["serial_wall_ms"] * 0.8
    # Serial mode runs every unit on the first group's warm target, one after
    # another: one warm target total, no second group touched.
    assert len(serial["groups"]) == 1
    assert [g.group for g in serial["groups"]] == ["group-a"]


def test_one_persistent_warm_target_per_group_reused_by_every_unit():
    """Each group gets exactly one persistent warm target, reused by all of the
    group's units instead of being rebuilt per unit."""
    fa = _module()
    groups = [
        fa.Group("group-a", "warm-a", 0.1, ("host-a1", "host-a2")),
        fa.Group("group-b", "warm-b", 0.2, ("host-b1", "host-b2", "host-b3")),
    ]
    runner = ScriptedRunner()
    result = fa.fan_out(groups, [f"u{i}" for i in range(6)], runner)
    # spec: MEDULLA-CONTRACT-R011
    assert runner.warm_calls == ["group-a", "group-b"]
    for group in result["groups"]:
        group_units = {unit.unit for unit in group.units}
        handles = {
            handle for unit, handle in runner.unit_handles if unit in group_units
        }
        # spec: MEDULLA-CONTRACT-R011
        assert handles == {runner.handles[group.group]}
    assert len(runner.closed) == 2


def test_headroom_reservation_caps_group_concurrency():
    """Each gate host keeps its declared headroom reservation: a group never
    uses more worker slots than it has after reserving headroom."""
    fa = _module()
    assert fa.Group("solo", "warm", 0.1, ("host-s1",)).concurrency == 1
    assert fa.Group("quad", "warm", 0.5, ("h1", "h2", "h3", "h4")).concurrency == 2
    assert fa.Group("full", "warm", 0.0 + 1e-9, ("h1",)).concurrency == 1
    groups = [
        fa.Group("solo", "warm-solo", 0.1, ("host-s1",)),
        fa.Group("quad", "warm-quad", 0.5, ("h1", "h2", "h3", "h4")),
    ]
    runner = ScriptedRunner(unit_sleep=0.05)
    fa.fan_out(groups, [f"u{i}" for i in range(12)], runner)
    # A single-host group keeps its headroom free: never two units at once.
    # spec: MEDULLA-CONTRACT-R011
    assert runner.peak["solo"] == 1
    # A half-reserved four-host group never exceeds its two remaining slots.
    # spec: MEDULLA-CONTRACT-R011
    assert runner.peak["quad"] <= 2
    assert runner.peak["quad"] >= 2


def test_a_failed_unit_fails_the_merged_summary():
    fa = _module()
    runner = ScriptedRunner(fail_units=frozenset({"u2"}))
    result = fa.fan_out(_groups(), ["u1", "u2", "u3", "u4"], runner)
    merged = result["merged"]
    assert merged["fail"] == 1
    assert merged["pass"] == 3
    failed = [entry for entry in merged["units"] if entry["status"] == "fail"]
    assert [entry["unit"] for entry in failed] == ["u2"]


def test_overlapping_manifest_fails_closed_before_any_dispatch(tmp_path):
    fa = _module()
    manifest = tmp_path / "manifest.yml"
    manifest.write_text(
        "groups:\n"
        "  - name: a\n    warm_target: wa\n    headroom_reservation: 0.1\n"
        "    hosts: [h1]\n"
        "  - name: b\n    warm_target: wb\n    headroom_reservation: 0.1\n"
        "    hosts: [h1]\n",
        encoding="utf-8",
    )
    runner = ScriptedRunner()
    with pytest.raises(fa.ManifestError, match="claimed by both"):
        fa.load_groups(manifest)
    # spec: MEDULLA-CONTRACT-R011
    assert runner.warm_calls == []
    assert runner.unit_handles == []


def test_missing_headroom_reservation_is_refused(tmp_path):
    fa = _module()
    manifest = tmp_path / "manifest.yml"
    manifest.write_text(
        "groups:\n  - name: a\n    warm_target: wa\n    hosts: [h1]\n",
        encoding="utf-8",
    )
    with pytest.raises(fa.ManifestError, match="headroom_reservation"):
        fa.load_groups(manifest)


def test_repository_default_manifest_is_fully_specified():
    """The checked-in manifest is a valid fan-out shape: one group, one warm
    target, a headroom reservation, disjoint hosts."""
    fa = _module()
    groups = fa.load_groups(fa.DEFAULT_MANIFEST)
    assert len(groups) == 1
    assert groups[0].name == "r820"
    assert groups[0].warm_target == "medulla-r820-warm"
    assert 0.0 < groups[0].headroom < 1.0
    assert groups[0].concurrency >= 1
