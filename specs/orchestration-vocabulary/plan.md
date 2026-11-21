# MEDULLA-ORCH-VOCAB — Implementation plan

Governing spec: [spec.md](spec.md). Rows: [requirements.md](requirements.md).

## Summary

The orchestration vocabulary makes four questions one graph query each: which steps a task has, which agents staff a topology, which skills, tools, prompts and models bind to each step, and how a harness run performed. The ontology terms, shapes and run-trace records live in `crates/medulla-core/ontology/`. Writes enter through the existing admission path of `crates/medulla-h-graph`. Reads enter through `Method::Uql` of `crates/medulla-h-query`. `AgentAssemble` in `crates/medulla-h-decide` consumes the vocabulary and adds no planner. The reuse candidates are the schema composition of `crates/medulla-server-api/src/schema_compose.rs`, the harness admission rules of `crates/medulla-h-graph/src/harness_admission.rs`, the projection worker pattern of `crates/medulla-h-admin/src/connector_pack_projection.rs` and the revision-pointer path of MEDULLA-DECISION-ENGINE-R805.

## Technical Context

- **Language/Version**: Rust 2021 edition on the pinned toolchain of `rust-toolchain.toml`; Turtle for the ontology; Python 3.14 (floor 3.12) for served tests and the hand-written client helpers.
- **Primary Dependencies**: `medulla-rdf`, `medulla-reasoner`, `medulla-shacl`, `medulla-graph-core`, `medulla-h-graph`, `medulla-h-query`, `medulla-h-decide`, `medulla-h-admin`, `medulla-work-types`, `medulla-result-types`.
- **Storage**: The tenant graph holds trace, step, fact and definition nodes. The Agent Library store holds library entries and its outbox. No second trace store exists.
- **Testing**: Reasoner and shape tests in `crates/medulla-rdf/tests/orchestration_vocabulary.rs`; unit tests per crate; served tests in `crates/medulla-server-served-tests` and `tests/orchestration/` (Python).
- **Target Platform**: Linux x86_64 and aarch64 hosts.
- **Project Type**: Ontology files plus library and handler crates.
- **Performance Goals**: Nightly budgets named by rows: twin capture p50 8 ms and p99 20 ms (R750.753), render p50 10 microseconds and p99 50 microseconds (R800.6), component chain p99 5 ms (R801.6), lesson retrieval p99 10 ms (R802.6).
- **Constraints**: Each refusal has one stable code. Unknown stays unknown and never becomes zero. A write never leaves a partial node. No version suffix in a name.
- **Scale/Scope**: 148 leaf rows in 8 user stories and 8 functional requirements.

**Knob convention.** An engine knob named `knob x.y` in this spec's rows is a typed field of the server configuration, read from the command line or from the environment variable `MEDULLA_<NAME>` (the dotted name upper-cased, dots as underscores) and bounds-checked at startup: a value outside the stated range refuses startup with the knob name. Decision knobs are the exception and live only in the `MEDULLA-DECISION-ENGINE-R179` registry. Rows do not restate this.

## Constitution Check

- **R-I. The engine is native Rust and the only fact store**: pass. Every row lands in a Rust crate or an ontology file of the engine.
- **R-II. Every wait has a deadline**: pass. The projection worker leases rows for a bounded time and every query is paged.
- **R-III. Every row sets budgets and names a benchmark**: pass. Hot-path rows name a nightly bench and a served proof for the pull request.
- **R-IV. Crates are small, layered and cut over hard**: pass. Rows name post-split crates, and no handler crate calls a sibling handler crate.
- **Organization standards**: pass. Rows follow the spec standard and the ecosystem boundaries.

## Project Structure

