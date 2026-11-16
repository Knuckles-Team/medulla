# MEDULLA-PLACEMENT — Research

Phase 0 unknowns and their resolution status.

## Production deployment template (Arbora, typed packs)

Unknown: the production deployment template and fleet-tool components do not exist in any Agent Library yet, so the assembled step-graph proof cannot run against production templates. Status: BLOCKED on the owner. The engine proof runs over a fixture template held in the tenant Agent Library (one template per target with the `render-artifact`, `apply` and `verify` slots and three tool components). Arbora publishes the production templates and components as a typed pack; the consumer link is ARBORA-DEPLOY-R017.

## Capacity adjustment ownership (decision engine)

Unknown: who owns the typed capacity adjustment proposal and verdict. Status: DECIDED. The question, verdict and the nine refusal codes live in `MEDULLA-DECISION-ENGINE-R824` under `MEDULLA-DECISION-ENGINE-R112`; model concurrency, circuit breaking and GPU group budgets live in `MEDULLA-DECISION-ENGINE-R825`. Arbora generates the AIMD proposal and actuates. This spec keeps one rule: the evaluator reuses the placement headroom premise and adds no second controller.

## Decision: Leader lease is a tenant-graph record

**Decision.** The leader lease is a tenant-graph lease record updated by compare-and-set on its revision (`MEDULLA-PLACEMENT-R751.761`), modelled as an explicit state machine Free, Held, Expired. Schedules are tenant-graph records. The task-kind to job-spec mapping is a closed table beside the schedule kinds (`MEDULLA-PLACEMENT-R751.762`).

## Decision: Schedule run policy is one decision function over one run record

**Decision.** Each due window is decided by one pure function, `decide_window` (`MEDULLA-PLACEMENT-R751.755`, `R751.756`, `R751.757`, `R751.760`), in a fixed order: paused, missed, budget, overlap, group active, group spacing. A skip is a final recorded outcome with a reason; nothing is queued. The tick task keeps one `ScheduleRun` record per schedule in the tenant graph (cursor, last fired window, active work item, budget usage of the period) and writes it in one batch with the tick node. Pause is the explicit `ScheduleMachine` (Enabled, Paused; Pause, Resume); the served methods answer a repeat idempotently before calling the machine, so the machine itself refuses the repeat. Resume records `resumed_ms` and every window before it is a paused window, so a window that passed while paused is never fired late. The budget is run seconds and tokens per budget period; a run reports usage through its completed result, and a run that reports none is charged its wall time in whole seconds, at least one. Group membership is the tenant's schedules that name the same group; the group check reads the other members' run records and work items, so no new store is needed. A scheduled task kind may require a worker capability (`association.remote`), which lets a run stay active until a worker takes it.

**Why.** One function with one input record keeps the rules testable without a server and keeps every skip reason in one place; the run record carries the only mutable schedule state outside the owner-written `Schedule` node, so the owner's updates and the tick never write the same node.
## Placement decide wiring (MEDULLA-PLACEMENT-R001.7)
Decision: the placement question is a typed `PlacementQuestion` parameter (graph, body, engine-filled `read`), the same shape as the standalone topology question, so the facts and headroom the decision used are recorded in its inputs and replay reads the same values. Policy caps are `placement.*` policy knobs (the policy digest is already in the record), not a policy node. Capacity headroom is per target and dimension (`placement:<target>:cpu`, `placement:<target>:memory`) in the existing capacity ledger, which gains the `memory` resource class; no second ledger and no capacity in the target facts. A target with no cell named in the request has zero headroom and is eliminated as `capacity:headroom<demand`.

## Move and shard placement machines (R783.7, R783.11)
Decision: `MovePhaseMachine` and `ShardPlacementMachine` live beside their enums in `cross_node_elasticity/`; each holder has one writer (`MoveCheckpoint::advance`, `ShardPlacement::transition`). The census writer lists named `drain.rs` (writes `DrainPhase`) and `Cluster.kill`/`restart` (writes the harness member state); those are other types and are not routed (SM-CENSUS-WRITERS). A move's entry state follows its kind (`MovePhase::entry_for`), so `MoveCheckpoint::new` takes no phase.

## Schedule machine location (R785.1)
Decision: the schedule machine moves to `medulla-types::schedule_machine` so the stored `ScheduleRecord` and the served `ScheduleView` carry one closed enum; `medulla-protocol-types` cannot depend on `medulla-jobs`. Wire text is unchanged (`enabled`/`paused`); the contract schema now names the enum (WIRE-REGEN needed for the schema vectors).

## Control lease mirror (R785.2)
Decision: `ControlLeaseStatus` in the hand-maintained `medulla/generated/models.py` becomes a `str` enum of the four Rust variants (WIRE-REGEN needed).

## Placement abstentions (R001.10)
Decision: a cost cap above the pinned `placement.max_relative_cost` abstains `PolicyLoosening{field: max_relative_cost}` instead of failing the request; a target with no declared facts is `UnknownFact` (never guessed); every other elimination is one `Infeasible` constraint, a short capacity cell named `lease:<cell>:<class>` (the same name the headroom premise carries). The family's own codes (`PLACEMENT_FACTS_INVALID`, `CAPACITY_UNAVAILABLE`) live in the decision-types placement module so `codes::resolve` knows them.

## Placement plan scale (R700.2)
Decision: the exact solver holds at most 4,096 binaries, so the plan is solved by an exact decomposition, not by a heuristic: services that can all stay on their declared, capable current hosts stay (the move level is then at its lower bound and the host level cannot fall by moving them), and only the rest enter the 0-1 program. A larger program, or an exhausted node budget, abstains `BudgetExhausted`. The node budget is the decision-policy knob `placement.node_budget`.
## Stored row revisions without a table change (R780.3, R781.4)

Decision: a change to the stored shape of a row in an existing table is a logical schema revision of that table (`eg.logical.<table>.v<n>`), which is already part of the layout digest. The previous digest is declared as a lineage predecessor with the same table set; the offline upgrade rewrites the rows in the commit that replaces the manifest. No migration runs at open, and no compatibility reader exists. Why: the only other way to move the digest is to invent a table, and the registry, the refusal code, the operator document and `store-upgrade` already follow a declared predecessor.
## Placement why-not (MEDULLA-PLACEMENT-R001.9)
Decision: the why-not of a placement answer is the sealed `WhyNot` of the decision record, carried as `FamilyAnswer::Placement(PlacementAnswer { why_not })` in the record's one family answer, so no second why-not type exists. `PlacementVerdict::why_not` builds it: an eliminated target carries `Violation::Placement { reason }` with the stable `as_why_not` string; a feasible but costlier target carries `forced_objective` (its relative cost as the one objective level) and the reason `costlier than <chosen> (<cost> > <best>)`. The chosen target has no entry. An abstained answer carries none, because its `Infeasible` reasons already name every target. The new `Violation` variant is a wire change: the Python generated models and wire vectors need the WIRE-REGEN refresh (the Python `Violation` union already lacks `PrimitiveContract`).
