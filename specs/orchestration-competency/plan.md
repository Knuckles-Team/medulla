# MEDULLA-CQ-ORCH — Implementation plan

Governing spec: [spec.md](spec.md). Rows: [requirements.md](requirements.md).

## Summary

"How can I execute task XYZ?" is answered when it decomposes into seven separately answerable questions, each answered by one query over the ontology from connected data, and a composed answer carries all seven parts with provenance through the existing assembly handler. The live caller of the question vocabularies is `GraphSchemaSources::default()` in `crates/medulla-graph-core/src/graph/schema_sources.rs`; the composed answer enters at `dispatch_decision_methods` in `crates/medulla-server-router/src/dispatch/router/decision_plane.rs` beside `Method::AgentAssemble`. Reuse candidates found by searching the code first: `assemble_for`, `coverage_for` and `entailment_for` of `crates/medulla-h-decide`, the source-selection model of `crates/medulla-virtual-graph`, and the mining read path of `crates/medulla-h-mining`.

## Technical Context

- **Language/Version**: Rust 2021 edition on the pinned toolchain of `rust-toolchain.toml`; Turtle ontology and SHACL files; Python 3.14 (floor 3.12) for served tests.
- **Primary Dependencies**: `medulla-rdf` (parser, SPARQL), `medulla-shacl`, `medulla-reasoner`, `medulla-virtual-graph`, `medulla-h-decide`, `medulla-h-mining`, `medulla-h-graph` and the standard library.
- **Storage**: the engine graph store; workflows persist as versioned graph content through the mutation gateway. No second store.
- **Testing**: `crates/medulla-rdf/tests/orchestration_competency.rs` for questions 1 to 7, the unit tests named by the rows, and served L3 and L4 tests under `tests/`.
- **Target Platform**: Linux x86_64 and aarch64 hosts.
- **Project Type**: Library crates, ontology files and server handler crates.
- **Performance Goals**: the nightly groups `architecture_view`, `process_compile` and `process_import` of `MEDULLA-DECISION-ENGINE-R178.1` hold the budgets stated in rows R010.6, R700.6 and R701.6.
- **Constraints**: no second planner; every refusal has a stable code; reads write nothing; no version suffix in a name.
- **Scale/Scope**: the leaf rows of `requirements.md`, spread over 6 user stories and 6 `FR-` labels.

**Knob convention.** An engine knob named `knob x.y` in this spec's rows is a typed field of the server configuration, read from the command line or from the environment variable `MEDULLA_<NAME>` (the dotted name upper-cased, dots as underscores) and bounds-checked at startup: a value outside the stated range refuses startup with the knob name. Decision knobs are the exception and live only in the `MEDULLA-DECISION-ENGINE-R179` registry. Rows do not restate this.

## Constitution Check

- **R-I. The engine is native Rust and the only fact store**: pass. Every row lands in a Rust crate, an ontology file or an engine test.
- **R-II. Every wait has a deadline**: pass. No row adds a wait.
- **R-III. Every row sets budgets and names a benchmark**: pass. Rows R010.6, R700.6 and R701.6 name the nightly groups.
- **R-IV. Crates are small, layered and cut over hard**: pass. Rows name post-split crates; no handler crate calls a sibling handler crate.
- **Organization standards**: pass. Rows follow the spec standard and the ecosystem boundaries.

## Project Structure

```text
crates/medulla-core/ontology/orchestration-v1.ttl, capability-v1.ttl, swarm_topology-v1.ttl  existing
crates/medulla-core/ontology/orchestration-v1.shapes.ttl, swarm_topology-v1.shapes.ttl      existing
crates/medulla-core/tests/fixtures/orchestration_competency/q1..q7/                         existing
crates/medulla-rdf/tests/orchestration_competency*.rs                                       existing
crates/medulla-decision-outcome-types/src/decision/execution_answer.rs                      existing
crates/medulla-types/src/architecture_view.rs                                               existing
crates/medulla-h-decide/src/decide/execution_answer.rs                                      new
crates/medulla-h-mining/src/mining/architecture.rs                                          new
crates/medulla-h-graph/src/graph_ops/process_compile.rs                                     new
crates/medulla-core/src/{architecture_view,process_compile,process_import}.rs               new
crates/medulla-types/src/process_import.rs                                                  new
tests/orchestration/, tests/competency/                                                     new served tests
```

