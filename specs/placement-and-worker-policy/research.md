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
