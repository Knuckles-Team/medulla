# MEDULLA-PLACEMENT — Implementation plan

Governing spec: [spec.md](spec.md). Rows: [requirements.md](requirements.md).

All three gaps keep ONE decision authority: the engine decides, callers (Arbora, the ingest worker, the submit path) ask and obey. Nothing here adds a planner outside `specs/decision-engine`.

## Summary

This spec closes three engine-side design gaps so each keeps one decision authority: the placement question (MEDULLA-PLACEMENT-R001), the ingest-worker policy (MEDULLA-PLACEMENT-R002) and the association payload bound (MEDULLA-PLACEMENT-R003). The engine decides; Arbora, the ingest worker and the submit path ask and obey. Nothing here adds a planner outside specs/decision-engine. The owner-batch rollups MEDULLA-PLACEMENT-R611, MEDULLA-PLACEMENT-R750, MEDULLA-PLACEMENT-R751 and MEDULLA-PLACEMENT-R700 keep only their engine-side slices, citing their cross-repo owners.

## Technical Context

| Field | Value |
|---|---|
| Language/Version | Rust workspace (medulla 3.x) plus the Python client; served methods over the wire protocol. |
| Primary Dependencies | medulla-decision-types, medulla-h-decide, medulla-jobs, medulla-resource, medulla-types, medulla-work-types, medulla-mining. No new external dependencies. |
| Storage | Tenant graph (placement vocabulary and host capabilities), job store and job event log, capacity ledger read as premises only. |
| Testing | Levels L1 to L6; crate unit tests, served journey tests under tests/, nightly bench groups that never gate a PR. |
| Target Platform | The native engine (Linux); the Python client for the SDK runner. |
| Project Type | Engine feature: pure functions plus served methods. No new service. |
| Performance Goals | Claim pacer p50 under 2 microseconds and p99 under 10 microseconds; schedule tick p50 under 3 ms and p99 under 9 ms; placement plan p50 under 12 ms and p99 under 80 ms. All nightly, never PR-gating. |
| Constraints | Decide is evaluate-only and takes no lease; the worker holds no policy; one decision authority per concept. |
| Scale/Scope | At most 512 hosts and 4,096 services per placement plan; at most 31 distinct association items; at most 64 routing candidates per decision. |

## Constitution Check

Single decision authority per concept: placement, worker policy and the analytics bound each keep exactly one owner (this spec or the named cross-repo spec). No stubs: `AgentAssemble` is confirmed not a refusal stub (decision D1). Every row carries its budget and names its proof. No served operation is added outside the documented method family; `Decide` and `AgentAssemble` are evaluate-only and write nothing.

## Project Structure

```text
crates/medulla-decision-types/src/decision/statistical/placement.rs      existing  Placement types, place, route_heavy_job (R001.2 to R001.6, R004.1, R004.2)
crates/medulla-decision-types/src/decision/statistical/placement_plan.rs new       plan types (R700.1)
crates/medulla-decision-policy/src/solve/placement.rs                    new       plan_services (R700.2)
crates/medulla-h-decide/src/decide/stat_placement.rs                     new       placement_outcome, load_target_facts (R001.5 to R001.10)
crates/medulla-h-decide/src/decide/stat_placement_plan.rs                new       placement_plan_outcome (R700.3 to R700.5)
crates/medulla-h-decide/src/decide/capacity_premise.rs                   existing  headroom premise (R001.8)
crates/medulla-h-decide/src/decide/assemble.rs                           existing  AgentAssemble (R001.12, R001.13)
crates/medulla-core/ontology/placement.ttl, placement.shapes.ttl         new       placement vocabulary and shapes (R001.5)
crates/medulla-work-types/src/worker_registry/grants.rs                  existing  granted_slots; WorkerGrant added (R002.3, R002.4)
crates/medulla-types/src/scheduler_config.rs                             existing  engine-owned settings (R002.2, R002.14, R751.753)
crates/medulla-h-jobs/src/jobs/submit.rs                                 existing  association bound (R003.1 to R003.6)
crates/medulla-mining/src/mining/association.rs                          existing  kernel guard (R003.4)
crates/medulla-h-jobs/src/jobs/host_capabilities.rs                      new       declared_for (R004.3, R004.4)
crates/medulla-jobs/src/store.rs                                         existing  claim_next routing and pacing (R004, R750.753)
crates/medulla-resource/src/admission.rs                                 existing  PriorityClass, ClaimPacer (R750)
crates/medulla-jobs/src/schedule.rs                                      existing  schedule, tick ledger, leadership (R751)
crates/medulla-h-jobs/src/jobs/executor_loop.rs                          existing  worker pool and tick task (R002.14, R751.753)
medulla/placement.py                                                     existing  tenant pool, breaker, route directory (R611)
```