## Phase 0: Research

No unknowns. The two earlier open questions are resolved by the code: `AgentTemplate` is declared in `orchestration-v1.ttl`, and step order reuses the core `:precedes` property.

## Phase 1: Design

### Layer model

| Layer | Contents | Read by |
|---|---|---|
| L0 | Sources and ontologies | Q1, Q4, composed answer |
| L1 | Components: tools, skills, prompts, model profiles | Q3, Q4, Q5, composed answer |
| L2 | Agents and the Agent Library | Q2, Q3, Q5, composed answer |
| L3 | Agent graphs | Q1, Q2, Q6, Q7, composed answer |
| L4 | Harness port | Q6, composed answer |
| L5 | Runs, outcomes and feedback | Q1, Q2, Q6, composed answer |

Composition is the engine's decision functionality: `graph.assemble()` (`MEDULLA-DECISION-ENGINE-R036`) lowers typed tasks and capabilities to a bounded 0-1 program and returns a certified assembly or a typed abstention. Cross-source answers use federated query and foreign sources. A question is answered when one query over the ontology returns a correct, provenance-carrying answer; otherwise the answer is a typed abstention.

### Questions 1 to 7

| Question | Terms (as built) | Shape | Fixture source | Rows |
|---|---|---|---|---|
| Q1 steps | `:TaskStep`, `:hasStep`, core `:precedes` | `TaskStepShape` | task-decomposition | R001 |
| Q2 staffing | `:AgentKind`, `:hasAgentKind`, `swarm:requiresAgentKind` | swarm slot shapes | topology-templates | R002 |
| Q3 template coverage | `:AgentTemplate`, `:AgentLibrary`, `:memberOfLibrary` | `LibraryMemberShape` | agent-library | R003 |
| Q4 guardrails | `:BestPractice`, `:appliesToTaskClass` | `RuleScopeShape` | guardrail-rules | R004 |
| Q5 bindings | `:ModelProfile`, `:needsSkill`, `:stepAssignedAgent` | `StepBindingShape` | assembly | R005 |
| Q6 workflow | `:handsOffTo` | `HandOffShape` | workflow-store | R006 |
| Q7 DAG | `swarm:feeds` | `FeedsAcyclicShape`, `FeedsSingleRootShape` | topology-templates | R007 |

Each question has a fixture directory with `fixture.ttl`, `query.rq` and `expected.tsv`; the acceptance test asserts that every row carries its source and that an unconnected source yields the typed abstention.

### Composed answer

`AgentExecutionAnswer` returns `ExecutionAnswer{task_iri, parts{steps, staffing, coverage, guardrails, bindings, workflow, dag}, assembled_by, certificate_digest}`. Each part is an answer or a typed abstention with provenance and an evidence class. The handler runs `assemble_for` once, reads coverage and guardrails through their existing functions and runs the fixture queries of Q1, Q6 and Q7. It adds no planner. Rows R008 and R012.

### Probes

Six probe questions are answered by existing engine paths (`NlQuery`, `Decide`, `AgentLibrary`, `Discover`). Medulla owns the engine answer and its fixture; arbora owns intent resolution and proves the same answers through the intent tools. Row R009.

### Architecture views

Data flow: `Method::ArchitectureView` → `crates/medulla-h-mining/src/mining.rs::try_handle` → `handle_architecture_view` (new) → `derive_architecture_view` (new, `crates/medulla-core/src/architecture_view.rs`) → `ArchitectureView` with containers, edges and support paths. `medulla-h-mining` depends on `medulla-core` for the derivation (a downward dependency; the layering holds). The view has its own read method; `handle_impact` serves risk propagation and is not the call site. Failure behaviour: a scope with no supporting source returns a typed refusal; a diagram element with no matching package stays a container with empty support. Decision: derive on read from the graph; a stored diagram per repository goes stale at the next ingest. Row R010.

