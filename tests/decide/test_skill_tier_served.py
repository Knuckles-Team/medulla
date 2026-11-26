"""MEDULLA-DECISION-ENGINE-R714.6 served skill tiers.

L4: every call goes through the running engine over the wire client. A skill is
promoted through the ``promotion`` ``Decide`` question on paired rollouts published as
a ``RolloutSet`` (``learn`` write ``record_rollouts``); each winning record is committed
and the champion pointer moved, which stores the tier it carries.

* ``Domain`` needs a reviewer from the owning team: an approver outside the team is
  refused ``TIER_REVIEW_MISSING``, a member is admitted.
* ``Core`` needs the platform team and a healthy suite report of enough cases. The
  report is written by the served ``skill_eval`` job: a suite of 5 cases is refused
  ``PROMOTION_EVAL_TOO_SMALL``, a suite of 20 admits the promotion.
* ``AlwaysOn`` is admitted and sealed with its ``tier:always_on=<n>/100`` premise.
* A served ``load_set`` question for a persona lists the always-on skills first, then
  the persona's skills with the tiers they hold.

The count boundary of 99 and 100 always-on skills needs a tenant with a known number
of them; it is proven at L3 by
``crates/medulla-h-decide/src/decide/stat_tests/skill_tier_tests.rs``.
"""

from __future__ import annotations

import asyncio
import json
import os
import re
import sys
import uuid
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import pytest
from _commons_generation import publish_schema
from conftest import TEST_TENANT, request_context

from medulla import decision_client as dc
from medulla.client import MedullaClient

TRAJECTORY = [{"kind": "tool", "name": "search"}]
ROLLOUTS = 60
GAIN_PPM = 150_000


def _subject(skill: str, tier: str | None) -> dict[str, Any]:
    subject: dict[str, Any] = {"kind": "skill", "skill_ref": skill}
    if tier is not None:
        subject["tier"] = tier
    return subject


def _rollouts(skill: str, challenger: str, champion: str) -> dict[str, Any]:
    return dc.rollout_set(
        skill,
        challenger,
        champion,
        (
            {
                "seed": index % 3,
                "task_id": f"task-{index}",
                "challenger_score_ppm": 600_000 + GAIN_PPM,
                "champion_score_ppm": 600_000,
                "trace_ref": f"trace-{index}",
            }
            for index in range(ROLLOUTS)
        ),
    )


def _review(tier: str, team: str, approver: str) -> dict[str, Any]:
    return {"tier": tier, "reviewer_team": team, "approver_principal": approver}


def _suite(skill: str, cases: int) -> list[dict[str, Any]]:
    suite = []
    for index in range(cases):
        kind = (
            "golden"
            if index == 0
            else "real_prompt"
            if index < cases - 3 or cases < 20
            else "author_judged"
        )
        suite.append(
            {
                "case_id": f"{skill}-case-{index}",
                "skill_ref": skill,
                "kind": kind,
                "input_digest": f"in-{index}",
                "expected_digest": f"out-{index}",
                "expected_trajectory": TRAJECTORY if kind == "real_prompt" else [],
                "source": "authored",
            }
        )
    return suite


def _eval_request(skill: str, cases: int, failing: int) -> dict[str, Any]:
    suite = _suite(skill, cases)
    runs = []
    for index, case in enumerate(suite):
        ok = index >= failing
        runs.append(
            {
                "case_id": case["case_id"],
                "actual_digest": f"out-{index}" if ok else "wrong",
                "actual_trajectory": TRAJECTORY if ok else [],
                "judged_passed": ok,
                "attestation": {
                    "sandbox_digest": "sandbox",
                    "env_digest": "env",
                    "host_paths_read": 0,
                },
                "run_trace_id": f"trace-{index}",
            }
        )
    return {
        "tenant_id": TEST_TENANT,
        "skill_ref": skill,
        "cases": suite,
        "runs": runs,
        "calibration": {
            "judge_ref": "judge-1",
            "annotated_set_ref": "set-1",
            "kappa_ppm": 700_000,
            "n": 60,
        },
        "known_tools": ["search"],
    }


def _node(node_id: str, node_type: str) -> dict[str, Any]:
    return {"op": "upsert_node", "id": node_id, "properties": {"type": node_type}}


def _edge(source: str, target: str, relationship: str) -> dict[str, Any]:
    return {
        "op": "upsert_edge",
        "source": source,
        "target": target,
        "properties": {"relationship": relationship},
    }


class _Journey:
    def __init__(
        self,
        decisions: dc.DecisionClient,
        schema: dict[str, Any],
        graph: str,
    ) -> None:
        self.decisions = decisions
        self.schema = schema
        self.graph = graph

    async def ask(
        self,
        skill: str,
        tier: str,
        challenger: str,
        champion: str,
        review: dict[str, Any] | None,
    ) -> dict[str, Any]:
        recorded = await self.decisions.log(
            dc.record_rollouts_op(TEST_TENANT, _rollouts(skill, challenger, champion))
        )
        request = dc.promotion_request(
            TEST_TENANT,
            f"tier-{uuid.uuid4().hex[:8]}",
            _subject(skill, tier),
            recorded["receipt_digest"],
            challenger,
            self.schema,
            graph=self.graph,
            tier_review=review,
        )
        return (await self.decisions.decide(request))["records"][0]

    async def promote(
        self,
        skill: str,
        tier: str,
        challenger: str,
        champion: str,
        review: dict[str, Any],
    ) -> dict[str, Any]:
        record = await self.ask(skill, tier, challenger, champion, review)
        assert record["outcome"]["outcome"] != "abstained", record["outcome"]
        await self.decisions.log(dc.commit_op(record))
        moved = await self.decisions.log(
            dc.activate_champion_op(
                TEST_TENANT, _subject(skill, None), challenger, record["record_id"]
            )
        )
        assert moved["recorded"] == "pointer", moved
        return record