## Phase 0: Research

Unknown (see research.md): the production deployment template and fleet-tool components do not exist in any Agent Library yet (Arbora, typed packs). Until they land, the assembled step-graph proof runs over a fixture template.

### Problem and evidence

- A. Arbora `ARBORA-DEPLOY-R017` (branch `land/train-1010h`, `specs/portable-deployment/requirements.md:27`) requires "a deployment-placement question kind under `MEDULLA-DECISION-ENGINE-R046`/`R111`" and says assemble is "a refusal stub". In code: `QuestionKind` (`crates/medulla-decision-types/src/decision/statistical.rs:56-95`) has no placement or topology kind. `R111.1` (`specs/decision-engine/requirements.md:163`) says the standalone topology question is not built and its proving test name `a_standalone_topology_decide_question_returns_a_typed_decision_record` appears nowhere in `src`. Topology is reachable only through `AgentAssemble` (`crates/medulla-h-decide/src/decide/assemble.rs:57`, `topology_read.rs:44`).
- B. `specs/ingest-worker` (R001.2, .3, .10, .11, .15) asks the worker/Graft SDK to autosize (0.36 of cores, 3 GiB), hold a hydration floor and route partitions. `MEDULLA-DURABLE-KERNEL-R074`-`R077` (`specs/durable-graph-kernel/requirements.md:109-112`) give the engine the registry, admission, floors and shard-writer width. No code for either exists yet (`git grep AdmitWorkerTask|SchedulerConfig|WorkerRecord` finds nothing in `src`/`crates`). The overlap is still only in specs and is cheap to remove now. The only live autosize knob is `MEDULLA_ANALYTICS_WORKERS` (`crates/medulla-h-jobs/src/jobs/executor_loop.rs:22-25`, default 1, cap 32).
- C. `MEDULLA-DURABLE-KERNEL-R082.3` says the cap's existence "was not determinable". It is: `crates/medulla-h-jobs/src/jobs/submit.rs:289` refuses `distinct_items > 31` (inside the same merged check as the threshold tests, one message at `:293`).

## Phase 1: Design

The placement question family (MEDULLA-PLACEMENT-R001): the `Placement` question kind, the target registry, the typed request body with refusals, the pure feasibility function, the headroom premise, the why-not evidence, the typed abstention and the served contract proof.

### Architecture

```
caller (Arbora / worker / submit)        engine (single authority)
 question + typed body ------Decide-----> QuestionKind::Placement (stat_placement.rs)
                                            |- candidates: PlacementTarget (declared options)
                                            |- hard constraints -> eliminated + why-not
                                            |- capacity headroom premise (capacity_premise.rs, no lease)
                                            |- DecisionPolicy caps (request may only tighten)
                                            '- cheapest feasible -> Advisory; none -> Abstained
 task + capabilities -------AgentAssemble-> graph.assemble() over published deployment template
                                            -> Solved graph (ordered fleet-tool steps) | Abstained
 worker Register{cores,mem} -> WorkerRecord (R074) -> AdmitWorkerTask (R075) -> WorkerGrant{slots,lane}
                                            SchedulerConfig (R076) + shard width (R077) decide everything
 submit MineAssociate -> submit.rs MAX_ASSOCIATION_DISTINCT_ITEMS=31 (u32 mask width of rule enumeration)
```

#### A. Placement question