### Job and plan questions

| Question | Mechanism |
|---|---|
| load set for job J | `MEDULLA-DECISION-ENGINE-R176` |
| next downtime jobs | `MEDULLA-DURABLE-KERNEL-R101` |
| best virtual plan across two connectors | `MEDULLA-FEDERATED-QUERY-R091` |
| how and when to use connector X | `MEDULLA-DECISION-ENGINE-R177` |
| seed facts and changed parts of connector X | `MEDULLA-TYPED-PACKS-R104` |

Call path: arbora `find` → served `Decide` or SPARQL → the mechanism row → answer with provenance. Row R011.

### Workflows

A modeled process compiles deterministically into one `WorkflowPattern`, stored as versioned graph content that replays by recomputing its digest; a stored process model imports to and exports from the same workflow shape. The workflow type belongs to `MEDULLA-ORCH-VOCAB-R800.1`. Rows R700 and R701.

## Phase 2: Task approach

`tasks.md` has one line per leaf row. Producer first: ontology terms, shapes and authority mappings (US1), then the composed envelope (US2), probes (US3), architecture view (US4), job questions (US5) and workflows (US6). The first three rows of each question are marked parallel because they touch disjoint files.

## Complexity Tracking

None.

## Build contract

Gates for any change under these rows: `cargo test -p medulla-rdf --test orchestration_competency`, `cargo test -p medulla-decision-outcome-types execution_answer`, `cargo clippy --workspace --all-targets -- -D warnings`, the repository complexity, duplication, dead-code and KISS gates with their committed thresholds, and the spec gate set (`pre-commit run --config .config/pre-commit.yaml --hook-stage pre-push`). No scanner gaming and no duplicate implementation: the composed answer reuses `assemble_for`, and the workflow compiler reuses the mutation gateway.

## Production caller chain

- Question vocabularies: `GraphSchemaSources::default()` (`crates/medulla-graph-core/src/graph/schema_sources.rs:207`) → `current_core_catalog` (:656) → `core_specs` modules `orchestration` (:930), `capability` (:740), `swarm-topology` (:1047) and the shape modules `orchestration-shapes` (:940) and `swarm-topology-shapes` (:1055).
- Composed answer: `crates/medulla-server-router/src/dispatch/router/decision_plane.rs::dispatch_decision_methods` → `crates/medulla-h-decide/src/decide/execution_answer.rs::handle_execution_answer` (new) → `assemble_for` (`crates/medulla-h-decide/src/decide/assemble.rs:46`).
- Architecture view: `crates/medulla-server-router/src/dispatch/router.rs::dispatch_request_method_inner` (:208) → `dispatch_graph_op` (`crates/medulla-server-graph/src/dispatch/graph_pipeline.rs:68`) → `crates/medulla-h-mining/src/mining.rs::try_handle` (:80).
- Workflows: the same graph-op chain → `crates/medulla-h-graph/src/graph_ops/terminal.rs::try_handle_graph_domains` (:14) → the new `process_compile` handler.
- Obsolete paths to delete: none.

## Cross-repo seams

- arbora intent-surface probes: `ARBORA-FLEET-R052.4` (consumer rows R009.1 to R009.6) and `ARBORA-FLEET-R056`; contract `contracts/arbora-intent-probes.md`.
- arbora acceptance scenario for the execution plan: `ARBORA-ACCEPTANCE-R006.7` (consumer row R012); contract `contracts/arbora-execution-scenario.md`.
- meristem consumer of the architecture view: `MERISTEM-HARNESS-CYCLE-R026.1` (provider row R010.3); contract `contracts/meristem-architecture-view.md`.
- scion-sdk contract vocabulary: `SCION-CONNECTOR-CONTROL-R916.3` provides the served `contracts.ttl` (consumer rows R012.1 and R012.2); contract `contracts/scion-surface-contracts.md`.
