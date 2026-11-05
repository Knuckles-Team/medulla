# MEDULLA-ORCH-VOCAB — Data model

This file names the records and graph terms that the rows add. Each entity has one owner file.

## Entities

### Ontology terms

- Owner: `crates/medulla-core/ontology/orchestration-v1.ttl`, `swarm_topology-v1.ttl`, `capability-v1.ttl` and `sdd-v1.ttl`.
- Rule: a term has one declaration with its domain and range; shapes in the matching `.shapes.ttl` file constrain it.

### RunTrace and StepRecord

- Fields: harness counters, `:terminationReason`, `:runModel`, `:runTaskClass`, `:appliedHarnessConfig`; each step has `step_index`, `kind` and three digests.
- Owner: `crates/medulla-result-types/src/outcome_bundle.rs`.
- Rule: digests come from the one `step_digest` recipe.

### RunObservationWindow and DurableUnit

- Fields: revisioned window with measured-or-unknown fields; a unit with `run_id`, closed `status` and artifact refs.
- Owner: `crates/medulla-types/src/run_observation.rs` and `crates/medulla-types/src/durable_unit.rs`.
- Relation: both link to a `:RunTrace`.

### MeasuredModelFact

- Fields: model reference, closed `:measuredProperty`, value, run evidence.
- Relation: a newer fact links to the older one by `:measuredSupersedes`; currency is a champion pointer.

### Definition records

- Records: `PromptTemplate`, `WorkflowPattern`, `OrgChart`, `AgentTwin`, `AgentInstance`, `TeamConfig`, `CapabilityGap`.
- Owner: `crates/medulla-work-types/src/orchestration/`.
- Relation: a stored definition is a revision of the subject named in `MEDULLA-DECISION-ENGINE-R805.10`.

### Harness components and lessons

- Records: `HarnessComponentVersion`, `AdoptedHarness`, `HarnessContrast`, `HarnessLesson`.
- Owner: `crates/medulla-work-types/src/orchestration/mod.rs` and `improvement.rs`.
- Relation: a newer version supersedes an older one by edge; superseded versions stay readable.

### PolicyRule and PolicyCandidate

- Fields: rule id, scope, closed severity, source reference with span, clause digest, status.
- Owner: `crates/medulla-types/src/policy_document.rs`.
- Relation: activation moves the `GovernedSet` pointer of the repository.
