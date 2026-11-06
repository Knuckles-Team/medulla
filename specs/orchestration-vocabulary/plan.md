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
- **Performance Goals**: Nightly budgets named by rows: synthesis p99 250 ms (R616.6), twin capture p50 8 ms and p99 20 ms (R750.753), team score p50 6 ms and p99 15 ms (R751.753), render p50 10 microseconds and p99 50 microseconds (R800.6), component chain p99 5 ms (R801.6), lesson retrieval p99 10 ms (R802.6).
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
crates/medulla-types/src/                                   new: harness_metrics.rs, harness_session.rs, run_observation.rs, durable_unit.rs, policy_document.rs, run_trace_export.rs, evolution_metrics.rs
crates/medulla-protocol-types/src/change_envelope/          existing: new measured_fact.rs
crates/medulla-rdf/tests/orchestration_vocabulary.rs        existing: reasoner and shape tests
tests/orchestration/                                        existing: served Python tests
```

## Phase 0: Research

No unknowns; no research.md. No generated artifact is committed (`MEDULLA-SDLC-R020.1`).

## Phase 1: Design

The rules below bind every row.

1. **One order property.** Step order reuses the core `:precedes`; `:handsOffTo` is a sub-property of it. No second order property exists.
2. **Terms are data.** Agent kinds, termination reasons and measured properties are closed sets in the ontology and the shapes. No solver or handler carries a term list.
3. **One writer per term.** A term has exactly one writer: typed-pack projection, the Agent Library outbox worker, the topology projection or the assemble commit. Each term has a named reader.
4. **Writes pass the admission path.** A node write enters `try_handle_gateway` and reaches `harness_admission`; an envelope write enters `dispatch_change_envelopes` and reaches the envelope boundary. A refusal writes nothing.
5. **Reads are UQL statements.** Metric, usage, session and gap reads are statements of `HARNESS_QUERIES`, served by `Method::Uql`. Only reads with their own paging or write-side effects get a method (R803 to R807).
6. **Revisions use the pointer path.** A definition, a harness component, a measured fact and a policy set become current only by a champion-pointer move of MEDULLA-DECISION-ENGINE-R805. A newer version never overwrites an older one.
7. **Unknown stays unknown.** A missing measurement is `Unknown(reason)` or an abstention, never zero.
8. **Assembly decides.** Synthesis proposes a draft; `AgentAssemble` decides (MEDULLA-DECISION-ENGINE-R036).

Data entities are in [data-model.md](data-model.md). The shortest served proof is in [quickstart.md](quickstart.md). Each seam to another repository has a document in `contracts/`.

### Standard process and decision vocabulary (R014)

Six core terms describe every process. `:ProcessModel` exists in `enterprise-v1.ttl`. `:ProcessStep` and `:ProcessGate` come from `MEDULLA-CQ-ORCH-R700.2`. R014.1 adds `:ProcessEventNode`, `:ProcessSequenceFlow` and `:ProcessLane` to `orchestration-v1.ttl`. `:ProcessStep`, `:ProcessGate` and `:ProcessEventNode` subclass the existing `:TaskStep`; flows and lanes subclass `:Concept`. `bpmn-v1.ttl` and `dmn-v1.ttl` hold the BPMN 2.0 and DMN element classes in the core namespace. Their `:Bpmn` and `:Dmn` IRIs are the ones the process engine connector pack already declares, so one term has one IRI. Each class subclasses one core term and neither file uses `owl:imports`. DMN rolls up to the new core terms `:DecisionLogic` and `:DecisionRule` under `:Policy`. The step binding properties (`:usesTool`, `:hasPrompt`, `:evaluatesDecision`, `:approverRole`, `:toolArguments`) are read by the process plan of `MEDULLA-CQ-ORCH-R704`. Both files and their shapes register as core schema sources beside the `archimate` entry of `crates/medulla-graph-core/src/graph/schema_sources.rs` (:889).

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
- Obsolete paths to delete: the write-time supersession of `register_version` (`crates/medulla-work-types/src/orchestration/mod.rs:378`) once R801.7 lands.

## Cross-repo seams

- Run-trace metric reads: `MEDULLA-ORCH-VOCAB-R010.1` provides, `ARBORA-OPS-R021.4.4` consumes. Contract: [arbora-run-metrics.md](contracts/arbora-run-metrics.md).
- Run-trace producers: `MEDULLA-ORCH-VOCAB-R013.2` and `MEDULLA-ORCH-VOCAB-R009.18.2` provide, `MERISTEM-HARNESS-CYCLE-R072` and `MERISTEM-HARNESS-CYCLE-R071` consume. Contract: [meristem-run-trace-producer.md](contracts/meristem-run-trace-producer.md).
- Harness session index: `MEDULLA-ORCH-VOCAB-R009.101.1` provides, `SCION-SOURCE-INGEST-R013` consumes. Contract: [scion-harness-session-ingest.md](contracts/scion-harness-session-ingest.md).
- Policy activation: `MEDULLA-ORCH-VOCAB-R806` provides, `ARBORA-OPS-R050.2` consumes. Contract: [arbora-policy-activation.md](contracts/arbora-policy-activation.md).
- Organization runtime and teams: `MEDULLA-ORCH-VOCAB-R750.751` provides, `MERISTEM-HARNESS-CYCLE-R074` consumes. Contract: [meristem-org-runtime.md](contracts/meristem-org-runtime.md).
