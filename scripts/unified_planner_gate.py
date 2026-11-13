"""Judge the unified-planner falsification gate (MEDULLA-FEDERATED-QUERY-R730).

``benches/unified_vs_contracts.rs`` writes a report (``arm_a_p95_ms``,
``arm_b_p95_ms``, ``arm_a_recall``, ``arm_b_recall``, ``writes_lost``,
``max_p95_ratio``). This script turns it into a verdict and records the verdict in
the engine as a ``PlannerGateRun`` node. The scope of the planner is one explicit
state machine, ``PlannerScopeMachine`` (``state-machines-v1.ttl``): ``clear`` and
``frozen``, moved only by :func:`transition` on the events ``gate_failed`` and
``gate_passed``. The state is the one ``state`` property of the latest run node; the
freeze decision is the node whose ``decision`` is ``PLANNER_SCOPE_FROZEN``. While the
state is ``frozen`` no new R720 or R721 operator may land (:func:`landing_refusal`).
"""

from __future__ import annotations

import argparse
import json
import sys
from dataclasses import dataclass
from enum import Enum
from typing import Any

DECISION_CODE = "PLANNER_SCOPE_FROZEN"
RUN_LABEL = "PlannerGateRun"
NODE_PREFIX = "planner-gate-run-"
REQUIRED_FIELDS = (
    "arm_a_p95_ms",
    "arm_b_p95_ms",
    "arm_a_recall",
    "arm_b_recall",
    "writes_lost",
    "max_p95_ratio",
)


class PlannerScope(str, Enum):
    """The closed set of states of the planner scope machine."""

    CLEAR = "clear"
    FROZEN = "frozen"


class GateEvent(str, Enum):
    """The closed set of events of the planner scope machine."""

    GATE_FAILED = "gate_failed"
    GATE_PASSED = "gate_passed"


class IllegalTransition(ValueError):
    """A typed refusal naming the machine, the current state and the event."""

    def __init__(self, state: object, event: object) -> None:
        super().__init__(
            f"PLANNER_SCOPE_ILLEGAL_TRANSITION: machine PlannerScopeMachine, "
            f"state {state!r}, event {event!r}"
        )


_TRANSITIONS: dict[tuple[PlannerScope, GateEvent], PlannerScope] = {
    (PlannerScope.CLEAR, GateEvent.GATE_FAILED): PlannerScope.FROZEN,
    (PlannerScope.FROZEN, GateEvent.GATE_FAILED): PlannerScope.FROZEN,
    (PlannerScope.FROZEN, GateEvent.GATE_PASSED): PlannerScope.CLEAR,
    (PlannerScope.CLEAR, GateEvent.GATE_PASSED): PlannerScope.CLEAR,
}


def transition(state: PlannerScope, event: GateEvent) -> PlannerScope:
    """The only writer of the planner scope state."""
    try:
        return _TRANSITIONS[(PlannerScope(state), GateEvent(event))]
    except (KeyError, ValueError):
        raise IllegalTransition(state, event) from None


@dataclass(frozen=True)
class Verdict:
    """The gate's judgement of one report."""

    passed: bool
    reasons: tuple[str, ...]


def evaluate(report: dict[str, Any]) -> Verdict:
    """Arm A passes: p95 within the ratio of arm B, equal recall, no write lost."""
    missing = [name for name in REQUIRED_FIELDS if name not in report]
    if missing:
        raise ValueError(f"PLANNER_GATE_REPORT_INCOMPLETE: missing {missing}")
    reasons = []
    ceiling = report["max_p95_ratio"] * report["arm_b_p95_ms"]
    if report["arm_a_p95_ms"] > ceiling:
        reasons.append(
            f"p95 {report['arm_a_p95_ms']:.3f} ms exceeds {report['max_p95_ratio']} x "
            f"{report['arm_b_p95_ms']:.3f} ms"
        )
    if report["arm_a_recall"] != report["arm_b_recall"]:
        reasons.append(
            f"recall differs: {report['arm_a_recall']} against {report['arm_b_recall']}"
        )
    if report["writes_lost"] != 0:
        reasons.append(f"{report['writes_lost']} writes lost")
    return Verdict(passed=not reasons, reasons=tuple(reasons))


def _ids(client: Any, state: str) -> list[str]:
    result = client.query.uql(
        f"MATCH (:{RUN_LABEL}) WHERE state = '{state}' |> LIMIT 1000000"
    )
    return [row["id"] for row in result["rows"]]


def current_state(client: Any) -> PlannerScope:
    """The state of the latest recorded run; ``clear`` before any run."""
    latest = {s: max(_ids(client, s.value), default="") for s in PlannerScope}
    newest = max(PlannerScope, key=lambda s: latest[s])
    return newest if latest[newest] else PlannerScope.CLEAR


def record(client: Any, report: dict[str, Any]) -> PlannerScope:
    """Judge ``report``, move the machine, persist the run and any freeze decision."""
    verdict = evaluate(report)
    event = GateEvent.GATE_PASSED if verdict.passed else GateEvent.GATE_FAILED
    state = transition(current_state(client), event)
    sequence = len(_ids(client, PlannerScope.CLEAR.value)) + len(
        _ids(client, PlannerScope.FROZEN.value)
    )
    properties: dict[str, Any] = {
        "type": RUN_LABEL,
        "state": state.value,
        "event": event.value,
        "report": json.dumps(report, sort_keys=True),
        "reasons": "; ".join(verdict.reasons),
    }
    if not verdict.passed:
        properties["decision"] = DECISION_CODE
    client.nodes.add(f"{NODE_PREFIX}{sequence + 1:09d}", properties)
    return state


def landing_refusal(client: Any, operator: str) -> str | None:
    """Refuse a new R720 or R721 operator while the freeze decision stands."""
    if current_state(client) is PlannerScope.FROZEN:
        return f"{DECISION_CODE}: no new planner operator may land ({operator})"
    return None


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("report", help="path of the bench report JSON")
    args = parser.parse_args(argv)
    with open(args.report, encoding="utf-8") as handle:
        verdict = evaluate(json.load(handle))
    for reason in verdict.reasons:
        print(f"{DECISION_CODE}: {reason}", file=sys.stderr)
    return 0 if verdict.passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