- **Question**: `QuestionKind::Placement`, candidates `CandidateSource::Declared` with option ids `placement:in_process|compose|helm|swarm` (same shape as `IngestionLane`, `statistical/ingestion_lane.rs:24-47`, and `ToolSubset`, `stat_tool_subset.rs`). It is a flat choice among deployment targets, not a template/slot assembly, so it does not go through `TopologyRequirements` (`decision/topology.rs:110`), which sizes widths of a swarm template. It reuses the topology path's pieces: the capacity headroom premise (`capacity_premise.rs:headroom`, R112), the policy-tightening rule (`TopologyCaps::loosened_field`, `topology.rs:~60`), and the standalone-typed-question dispatch R111.1 builds first.
- **What is placed**: `PlacementSubject{workload_iri, cpu_millis, memory_bytes, replicas, needs_persistent_volume}`.
- **Constraints**: `PlacementConstraints{requires_ha, forbidden_targets, max_relative_cost, capacity_cells, priority}`; capacity from the ledger at the stated `LeasePriority`; policy from the pinned `DecisionPolicy` (request can only tighten).
- **Target facts** are data, not code: an admin-attached schema source (like `swarm-topology`, `topology_schema.rs`) states per target `placement:supportsHa`, `placement:supportsVolumes`, `placement:maxReplicas`, `placement:relativeCost`. The engine hardcodes none of them. Only the four-member target registry is closed (it must match Arbora's supported artifacts; systemd is intentionally absent, ARBORA-DEPLOY-R017.6).
- **Answer**: chosen target (`Advisory` top score, `resolution_kind` Constraint when one candidate survives, Optimization when cost chooses), evidence class from premises (capacity headroom = Observation, target facts = their schema authority), why-not for each other target via `DecisionRecord.why_not`/`eliminated` (`record.rs:267-270`), and typed abstention via `StatisticalOutcome::Abstained{reasons: AbstainReason}` (`outcome.rs:107`) when no target is feasible or capacity facts are unavailable (`CAPACITY_UNAVAILABLE`, `topology_read`/`capacity_premise.rs:30`). No lease is taken (R046); acquisition stays `AcquireCapacity`.
- **Step graph**: separate call, `AgentAssemble` over a published deployment graph template whose slots are fleet-tool components (render-artifact, apply, verify). Chosen target selects the template by IRI (`AssemblyRequest.templates`).

#### B. Single arrangement

The worker holds NO policy. Sequence: `register(cores, mem_bytes, lanes_supported)` -> engine returns `WorkerRecord` and the engine-computed `WorkerGrant{slots, reserved_lane}`; each claim goes through `AdmitWorkerTask`; claim order and lane are derived by the engine from `lane_for(kind)` and `partition_key(envelope)` (engine-side pure functions, ingest-worker R001.2/.3, now explicitly engine-called). Autosize constants (360 per-mille of cores, 3 GiB per worker), lane floors and the queue-depth limit become fields of `SchedulerConfig` (R076) with those defaults. The worker's `--workers` becomes a request ceiling the engine may lower, never raise. A one-worker minimum never overrides a zero request ceiling, insufficient available memory or zero writer width; those cases return a typed capacity refusal and no allocation. Admission is a deterministic served constraint (R075), not a `Decide` question: `QuestionKind::IngestionLane` (fast/medium/slow) is a different concept (derivation cost lanes) and must not be conflated with worker lanes general/hydration.

#### C. Association bound

`rules_for_itemset` enumerates antecedent masks `for mask in 1u32..((1u32 << k) - 1)` (`crates/medulla-mining/src/mining/association.rs:402`), and `rule_parts`/`rule_for_mask` take `mask: u32` (`:349`, `:364`). `k` is an itemset length, which cannot exceed the number of distinct items, so 31 distinct items guarantees `1u32 << k` cannot overflow (`1u32 << 32` overflows). The Apriori/Eclat/FP-Growth itemset engines themselves have no bound (`association.rs:124`, `:265`); `generate_rules` is `pub`, so a direct caller with k >= 32 is unprotected.

### Decisions

| # | Item | Outcome | Evidence and reasoning |
|---|---|---|---|
| D1 | `AgentAssemble` is a refusal stub? | CONFIRMED NOT a stub | `crates/medulla-h-decide/src/decide/assemble.rs:28-55` reads the library snapshot, runs `medulla_compute::assemble`, returns `AssemblyResult`; routed at `crates/medulla-server-router/src/dispatch/router/decision_plane.rs:64`. It refuses only when built without the `decide` feature (`assemble.rs:134-140`; `decide` is in `full`, `Cargo.toml:1278`, which `default` enables, `Cargo.toml:168`). The Arbora "stub" claim is therefore stale or describes a build without `decide`; propose reword (R001.14). |
| D2 | What does the topology decision path return today? | CONFIRMED | Only inside assembly: `TopologyPlan` (`topology/plan.rs:120`) when `request.requirements.topology` is set (`topology_read.rs:44-47`), else `None`. Standalone Decide topology question: absent (R111.1 open; test name not in tree). |
| D3 | Placement ID/shape | DECIDED | New `QuestionKind::Placement` with declared candidates, modelled on `IngestionLane`/`ToolSubset`; alternative (encode targets as swarm templates in `TopologyRequirements`) rejected: targets are not slots/widths, and it would stretch the swarm model. Depends on R111.1 only for the shared standalone-dispatch pattern, not for its types. |
| D4 | Assemble prerequisites | BLOCKED (owner: Arbora + typed packs) | Handler exists, but a `Solved` deployment graph needs published components: fleet-tool components and a deployment graph template in the tenant Agent Library. None exists for deployment; Arbora publishes them as a typed pack (`specs/typed-packs-and-catalog`). Medulla row R001.12 proves assembly with a fixture template. |
| D5 | Ingest-worker overlap | DECIDED | Engine owns admission, floors, widths, routing, autosize constants and depth limit; worker asks/obeys. Seven rows reworded below. Alternative (keep worker autosize as a hint) kept only as a request ceiling. |
| D6 | Why 31 | CONFIRMED | `u32` mask in `association.rs:402`, `:349`, `:364`; `1u32 << 32` overflows. Cap check `submit.rs:289`. |
| D7 | The bound | DECIDED | Keep 31 as named `MAX_ASSOCIATION_DISTINCT_ITEMS = u32::BITS - 1` in `submit.rs` (byte-identical parity with the old worker, R082.4, and rule enumeration cost is already 2^k, so a wider mask gains nothing: 2^63 is infeasible). Alternative (u64 masks, cap 63) rejected. Add a defence-in-depth guard so `generate_rules` cannot overflow for k >= 32. |
| D8 | Opaque-reference rule of R082.3 | DECIDED | Not a refusal: the engine itself maps every label to an opaque ref (`submit.rs:266-278`, `native_opaque_ref`) before counting, so an `eg:` prefix check on input would reject valid labels for no gain. The cap is measured after hashing. Tests cover deterministic references and distinct outputs for bounded fixtures; they do not prove that hashing is injective. R082.3's non-opaque-item test is replaced by a test that a plain label is accepted and stored only as an opaque ref. |

### Worked example

Question: "Where should the `graphos-core` workload (2000 mCPU, 4 GiB, 2 replicas, needs HA, no volume) run, among in_process, compose, helm, swarm?"

```
Decide{kind: placement, candidates: [placement:in_process, :compose, :helm, :swarm],
       params: workload_iri, cpu_millis=2000, memory_bytes=4GiB, replicas=2,
       requires_ha=true, capacity_cells=[cell-a], priority=Normal}
 -> eliminated: in_process (no HA), compose (single host: supportsHa=false)
 -> feasible: helm (cost 3), swarm (cost 2); capacity headroom cell-a covers both
 -> Advisory[ swarm 1.0 (Optimization), helm 0.0 ]   evidence_class: Observation
    why_not: in_process "placement:supportsHa=false", compose "placement:supportsHa=false",
             helm "costlier than swarm (3 > 2)"
Then AgentAssemble{task: deploy-Arbora-core, templates:[deploy-swarm-template]}
 -> Solved graph: render-artifact -> apply -> verify (engine order, certificate)
```
If cell-a headroom is below demand for every feasible target: `Abstained[CAPACITY_INFEASIBLE lease:cell-a:cpu]` and no lease taken.

### Out of scope

Rendering or applying manifests (Arbora `deploy/`), acquiring capacity, systemd, runtime auto-migration between targets, and the worker runner itself (Graft SDK).

#### B producer implementation boundary

R002.2 is implemented by the pure `medulla_types::scheduler_config` contract (R076.1). R076.2 owns its later served integration. Neither the autosize grant function nor the existing analytics pool has been switched by this producer slice.

R002.3 now provides pure `worker_registry::granted_slots` with `WorkerSizingInput` for the optional request ceiling, current available memory and engine-selected write classification. It consumes the real `WorkerRecord` and `SchedulerConfig`; the typed `WorkerSlotGrant` reports zero on refusal. CPU sizing alone may round up to one; memory, request ceiling, pool, disabled resources and writer width remain hard bounds. No reservations, served registration or R075 admission are claimed.

### Capability-declared heavy compute routing (R004)

Data flow: job submit wire method -> ready queue -> `claim_next` (`crates/medulla-jobs/src/store.rs:736`) -> capability anchor filter (`store.rs:773`) -> claim on the capable host.

Types: `HostCapability`, `RoutingFact`. Attach point: `store.rs:773`. Production caller: `crates/medulla-quantum-jobs/src/job.rs:275`, which passes no capabilities today; the Wiring row passes them. Failure behavior: `no_capable_host` leaves the job queued.

Reused: the capability anchor selection in `select_ready_for_worker`. New: capability declaration from graph topology facts.

Decision: hosts declare capabilities and the graph stores them. Rejected alternative: a fixed host list in configuration, because it drifts from the real topology.

Cross-repository: `ARBORA-` rows that submit jobs only pass the job class.

## Phase 2: Task approach

The ingest-worker policy (MEDULLA-PLACEMENT-R002) and the association bound (MEDULLA-PLACEMENT-R003) land with their durable-kernel owner rows. The owner-batch rollups (MEDULLA-PLACEMENT-R611, MEDULLA-PLACEMENT-R750, MEDULLA-PLACEMENT-R751, MEDULLA-PLACEMENT-R700) follow their cross-repo owners and keep only their engine-side slices.

## Complexity Tracking

- Decision D3: the target is modelled as a declared option, not a swarm template, so the topology model stays intact.
- Decision D7: the 31-item bound is the `u32` mask width; rule enumeration is already 2^k, so a wider mask gains nothing; the u64 alternative is rejected.
- The capacity adjustment question and verdict move to MEDULLA-DECISION-ENGINE-R824 and the model capacity question to MEDULLA-DECISION-ENGINE-R825 under MEDULLA-DECISION-ENGINE-R112; Arbora actuates; this spec keeps claim pacing, priority classes and the headroom premise.
- The seven ingest-worker rewording rows retire to MEDULLA-INGEST-WORKER-R001 (MEDULLA-PLACEMENT-R002.6 to MEDULLA-PLACEMENT-R002.12 and MEDULLA-PLACEMENT-R002.15 are retired rows).

## Build contract

Focused `cargo test` on the named files per row; `spec-status` and `spec-decomposition` gates compute the state. Cargo is run by the implementing lane, not by this design. The nightly bench groups (claim pacing, schedule tick, placement plan, tenant pool) carry the budgets and never gate a PR.

## Production caller chain

- Arbora `plan.deploy` (ARBORA-DEPLOY-R017) asks served `Decide` with `placement` (`Method::Decide` arm, `crates/medulla-server-router/src/dispatch/router/decision_plane.rs:112` → `stat_decide::handle_decide`, `crates/medulla-h-decide/src/decide/stat_decide.rs:60` → `placement_outcome`), then `AgentAssemble` with the chosen template key (`handle_agent_assemble`, `crates/medulla-h-decide/src/decide/assemble.rs:25`).
- The worker runner (SCION-SOURCE-INGEST-R011.1) registers through served `RegisterWorker` (MEDULLA-DURABLE-KERNEL-R074), admits through `AdmitWorkerTask` (MEDULLA-DURABLE-KERNEL-R075) and claims through the engine claim path (`handle_worker_claim`, `crates/medulla-h-jobs/src/jobs/worker_requests.rs:92` → `JobStore::claim_next`, `crates/medulla-jobs/src/store.rs:742`).
- A caller submits `MineAssociate` through served `SubmitJob`, handled in `crates/medulla-h-jobs/src/jobs/submit.rs`.
- The tenant client (`medulla/placement.py`) asks the engine for the route (`PlacementRouteRequest`) and the object store open check consumes it (MEDULLA-OBJECT-STORAGE-BACKEND-R004).

## Cross-repo seams

| Seam | Provider | Consumer |
|---|---|---|
| arbora-deployment | medulla MEDULLA-PLACEMENT-R001 | arbora ARBORA-DEPLOY-R017 |
| worker-registry | medulla MEDULLA-DURABLE-KERNEL-R075 | medulla MEDULLA-PLACEMENT-R002 |
| capacity-decisions | medulla MEDULLA-DECISION-ENGINE-R112 | medulla MEDULLA-PLACEMENT-R001 |
| ingest-worker-wording | medulla MEDULLA-INGEST-WORKER-R001 | medulla MEDULLA-PLACEMENT-R002 |
| worker-runner | medulla MEDULLA-PLACEMENT-R002 | scion-sdk SCION-SOURCE-INGEST-R011.1 |
| object-storage-route | medulla MEDULLA-PLACEMENT-R611 | medulla MEDULLA-OBJECT-STORAGE-BACKEND-R004 |

### Explicit state machines

Organization constitution Article XXXI requires a closed set of states, one transition function and a typed refusal for each machine. Rows `MEDULLA-PLACEMENT-R780`, `MEDULLA-PLACEMENT-R781`, `MEDULLA-PLACEMENT-R782`, `MEDULLA-PLACEMENT-R783` apply it here. Each machine returns `TransitionRefused` (MEDULLA-ORCH-VOCAB-R811.4), stores its state in one field, and is described in `crates/medulla-core/ontology/state-machines-v1.ttl` with classes that roll up to the core state-machine classes. The change is a hard cutover: the old literals, flags and inferred states are deleted in the same release.

#### Machine wiring

- `LaneHoldMachine`: entry point served reserve and cleanup operations → `reserve.rs` and `cleanup.rs` under `crates/medulla-result-types/src/epistemic_operations/` → `HoldState::apply` in `crates/medulla-result-types/src/epistemic_operations/hold_machine.rs` → stored state field. Attach points: `reserve.rs:358` and `cleanup.rs:67,131,368`. Wiring row: MEDULLA-PLACEMENT-R780.1. Hot-path proof row: `terminal_work_item_releases_hold` of MEDULLA-PLACEMENT-R780.2.
- `ResourceReservationMachine`: entry point served resource lifecycle methods → `resource/write_lifecycle.rs` → `ResourceReservationRecordState::apply` in `crates/medulla-result-types/src/epistemic_operations/resource_core.rs` → stored state field. Attach points: `write_lifecycle.rs:115`. Wiring row: MEDULLA-PLACEMENT-R781.1. Hot-path proof row: `no_writer_sets_state_outside_apply` of MEDULLA-PLACEMENT-R781.1.
- `CapacityLeaseMachine`: entry point served capacity lease methods → the lease store in `crates/medulla-resource-store` → `CapacityLeaseState::apply` in `crates/medulla-mutation-types/src/capacity_lease.rs` → stored state field. Attach points: `capacity_lease.rs:770,772,942`. Wiring row: MEDULLA-PLACEMENT-R782.1. Hot-path proof row: `no_writer_sets_state_outside_apply` of MEDULLA-PLACEMENT-R782.1.
- `ControlLeaseMachine`: entry point served control lease transition method → `apply_transition_control_lease_row` → `ControlLeaseStatus::apply` in `crates/medulla-work-types/src/control_lease.rs`, called by `apply_transition_control_lease_row` → stored state field. Attach points: `control_lease.rs:142` of the work store. Wiring row: MEDULLA-PLACEMENT-R782.3. Hot-path proof row: `no_writer_sets_state_outside_apply` of MEDULLA-PLACEMENT-R782.3.
- `MembershipShrinkMachine`: entry point the shrink controller → `advance` and `abort` → `advance` (`:210`) and `abort` (`:240`) in `crates/medulla-raft/src/raft/membership_shrink.rs` → stored state field. Attach points: `membership_shrink.rs:210,240`. Wiring row: MEDULLA-PLACEMENT-R783.2. Hot-path proof row: `no_writer_sets_state_outside_apply` of MEDULLA-PLACEMENT-R783.2.
- Sibling raft machines: inventory only in MEDULLA-PLACEMENT-R783.3; the edge tables are not yet extracted.