def _facts(record: dict[str, Any]) -> str:
    return json.dumps(record["premises"])


async def _journey(prefix: str) -> dict[str, Any]:
    context = request_context()
    admin = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"], verified_context=context
    )
    graph = f"tier-{prefix}"
    try:
        schema = await publish_schema(admin)
        await admin.tenants.create(graph)
    finally:
        await admin.close()
    client = await MedullaClient.connect(
        socket_path=os.environ["GRAPH_SERVICE_SOCKET"],
        verified_context=context,
        graph_name=graph,
    )
    try:
        decisions = dc.DecisionClient(client=client)
        journey = _Journey(decisions, schema, graph)
        domain_skill = f"skill:{prefix}-greet@1"
        core_skill = f"skill:{prefix}-refund@1"
        always_on_skill = f"skill:{prefix}-triage@3"
        ana, root, stranger = (
            f"principal:{prefix}-{n}" for n in ("ana", "root", "eve")
        )
        support, platform, persona = (
            f"team:{prefix}-support",
            f"team:{prefix}-platform",
            f"persona:{prefix}-support",
        )
        await client.lifecycle.batch_update(
            [
                *(
                    _node(node, "Entity")
                    for node in (ana, root, stranger, support, platform)
                ),
                _node(persona, "Persona"),
                *(
                    _node(s, "Skill")
                    for s in (domain_skill, core_skill, always_on_skill)
                ),
                _edge(ana, support, "memberOf"),
                _edge(root, platform, "memberOf"),
                _edge(persona, domain_skill, "includes"),
                _edge(persona, core_skill, "includes"),
            ]
        )

        out: dict[str, Any] = {}

        # Domain: an approver outside the team lacks the receipt; a member qualifies.
        outsider = await journey.ask(
            domain_skill,
            "domain",
            "revision:one",
            "revision:zero",
            _review("domain", support, stranger),
        )
        out["outsider"] = json.dumps(outsider["outcome"])
        out["domain"] = await journey.promote(
            domain_skill,
            "domain",
            "revision:one",
            "revision:zero",
            _review("domain", support, ana),
        )

        # Core: no suite report, then 5 cases, then 20 healthy ones.
        core_review = _review("core", platform, root)
        out["no_report"] = json.dumps(
            (
                await journey.ask(
                    core_skill, "core", "revision:one", "revision:zero", core_review
                )
            )["outcome"]
        )
        await decisions.skill_eval(_eval_request(core_skill, 5, 1))
        out["small"] = json.dumps(
            (
                await journey.ask(
                    core_skill, "core", "revision:one", "revision:zero", core_review
                )
            )["outcome"]
        )
        report = await decisions.skill_eval(_eval_request(core_skill, 20, 4))
        out["report"] = report["report"]
        out["core"] = await journey.promote(
            core_skill, "core", "revision:one", "revision:zero", core_review
        )

        # AlwaysOn.
        out["always_on"] = await journey.promote(
            always_on_skill,
            "always_on",
            "revision:one",
            "revision:zero",
            _review("always_on", platform, root),
        )

        # The load set of the persona: the always-on skills first.
        question = {
            "job_text": "look up an order and close the ticket",
            "max_tools": 64,
            "max_agents": 1,
            "graph": graph,
            "persona": persona,
        }
        request = dc.load_set_request(
            TEST_TENANT, f"tier-load-{prefix}", question, schema
        )
        loaded = (await decisions.decide(request))["records"][0]
        out["tiered"] = [
            (entry["skill_ref"], entry.get("tier"))
            for entry in loaded["answer"]["value"]["tiered_skills"]
        ]
        out["load_facts"] = _facts(loaded)
        out["skills"] = {
            "domain": domain_skill,
            "core": core_skill,
            "always_on": always_on_skill,
        }
        return out
    finally:
        await client.close()


# spec: MEDULLA-DECISION-ENGINE-R714
@pytest.mark.spec("MEDULLA-DECISION-ENGINE-R714.6")
def test_a_skill_climbs_tiers_and_the_load_set_follows(clean_graph) -> None:
    # spec: MEDULLA-DECISION-ENGINE-R714.6
    out = asyncio.run(_journey(uuid.uuid4().hex[:8]))
    skills = out["skills"]

    assert "TIER_REVIEW_MISSING" in out["outsider"], out["outsider"]
    assert "tier:target=domain" in _facts(out["domain"])

    assert "TIER_REVIEW_MISSING" in out["no_report"], out["no_report"]
    assert "PROMOTION_EVAL_TOO_SMALL" in out["small"], out["small"]
    assert out["report"]["band"] == "healthy", out["report"]
    assert out["report"]["case_count"] == 20, out["report"]
    assert "tier:target=core" in _facts(out["core"])

    assert "tier:target=always_on" in _facts(out["always_on"])
    sealed = re.search(r"tier:always_on=(\d+)/100", _facts(out["always_on"]))
    assert sealed is not None, _facts(out["always_on"])

    tiered = out["tiered"]
    refs = [ref for ref, _ in tiered]
    held = dict(tiered)
    assert held[skills["always_on"]] == "always_on", tiered
    assert held[skills["domain"]] == "domain", tiered
    assert held[skills["core"]] == "core", tiered
    first_other = min(refs.index(skills["domain"]), refs.index(skills["core"]))
    assert all(tier == "always_on" for _, tier in tiered[:first_other]), tiered
    assert refs.index(skills["always_on"]) < first_other, tiered
    assert re.search(r"tier:always_on=\d+/100", out["load_facts"]), out["load_facts"]