```text
crates/medulla-core/ontology/orchestration-v1.ttl           existing: terms of R001 to R011
crates/medulla-core/ontology/orchestration-v1.shapes.ttl    existing: shapes of R001 to R011
crates/medulla-core/ontology/swarm_topology-v1.ttl          existing: swarm:feeds, swarm:loopsBackTo, swarm:requiresAgentKind
crates/medulla-core/ontology/sdd-v1.ttl                     existing: spec classes of R012 (new classes)
crates/medulla-core/src/spec_folder.rs                      new: SpecNodeKind, classify_spec_file, parse_requirement_rows
crates/medulla-core/src/spec_folder/policy.rs               new: parse_policy_document, synthesize_policy_candidates
crates/medulla-graph-core/src/graph/schema_sources.rs       existing: core_specs entries for the sources
crates/medulla-h-admin/src/agent_library_projection.rs      new: projection worker beside connector_pack_projection.rs
crates/medulla-h-graph/src/harness_admission.rs             existing: run-trace, step, window, unit and gap admission
crates/medulla-h-query/src/query/                           existing: Uql; new run_views.rs, run_trace_export.rs, session_query.rs
crates/medulla-h-decide/src/decide/                         existing: assemble.rs, candidates.rs, commit.rs; new family files
crates/medulla-compute/src/assemble/                        existing: new topology_facts.rs and synthesis.rs
crates/medulla-work-types/src/orchestration/                existing: mod.rs, improvement.rs; new definitions.rs, org.rs, teams.rs, capability_gap.rs, workflow_rdf.rs
crates/medulla-result-types/src/outcome_bundle.rs           existing: StepRecord, step_digest, CodeWalkthrough
crates/medulla-types/src/                                   new: harness_metrics.rs, harness_session.rs, run_observation.rs, durable_unit.rs, policy_document.rs, run_trace_export.rs
crates/medulla-mutation-types/src/                          existing: harness_metrics.rs; new evolution_metrics.rs (R010.100)
crates/medulla-h-admin/src/agent_library_projection.rs     existing: entry worker; new component worker (R003.6)
crates/medulla-rdf/tests/orchestration_vocabulary.rs        existing: reasoner and shape tests
tests/orchestration/                                        existing: served Python tests
```

## Phase 0: Research

Unknowns and their resolution are in [research.md](research.md). They cover where the agent definition of R808 lives (the Agent Library entry) and how a knowledge scope is checked at publish. Each is decided there. No generated artifact is committed (`MEDULLA-SDLC-R020.1`).

## Phase 1: Design

The rules below bind every row.

1. **One order property.** Step order reuses the core `:precedes`; `:handsOffTo` is a sub-property of it. No second order property exists.
2. **Terms are data.** Agent kinds and termination reasons are closed sets in the ontology and the shapes. No solver or handler carries a term list.
3. **One writer per term.** A term has exactly one writer: typed-pack projection, the Agent Library outbox worker, the topology projection or the assemble commit. Each term has a named reader.
4. **Writes pass the admission path.** A node write enters `try_handle_gateway` and reaches `harness_admission`; an envelope write enters `dispatch_change_envelopes` and reaches the envelope boundary. A refusal writes nothing.
5. **Reads are UQL statements.** Metric, usage, session and gap reads are statements of `HARNESS_QUERIES`, served by `Method::Uql`. Only reads with their own paging or write-side effects get a method (R804 to R807).
6. **Revisions use the pointer path.** A definition, a harness component and a policy set become current only by a champion-pointer move of MEDULLA-DECISION-ENGINE-R805. A measured model fact is the exception by design: it is a field of the `ModelProfile` component, and its head is current (MEDULLA-TYPED-PACKS-R048.2). A newer version never overwrites an older one.
7. **Unknown stays unknown.** A missing measurement is `Unknown(reason)` or an abstention, never zero.
8. **Assembly decides.** Synthesis proposes a draft; `AgentAssemble` decides (MEDULLA-DECISION-ENGINE-R036).

Data entities are in [data-model.md](data-model.md). The shortest served proof is in [quickstart.md](quickstart.md). Each seam to another repository has a document in `contracts/`.

### Standard process and decision vocabulary (R014)

