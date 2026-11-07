# MEDULLA-CQ-ORCH — Data model

This file describes the ontology terms, record types and nodes that the rows of this spec add or reuse.

## Entities

### Question vocabularies

- Questions 1 to 7 reuse existing terms: `:TaskStep`, `:hasStep`, `:AgentKind`, `:hasAgentKind`, `:AgentTemplate`, `:AgentLibrary`, `:memberOfLibrary`, `:ModelProfile`, `:needsSkill`, `:stepAssignedAgent` and `:handsOffTo` in `orchestration-v1.ttl`; `:BestPractice` and `:appliesToTaskClass` in `capability-v1.ttl`; `swarm:feeds` and `swarm:requiresAgentKind` in `swarm_topology-v1.ttl`.
- Owner: `crates/medulla-core/ontology/`; rows `MEDULLA-CQ-ORCH-R001.1` to `R007.1`.

### ExecutionAnswer

- Fields: schema version, task IRI, seven parts (steps, staffing, coverage, guardrails, bindings, workflow, dag), `assembled_by` and `certificate_digest`.
- Owner: `crates/medulla-decision-outcome-types/src/decision/execution_answer.rs`, row `MEDULLA-CQ-ORCH-R008.1`.
- Relation: each part is an answer or an abstention with provenance premises and an evidence class; the digest is stable across calls.

### ArchitectureView

- Fields: containers (name, technology, support paths) and edges (from, to, kind, call count), plus `ViewScope` and `ViewAbstained`.
- Owner: `crates/medulla-types/src/architecture_view.rs`, rows `MEDULLA-CQ-ORCH-R010.1` to `R010.5`.
- Relation: derived on read from ingested code and diagram elements; never stored.

### Workflow and imported process

- `WorkflowPattern` is owned by `MEDULLA-ORCH-VOCAB-R800.1`; this spec adds `:ProcessStep`, `:ProcessGate`, `:gateKind` and `:loopGate` (R700.2) and the types `ImportedProcess`, `GateKind`, `ImportedGate` and `ImportError` in the new `crates/medulla-types/src/process_import.rs` (R701.1).
- Relation: a stored workflow node `workflow:<process_iri>:v<n>` links to its process by `:compiledFrom` and to an imported model by `importedFrom`.

### Refusal tables

- The composed answer, the architecture view, the compiler and the importer each declare a `REFUSALS` const collected by the uniqueness test of `MEDULLA-DECISION-ENGINE-R010.100.1`.

## State machine types

- Rows `MEDULLA-CQ-ORCH-R810`, `MEDULLA-CQ-ORCH-R811`: each machine type stores one state field and exposes one `apply` (or `step`) function that returns `TransitionRefused` for an illegal transition.
- Deleted fields: the dual-truth flags and literals named in the rows (for example `terminal`, `tombstone`, `reclaimed`, `cancel_requested`, `active`, `revoked`, `destination_installed`).

### State machine fields

- `BrokerMessageMachine`. States: `Pending`, `Claimed`, `Acked`, `Expired`, `DeadLettered`. Persisted state: `status` (stored text of `MessageState`, record built at `crates/medulla-core/src/broker.rs:344`). `lease_until` and `deliver_at` are data of their state.
- `AnalyticsJobMachine`. States: `Submitted`, `Running`, `Publishing`, `CancelRequested`, `Backoff`, `Succeeded`, `Failed`, `Cancelled`. Persisted state: `state` (enum `JobState`, `crates/medulla-jobs/src/model.rs:167`). the lease belongs to Running, Publishing and CancelRequested. `not_before` belongs to Backoff.
