# MEDULLA-CQ-ORCH — Orchestration competency questions

Owner: medulla. Cross-repo IDs: ARBORA-FLEET-R052.4 (intent-surface probes, rollup R009), ARBORA-ACCEPTANCE-R006.7 (companion scenario), MERISTEM-HARNESS-CYCLE-R026.1 (consumer of the architecture view).
Public provenance: none; this spec holds the exit-gate questions of the orchestration competency.

## Delivery mapping

Component: ontology and packs for the seven question vocabularies; decision engine for the composed answer.

Depends-on: `MEDULLA-DECISION-ENGINE-R036` (assembly), `MEDULLA-DECISION-ENGINE-R126` and `R127` (coverage and guardrails), `MEDULLA-ORCH-VOCAB-R800.1` (workflow type), `MEDULLA-TYPED-PACKS-R104` (seed import).

## User Scenarios & Testing

### User Story 1 - Answer each of the seven task questions from the ontology (Priority: P1)

An operator asks how to complete task XYZ. The question decomposes into seven separately answerable questions: steps, staffing, template coverage, guardrails, per-agent bindings, workflow and graph shape. Each is one query over the ontology with its own shape and source authority.

**Acceptance Scenarios**:

1. **Given** the fixture graph of a question, **When** its query runs with the task bound, **Then** the rows equal the expected rows and every row carries its source.
2. **Given** a source that is not connected, **When** the source-authority mapping is read, **Then** the answer is the typed abstention and never an empty success.

### User Story 2 - One composed answer to "how can I execute task XYZ?" (Priority: P1)

A caller asks one question and gets all seven parts, each with premises and an evidence class, assembled by the existing assembly handler and not by a second planner.

**Acceptance Scenarios**:

1. **Given** task XYZ and two imported connector packs, **When** the caller sends `AgentExecutionAnswer`, **Then** all seven parts answer and `assembled_by` is `graph.assemble`.
2. **Given** a part whose source is removed, **When** the same request runs, **Then** that part abstains with its code while the other six answer.

### User Story 3 - Probe questions through the intent surface (Priority: P2)

An operator probes the engine through six plain questions. Medulla owns the engine answer and its fixture; arbora owns the intent resolution.

**Acceptance Scenarios**:

1. **Given** twelve nodes across three labels, **When** the node-count probe runs, **Then** one row per label is returned and the counts sum to twelve.
2. **Given** an empty library, **When** the agent-library probe runs, **Then** an empty list with the snapshot digest is returned, not an error.

### User Story 4 - Architecture views derived from the graph (Priority: P2)

A reader asks for the container view of a repository. The engine derives it from ingested code and diagram elements and cites the files that support each container.

**Acceptance Scenarios**:

1. **Given** a fixture repository of three packages and one diagram, **When** the view is read, **Then** three code containers, one diagram-only container and the call edges with counts return.
2. **Given** a scope with no ingested source, **When** the view is read, **Then** the read refuses with `VIEW_NO_SUPPORTING_SOURCE`.

### User Story 5 - Job, downtime and plan questions (Priority: P2)

An operator asks which components to load for a job, which downtime jobs run next, which query plan is best, how to use a connector and what a connector seeded. Each answer carries provenance from facts that ingest and engine jobs produced.

**Acceptance Scenarios**:

1. **Given** two imported packs, **When** the load-set question runs for a job, **Then** a bounded non-empty answer with one premise per row returns.
2. **Given** a seed imported twice with the catalog part changed, **When** the seed question runs, **Then** `catalog` is `replaced` and the other parts are `unchanged`.

### User Story 6 - Business processes compile to stored workflows (Priority: P3)

A harness submits a modeled process and a stored process model; the engine compiles or imports it into one versioned workflow that replays.

**Acceptance Scenarios**:

1. **Given** a process of 12 nodes, **When** it compiles twice, **Then** version 2 is written and a stale expected version is refused.
2. **Given** an imported EPC model, **When** it is exported again, **Then** the AND, OR and XOR gates survive.

