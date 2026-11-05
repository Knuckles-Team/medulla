# MEDULLA-PLACEMENT — Placement, worker policy and analytics bound

Owner: Medulla. Cross-repo IDs: ARBORA-DEPLOY-R017 (Arbora), MEDULLA-DECISION-ENGINE-R036, R046, R111, MEDULLA-DURABLE-KERNEL-R074 to R077, R082.3, MEDULLA-INGEST-WORKER-R001.
Public provenance: closes three engine-side design gaps so each keeps one decision authority.

## Purpose and user stories

### User Story 1 - An operator gets one placement decision with evidence (Priority: P1)

An operator asks "where should this run?" and gets one engine decision (target, evidence, why-not, or typed abstention) plus an assembled, certificate-backed step graph; Arbora adds no planner.

### User Story 2 - An operator runs ingest workers that obey the engine (Priority: P1)

An operator runs ingest workers that register and obey the engine; no worker-side policy can disagree with the engine's.

### User Story 3 - A caller submits an association-mining job with a documented bound (Priority: P2)

A caller submits an association-mining job and gets a deterministic, documented bound instead of an unexplained refusal.

### User Story 4 - Jobs, claims and clients follow engine routing (Priority: P2)

An operator runs heavy jobs, scheduled work and tenant clients that all follow the engine's routing: a job lands on a host that declares the capability, background claims pause under foreground load, a schedule tick runs once on the leader, and a client opens a tenant graph only with an engine route.

## Acceptance scenarios

Given a served `Decide` placement question with candidate targets and headroom facts, when the question is evaluated, then the cheapest feasible target returns as a typed `Advisory` record with premises and one why-not per non-chosen target, and no lease is taken.
Given a placement demand above every cell headroom, when the question is evaluated, then the record abstains with the lease-named reason and the capacity ledger is unchanged.
Given an association job with more than 31 distinct labels, when it is submitted, then the engine refuses with the distinct-item limit message and stores nothing.
Given a `Backtest` job and two hosts where only one declares the capability, when workers claim, then only the declaring host's worker receives the job, and with no declaring host the claim refuses `NO_CAPABLE_HOST` and the job stays queued.
Given foreground load, when a background claim arrives, then the pacer returns `CLAIM_PACED` and leaves the job queued; given two engines and one schedule window, when the tick fires, then only the leader submits the job once.
Given a three-host inventory and five services with one service needing a capability no host declares, when a placement plan is requested, then the decision abstains naming the capability; after a host declares it, the plan solves with the fewest moves.

## Requirements and acceptance

- FR-001: A `Placement` Decide question over in_process/compose/helm/swarm returns a typed record or typed abstention. It never takes a lease (MEDULLA-PLACEMENT-R001).
- FR-002: `AgentAssemble` returns an ordered deployment step graph from a published template and abstains rather than invents steps (MEDULLA-PLACEMENT-R001).
- FR-003: Autosize, lane floors, widths, routing and queue-depth limit are served engine settings; the worker holds none (MEDULLA-PLACEMENT-R002).
- FR-004: `MineAssociate` refuses more than 31 distinct items with its own typed refusal, and the kernel cannot overflow (MEDULLA-PLACEMENT-R003).
- FR-005: A heavy job is claimed only by a worker whose host declares the required capability (MEDULLA-PLACEMENT-R004).
- FR-006: The tenant client follows the engine's placement route and opens a graph only with one (MEDULLA-PLACEMENT-R611).
- FR-007: Claims are paced by priority class and schedule ticks run once per window under a fenced leader (MEDULLA-PLACEMENT-R750, MEDULLA-PLACEMENT-R751).
- FR-008: A placement plan assigns services to hosts within capacity or abstains with a typed reason (MEDULLA-PLACEMENT-R700).
- SC-001: The seven overlapping ingest-worker rows retire to the ingest-worker spec and no duplicate policy constant remains (MEDULLA-PLACEMENT-R002.6, MEDULLA-PLACEMENT-R002.7, MEDULLA-PLACEMENT-R002.8, MEDULLA-PLACEMENT-R002.9, MEDULLA-PLACEMENT-R002.10, MEDULLA-PLACEMENT-R002.11, MEDULLA-PLACEMENT-R002.12, MEDULLA-PLACEMENT-R002.15).

## Scope and interfaces

Owned: `QuestionKind::Placement`, placement vocabulary, `stat_placement.rs`, proposed edits to durable-kernel R074-R076 and ingest-worker rows, and the distinct-item bound in `crates/medulla-h-jobs/src/jobs/submit.rs`. Not owned: manifests and fleet-tool components (Arbora), the worker runner (connector Graft SDK), R111.1 (decision-engine spec).

## Edge Cases

- A placement request that loosens a policy cap is refused with `POLICY_LOOSENING`; no target is guessed.
- A claim request with a forged lane or partition field fails schema validation with `INVALID_ARGUMENT`.
- A worker with zero available memory, a zero request ceiling or zero writer width gets a zero-slot grant with a typed capacity refusal; the floor of one never overrides a hard limit.
- Thirty-one distinct labels are accepted, thirty-two are refused, and a repeated label counts once after hashing.
- When no host declares the required capability, the job stays queued and the claim refuses `NO_CAPABLE_HOST`; it never falls back to another host.

## Assumptions

- Arbora publishes the production deployment template and fleet-tool components as a typed pack; until then the assembly proof runs over a fixture template (Open questions below).
- `MEDULLA-DURABLE-KERNEL-R074` to `MEDULLA-DURABLE-KERNEL-R077` are absent on main today; the ingest-worker rows are the consumer proof for them.
- Capacity adjustment and model capacity are decision-engine questions under `MEDULLA-DECISION-ENGINE-R112`; this spec keeps only the placement headroom premise and claim pacing.

## Traceability

| Requirement | Design section | Test ID | Evidence |
|---|---|---|---|
| FR-001 | plan.md A | T-001, T-002 | PENDING |
| FR-002 | plan.md A | T-003 | PENDING |
| FR-003 | plan.md B | T-004 | PENDING |
| FR-004 | plan.md C | T-005 | PENDING |
| FR-005 | plan.md Capability-declared heavy compute routing | T-006 | PENDING |
| FR-006 | plan.md Project Structure | T-007 | PENDING |
| FR-007 | plan.md Project Structure | T-008 | PENDING |
| FR-008 | plan.md Project Structure | T-009 | PENDING |

## Open questions

- BLOCKED (Arbora, typed packs): the production deployment template and fleet-tool components do not exist in any Agent Library yet (plan.md D4).

R002.2 supplies the pure R076.1 scheduler config contract. FR-003 remains pending until the served R076.2 setting and admission/queue consumers are wired and tested.