Six core terms describe every process. `:ProcessModel` exists in `enterprise-v1.ttl`. `:ProcessStep` and `:ProcessGate` exist in no ontology file today. R014.1 adds them, with `:ProcessEventNode`, `:ProcessSequenceFlow` and `:ProcessLane`, to `orchestration-v1.ttl`: five classes and seven properties, twelve new terms. `:ProcessStep`, `:ProcessGate` and `:ProcessEventNode` subclass the existing `:TaskStep`; flows and lanes subclass `:Concept`. `bpmn-v1.ttl` and `dmn-v1.ttl` hold the BPMN 2.0 and DMN element classes in the core namespace. Their `:Bpmn` and `:Dmn` IRIs are the ones the process engine connector pack already declares, so one term has one IRI. Each class subclasses one core term and neither file uses `owl:imports`. DMN rolls up to the new core terms `:DecisionLogic` and `:DecisionRule` under `:Policy`. The step binding properties (`:usesTool`, `:hasPrompt`, `:evaluatesDecision`, `:approverRole`, `:toolArguments`) are read by the process plan of `MEDULLA-CQ-ORCH-R704`. Both files and their shapes register as core schema sources beside the `archimate` entry of `crates/medulla-graph-core/src/graph/schema_sources.rs` (:889).

| Core term | BPMN 2.0 classes | DMN classes |
|---|---|---|
| `:ProcessModel` | `:BpmnProcess`, `:BpmnDefinitions` | none |
| `:ProcessStep` | `:BpmnTask` and its kinds, `:BpmnSubProcess`, `:BpmnCallActivity` | none |
| `:ProcessGate` | `:BpmnGateway` and its five kinds | none |
| `:ProcessEventNode` | `:BpmnEvent` and its five kinds | none |
| `:ProcessSequenceFlow` | `:BpmnSequenceFlow` | none |
| `:ProcessLane` | `:BpmnLane` | none |
| `:DecisionLogic` | none | `:DmnDecisionRequirements`, `:DmnDecision`, `:DmnDecisionTable` |
| `:DecisionRule` | none | `:DmnRule` |

One abstraction over process sources (owner, 2026-11-06). R014.9 adds the annotation `:dagNodeKind`, the IRI the `camunda-mcp` and `aris-mcp` packs already use, and `:canonicalGateKind`. Every flow-node class of every source states with them the canonical step kind it becomes; `MEDULLA-CQ-ORCH-R701` turns that into one canonical workflow and `MEDULLA-CQ-ORCH-R704` into one agent DAG. R014.10 keeps the three core EPC terms as shared IRIs and leaves every other EPC class to the ARIS pack, which extends them by subclassing.

| Canonical kind | BPMN classes | EPC classes (ARIS pack) |
|---|---|---|
| `start`, `end` | start and end events | start and end events (no incoming or no outgoing flow) |
| `human` | user and manual tasks | function carried out by an organisational element with no IT support |
| `agent` | service, script, send and plain tasks; intermediate throw events | IT-supported or unspecified function |
| `decision` | business rule task | function linked to a business rule |
| `call` | sub-process, call activity | process interface |
| `wait`, `boundary` | receive task, intermediate catch event, event-based gateway; boundary event | none (export loss, `MEDULLA-CQ-ORCH-R701.8`) |
| `branch`, `fork`, `join` with `xor`, `and`, `or` | exclusive, parallel and inclusive gateways | XOR, AND and OR rule operators |
| `state` (source only, folded) | none | intermediate event |

The seven design answers that bind every row:

- **Owner.** Medulla: the orchestration ontology files and shapes, `crates/medulla-work-types/src/orchestration/` and the served admission path. Each term has one writer and a named reader.
- **Cohesion.** Step order reuses the one core `:precedes`. A definition, a harness component and a policy set each have one writer and become current only by a champion-pointer move; a measured fact is current as the `ModelProfile` component head.
- **Dependency direction.** Handlers return the record and the mapper adds the code to the method set. Nothing imports a handler. The knowledge-scope check runs in the served handler because `medulla-persistence-records` cannot depend on `medulla-retrieval`.
- **Interface depth.** Reads are UQL statements of `HARNESS_QUERIES` served by `Method::Uql`. Only reads with their own paging or write-side effects get a method (R804 to R807).
- **Special cases.** One: unknown stays unknown. A missing measurement is `Unknown(reason)` or an abstention, never zero.
- **State.** Definitions are versioned graph records, and a measured model fact is a `MeasuredFact` on a `ModelProfile` component revision (MEDULLA-TYPED-PACKS-R048.2). A newer version never overwrites an older one.
- **Environment.** Terms are data in the ontology and the shapes; no solver or handler carries a term list. No generated artifact is committed (`MEDULLA-SDLC-R020.1`).