### Edge Cases

- A source that is not connected: typed abstention, never an empty success.
- A part of the composed answer without data: its own abstention, no dropped field.
- A cycle without a loop gate, an unmapped process element, an unknown repository: named refusal, durable state unchanged.
- A request for another tenant's task, repository or process: refused or answered empty, never read.

## Requirements

- **FR-001**: each of questions 1 to 7 has an ontology gap closed, a shape, a source-authority mapping, a fixture and an acceptance test. Rows: `MEDULLA-CQ-ORCH-R001`, `MEDULLA-CQ-ORCH-R002`, `MEDULLA-CQ-ORCH-R003`, `MEDULLA-CQ-ORCH-R004`, `MEDULLA-CQ-ORCH-R005`, `MEDULLA-CQ-ORCH-R006`, `MEDULLA-CQ-ORCH-R007`.
- **FR-002**: the composed answer is produced by the assemble handler and adds no planner, router or topology chooser. Rows: `MEDULLA-CQ-ORCH-R008`, `MEDULLA-CQ-ORCH-R012`.
- **FR-003**: each probe is an engine-side acceptance test. Rows: `MEDULLA-CQ-ORCH-R009`.
- **FR-004**: the graph serves architecture views derived from ingested code and diagrams. Rows: `MEDULLA-CQ-ORCH-R010`.
- **FR-005**: the job, downtime and plan questions are answered from served reads. Rows: `MEDULLA-CQ-ORCH-R011`.
- **FR-006**: modeled processes compile to stored workflows and stored process models import and export. Rows: `MEDULLA-CQ-ORCH-R700`, `MEDULLA-CQ-ORCH-R701`.

## Success Criteria

- **SC-001**: all seven question rollups have passing acceptance children on a fixture graph. Rows: `MEDULLA-CQ-ORCH-R001.5`.
- **SC-002**: the composed answer carries seven parts with premises and an evidence class each. Rows: `MEDULLA-CQ-ORCH-R008.5`.
- **SC-003**: no refusal changes durable state. Rows: `MEDULLA-CQ-ORCH-R700.4`, `MEDULLA-CQ-ORCH-R701.4`.

## Assumptions

- A question is answered when one query over the ontology returns a correct, provenance-carrying answer from connected data; otherwise the answer is a typed abstention naming the missing source or capability.
- Virtual-first: the engine ingests a source's ontology and access contract and fetches data at query time; copying a source needs its own stated reason.
- `AgentTemplate` lives in `orchestration-v1.ttl`, and step order reuses the core `:precedes` property.

## Why a new spec

The search of the decision-engine, federated-query, unified-data-plane and typed-packs specs found no owner for the seven task questions as one exit gate: those specs own the mechanisms and this spec owns the questions, fixtures and composed envelope that prove them.

## Scope and interfaces

Owned: ontology gaps, shapes, source-authority mappings, fixtures and acceptance tests, the composed envelope, the architecture view, the workflow compiler and importer, and the job questions. Mechanisms are cited, not re-specified. Non-goals: a new planner, solver changes, and arbora tool implementation.

## Traceability

| Label | Rows | Design section | Test ID | Evidence |
|---|---|---|---|---|
| FR-001 | `MEDULLA-CQ-ORCH-R001` | plan.md Questions 1 to 7 | T-001 | PENDING |
| FR-002 | `MEDULLA-CQ-ORCH-R008` | plan.md Composed answer | T-003 | PENDING |
| FR-003 | `MEDULLA-CQ-ORCH-R009` | plan.md Probes | T-004 | PENDING |
| FR-004 | `MEDULLA-CQ-ORCH-R010` | plan.md Architecture views | T-005 | PENDING |
| FR-005 | `MEDULLA-CQ-ORCH-R011` | plan.md Job and plan questions | T-006 | PENDING |
| FR-006 | `MEDULLA-CQ-ORCH-R700` | plan.md Workflows | T-007 | PENDING |