## Phase 2: Task approach

`tasks.md` holds one task per leaf row in table order, producer first. An ontology term precedes the shape that constrains it, a shape precedes the admission that applies it, and an admission precedes the query that reads it. A row lands in one slice with its caller, so no type lands without a production caller. Tasks of different stories run in parallel; a task with no `Depends on:` row carries `[P]`.

## Complexity Tracking

none

## Build contract

- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` pass for every touched crate.
- The architecture, complexity and duplication gates of `.config/pre-commit.yaml` pass with no baseline entry and no suppression.
- Each hot-path row names its budget and its nightly bench; benches never gate a pull request, and the served test of the row does.
- A public item of a types crate has a non-test caller under a handler or server crate. `MEDULLA-CONTRACT-R055` gates this.
- Ontology changes keep the reasoner consistent and every shape test green: `cargo test -p medulla-rdf --features owl orchestration_vocabulary`.
- Spec rows pass `spec-standard`, `plan-conformance` and `spec-decomposition` with zero findings.
- Dispatch stays in the router crate; a handler crate never calls a sibling handler crate.

## Production caller chain

- Schema: `core_specs` (`crates/medulla-graph-core/src/graph/schema_sources.rs:711`) embeds the orchestration sources; `compose_validated_sources` (`crates/medulla-server-api/src/schema_compose.rs:92`) composes them on every restore and read.
- Node write: `Method::AddNode` to `try_handle_gateway` (`crates/medulla-h-graph/src/graph_ops/gateway.rs:57`) to `harness_admission::direct_add_node` (`crates/medulla-h-graph/src/harness_admission.rs:745`) to the admission functions of that file; run-trace, step, window, unit and gap validation extend `harness_add_node_admission` (`crates/medulla-h-graph/src/harness_admission.rs:673`).
- Envelope write: `Method::ApplyChangeEnvelopes` to `dispatch_change_envelopes` (`crates/medulla-server-graph/src/dispatch/change_envelope.rs:18`) to `harness_boundary_envelope` (`crates/medulla-server-graph/src/dispatch/change_envelope.rs:580`).
- Read: `Method::Uql` (`crates/medulla-h-query/src/query/dispatch.rs:138`) to `handle_uql` (`crates/medulla-h-query/src/query/uql_statement.rs:17`).
- Assembly: `Method::AgentAssemble` arm (`crates/medulla-server-router/src/dispatch/router/decision_plane.rs:64`) to `handle_agent_assemble` (`crates/medulla-h-decide/src/decide/assemble.rs:25`) to `assemble_for` (`crates/medulla-h-decide/src/decide/assemble.rs:46`).
- Commit: `Method::DecisionCommit` arm to `handle_decision_commit` (`crates/medulla-h-decide/src/decide/commit.rs:22`).
- Spec folders: `Method::IndexRepository { scope: Some }` to `route_repository_index` (`crates/medulla-server-graph/src/dispatch/graph_pipeline/repository_index.rs:208`).
- Agent Library projection: `src/main.rs` bootstrap (`src/main.rs:488`) starts the worker beside the connector-pack worker.
- Obsolete paths to delete in R801.7: the write-time supersession of `register_version`, `current_harness_component`, `HarnessSupersession`, `AdoptionSupersession`, `current_adopted_harness`, the head-relative check of `component_admission` and the `supersedes_id` stamp of `stamp_new_record` for components.

### Machine wiring

- `WorkItemMachine`: entry point served `ClaimWorkItem`, `RenewWorkItemLease`, `CommitWorkItem`, `CancelWorkItem` and `DeferWorkItem` → the store writers `claim.rs`, `lease.rs`, `commit.rs`, `cancel.rs` and `submit.rs` under `crates/medulla-work-store/src/redb_store/work_item/` → `WorkItemStatus::apply` in `crates/medulla-work-types/src/work_item_machine.rs` → stored state field. Attach points: `claim.rs:58,91,302,494`, `lease.rs:70`, `commit.rs:78,98,163`, `cancel.rs:136,296`. Wiring row: MEDULLA-ORCH-VOCAB-R812.2. Hot-path proof row: MEDULLA-ORCH-VOCAB-R812.4.
- Core vocabulary: entry point server bootstrap → `crates/medulla-graph-core/src/graph/schema_sources.rs` (core spec list, beside `bpmn`) → `state-machines-v1.ttl` and its shapes. Wiring row: MEDULLA-ORCH-VOCAB-R811.5. Hot-path proof row: `test_illegal_transition_returns_all_four_fields` of MEDULLA-ORCH-VOCAB-R811.5 and `test_ready_work_item_lists_claim_and_cancel` of MEDULLA-ORCH-VOCAB-R811.7.
- Lane work item: lane store writers → `WorkItemStatus::apply` of R812.1 → lane row `status` (`types.rs:29`). Wiring row: MEDULLA-ORCH-VOCAB-R813. Hot-path proof row: `terminal_is_derived_from_status` of MEDULLA-ORCH-VOCAB-R813.

### TransitionRefused on the served path (R811.8)

Pre-untangled design, one line per answer.

- Owner. Medulla, `medulla-capabilities` (the method error sets) and each handler crate.
- Cohesion. The list of machine methods and their error set sit in one const, so the contract cannot drift from the handlers.
- Dependency direction. Handlers return the record; the mapper adds the code to the method set; nothing imports a handler.
- Interface depth. One const and one wire object.
- Special cases. None. All eight methods take the same append.
- State. The refusal is the typed edge of every machine row that cites R811.
- Environment. None.

Order: after R811.5 and the three machine rows named in the dependency list.

## Cross-repo seams

- Run-trace metric reads: `MEDULLA-ORCH-VOCAB-R010.1` provides, `ARBORA-OPS-R021.4.4` consumes. Contract: [arbora-run-metrics.md](contracts/arbora-run-metrics.md).
- Evolution proposal metrics: `MEDULLA-ORCH-VOCAB-R010.100` provides the UQL statement `evolution_trace_metrics`, `ARBORA-OPS-R059` consumes it through the engine client `uql`; `MERISTEM-HARNESS-CYCLE-R064` writes the `EvolutionProgress` node it reads.
- Run-trace producers: `MEDULLA-ORCH-VOCAB-R013.2` and `MEDULLA-ORCH-VOCAB-R009.18.2` provide, `MERISTEM-HARNESS-CYCLE-R072` and `MERISTEM-HARNESS-CYCLE-R071` consume. Contract: [meristem-run-trace-producer.md](contracts/meristem-run-trace-producer.md).
- Harness session index: `MEDULLA-ORCH-VOCAB-R009.101.1` provides, `SCION-SOURCE-INGEST-R013` consumes. Contract: [scion-harness-session-ingest.md](contracts/scion-harness-session-ingest.md).
- Policy activation: `MEDULLA-ORCH-VOCAB-R806` provides, `ARBORA-OPS-R050.2` consumes. Contract: [arbora-policy-activation.md](contracts/arbora-policy-activation.md).
- Organization runtime and teams: `MEDULLA-ORCH-VOCAB-R750.751` provides, `MERISTEM-HARNESS-CYCLE-R074` consumes. Contract: [meristem-org-runtime.md](contracts/meristem-org-runtime.md).

### Explicit state machines

Organization constitution Article XXXI requires a closed set of states, one transition function and a typed refusal for each machine. Rows `MEDULLA-ORCH-VOCAB-R811`, `MEDULLA-ORCH-VOCAB-R812`, `MEDULLA-ORCH-VOCAB-R813` apply it here. Each machine returns `TransitionRefused` (MEDULLA-ORCH-VOCAB-R811.4), stores its state in one field, and is described in `crates/medulla-core/ontology/state-machines-v1.ttl` with classes that roll up to the core state-machine classes. The change is a hard cutover: the old literals, flags and inferred states are deleted in the same release.

## Amendments

### 2026-11-20: build-lane gaps resolved

Withdrawn rows, each with its reason:

- `MEDULLA-ORCH-VOCAB-R803`: the five run-trace queries are already served by `Method::Uql` through `HARNESS_QUERIES` (R010.1 to R010.5); a second method is a parallel read path with no paging or write of its own, against plan principle 5.
- `MEDULLA-ORCH-VOCAB-R815.3`: `ChildInputState` has no production writer of its `Apply` event; the record is built once in `Queued` and no served method moves it, so no machine is needed.
- `MEDULLA-ORCH-VOCAB-R815.1`: `ActionStatus` is a classification, not a lifecycle; an `ActionRun` is created in its final outcome, `Applied` or `Refused`, and never moves.
- `MEDULLA-ORCH-VOCAB-R815.7`: `WorkItemDeferStatus` is the outcome classification of one `DeferWorkItem` answer, not a stored lifecycle; the work item lifecycle is `WorkItemMachine` (R812).
- `MEDULLA-ORCH-VOCAB-R815.8`: `WorkOfferView.stage` is the version stamp of the pricing function (`WORK_OFFER_UTILITY_RATE_STAGE`), not a lifecycle state.
- `MEDULLA-ORCH-VOCAB-R007.2`: the committed assembly is the sealed `DecisionRecord`, served by MEDULLA-CQ-ORCH-R008; a graph copy would need a sync path, and its `swarm:feeds` edges are the template facts R006.5 already projects.
- `MEDULLA-ORCH-VOCAB-R011.7`: a measured model fact is stored once, as a `MeasuredFact` on the `ModelProfile` component (MEDULLA-TYPED-PACKS-R048.2), where `HarnessConfig` reads it (MEDULLA-DECISION-ENGINE-R128.8); a second graph node has no reader.
- `MEDULLA-ORCH-VOCAB-R011.8`: `MeasuredFact::validate` already refuses a measured fact without its run at publish (MEDULLA-TYPED-PACKS-R048.2).
- `MEDULLA-ORCH-VOCAB-R011.1` to `MEDULLA-ORCH-VOCAB-R011.6`: the `:EvaluationRun`, `:MeasuredModelFact`, `:factOfModel`, `:measuredProperty`, `:evidencedByRun` and `:measuredSupersedes` terms and `:MeasuredModelFactShape` have no writer or reader once the fact lives on the `ModelProfile` component; the declarations and their tests are removed.
- `MEDULLA-ORCH-VOCAB-R011.9`: a measured-fact champion pointer would be a second notion of the current fact; `HarnessConfig` reads the `ModelProfile` head.

Resolved rows:

- `MEDULLA-ORCH-VOCAB-R007.1`: steps are recorded as `DecisionInputs.steps`; only the requirement derivation reads them; `STEP_ORDER_CYCLE` joins `DecisionErrorCode`.
- `MEDULLA-ORCH-VOCAB-R811.9`: `Response.transition_refused` carries the four fields; `Response::err` fills it from the canonical detail; `IncidentSetStatus` is the proof path.
- `MEDULLA-ORCH-VOCAB-R006.5`: the R003.6 projection is the one writer of template `swarm:` facts; `AgentGraphEdge.kind` marks a loop back; the publish rules refuse a feeds cycle.
- `MEDULLA-ORCH-VOCAB-R003.6`: the source is the `A2aAgentCard` component on `medulla.agent-component.revision.v1`, not `AgentLibraryEntry`; a second consumer `agent-component-projection` commits `project_component` triples through the R003.6.1 envelope; the kind is the `agent_kind` attribute, refused at publish with `MALFORMED_A2A_AGENT_CARD` when it is not an IRI.
- `MEDULLA-ORCH-VOCAB-R002.2`: `:hasAgentKind` carries no domain, so a projected card is not inferred to be an `:Agent`.
- `MEDULLA-ORCH-VOCAB-R010.100`: one UQL statement `evolution_trace_metrics` in `HARNESS_QUERIES`; the join reads `EvolutionProgress`, `ProgramRevision` and `RunTrace` nodes of one RLS snapshot; the window digest is `Digest256::framed(b"eg/evolution-metric-window/v1", ..)`; the three refusals join `ServerErrorCode` and the `Uql` entry of `EXACT_METHOD_ERRORS`.
- `MEDULLA-ORCH-VOCAB-R011.10`: one `AgentComponent` `Publish` of the next `ModelProfile` revision per evaluation result; no promotion follows.
- `MEDULLA-ORCH-VOCAB-R011.11`: `HarnessConfig` cites the measured fact of the `ModelProfile` head (MEDULLA-DECISION-ENGINE-R128.11).
- `MEDULLA-ORCH-VOCAB-R007.4`: the slot kind is matched against the candidate component's `agent_kind` attribute (R003.6), read from `ReadCandidates.entries`.
- `MEDULLA-ORCH-VOCAB-R014.3`: the exact triples are in `data-model.md`; the `dmn` core source is registered; each pin moves by a stated delta.
- `MEDULLA-ORCH-VOCAB-R815.2`, `R815.4` to `R815.6` and `R815.9`: each names its initial state, events, edge table, writers and refusal.

### 2026-11-20 (pass 3b): wave-3 gap rows resolved

Withdrawn rows, each with its reason:

- `MEDULLA-ORCH-VOCAB-R616.3`: the wiring is the caller chain R616.2 already names; the existing solver decides.
- `MEDULLA-ORCH-VOCAB-R616.4`: a draft graph's shape is refused at the one `AgentGraph` `Publish` chokepoint (`TOPOLOGY_SHAPE_INVALID`, R006.5); a second check with `SPEC_SHAPE_INVALID` would be a second route.
- `MEDULLA-ORCH-VOCAB-R801.2`: write-time supersession is replaced by the champion pointer of R801.7.

Resolved rows:

- `MEDULLA-ORCH-VOCAB-R004.4`, `MEDULLA-ORCH-VOCAB-R005.5`: one arm each in R003.6's `project_component`; publish already refuses an unscoped rule (`MALFORMED_GUARDRAIL_RULE`) and a zero window, so no sweep code is added; the window comes from `AgentComponentFacts::ModelProfile`, not an attribute.
- `MEDULLA-ORCH-VOCAB-R006.5`: graph templates arrive on `medulla.agent-graph.revision.v1`; one generic `ProjectedStream` worker serves the entry, component and graph streams; graph IRI `{CORE}agentGraph/<tenant>/<graph_id>`.
- `MEDULLA-ORCH-VOCAB-R010.6`: its own statement `harness_config_evidence` with `ConfigEvidence` rows carrying trace ids in provenance; the threshold is the `$min_traces` read parameter (default 20, [5, 1000]), not a stored knob.
- `MEDULLA-ORCH-VOCAB-R010.100`: `sources: Vec<SourceTrace { run_id, state: TraceState }>` replaces `trace_refs` and `missing_ids`, keeping run order on the wire; the nightly bench is named and registered.
- `MEDULLA-ORCH-VOCAB-R011.10`: the server binds the whole mutation context except `tenant_id` and `expected_revision`; the helper sends `expected_revision = head.entry_revision` and detects a retry by comparing the head's field.
- `MEDULLA-ORCH-VOCAB-R750.752`: `TWIN_PIN_MISSING` is a `PARAMETER_INVALID` detail; an excluded offer is an answer, not a refusal; `STATECHART_NO_TRANSITION` and `INSTANCE_DEFINITION_UNREGISTERED` join `ServerErrorCode` and a `Statechart` entry of `EXACT_METHOD_ERRORS`.
- `MEDULLA-ORCH-VOCAB-R814`, `MEDULLA-ORCH-VOCAB-R805`: a turn is a committed run; timings ride `RunCompletionInput.timings`, so there is no `DecisionLog` entry and no orphan refusal; the session read assigns turn indexes by commit sequence.
- `MEDULLA-ORCH-VOCAB-R014.6`: `:dmnTable` and `:dmnDecision` are declared as the importer writes them; hit policy, aggregation and requires-decision cycle are shapes; entry counts are refused at `parse_dmn` (`RuleEntriesMismatch`).
- `MEDULLA-ORCH-VOCAB-R801.7`: versions and adoptions are written with no supersession; `PointerRef::Champion` (MEDULLA-DECISION-ENGINE-R805.3) makes one current; the pointer history is the provenance; R801.1, R801.5 and R801.6 follow the cutover.
