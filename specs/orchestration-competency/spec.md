# MEDULLA-CQ-ORCH — Orchestration competency questions

Owner: medulla. Cross-repo IDs: ARBORA-FLEET-R052.4 (intent-surface probes, rollup R009), ARBORA-ACCEPTANCE-R006.7 (companion scenario), MERISTEM-HARNESS-CYCLE-R026.1 (consumer of the architecture view).
Public provenance: none; this spec holds the exit-gate questions of the orchestration competency.

## Delivery mapping

Component: ontology and packs for the seven question vocabularies; decision engine for the composed answer.

Depends-on: `MEDULLA-ORCH-VOCAB-R014` (process vocabulary), `MEDULLA-SRC-AUTH-R012` (two-way sync), `MEDULLA-DECISION-ENGINE-R134.111.14` (control steps), `MEDULLA-DECISION-ENGINE-R036` (assembly), `MEDULLA-DECISION-ENGINE-R126` and `R127` (coverage and guardrails), `MEDULLA-ORCH-VOCAB-R800.1` (workflow type), `MEDULLA-TYPED-PACKS-R104` (seed import).

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

### User Story 6 - Every process source meets in one canonical workflow (Priority: P2)

A harness submits a modeled process, a BPMN document or the canonical form of an ARIS EPC model. The engine compiles or admits it into one versioned canonical workflow that replays. BPMN and EPC are interchangeable: the same process authored in either gives one workflow and one agent DAG, and an export to a format that cannot hold a construct lists the loss instead of dropping it.

**Acceptance Scenarios**:

1. **Given** a process of 12 nodes, **When** it compiles twice, **Then** version 2 is written and a stale expected version is refused.
2. **Given** an imported EPC model, **When** it is exported again, **Then** the AND, OR and XOR gates survive.
3. **Given** the invoice approval process authored once as BPMN and once as an ARIS AML export, **When** both are imported and planned, **Then** the two workflows share one canonical digest and the two plans share one plan digest.
4. **Given** a workflow with a boundary event, **When** it is exported strictly to EPC, **Then** the export refuses `PROCESS_EXPORT_LOSSY` and lists the loss.

### User Story 7 - Process documents round-trip and sync with the process engine (Priority: P2)

An operator imports a BPMN 2.0 or DMN document and exports it again unchanged. The graph stays the system of record, and the process engine receives deployments through its connector.

**Acceptance Scenarios**:

1. **Given** a BPMN fixture with lanes, gateways, a boundary event and an extension element, **When** it is imported, exported and imported again, **Then** the canonical model digests are equal.
2. **Given** a synced definition changed on both sides, **When** sync runs, **Then** one conflict is recorded, the graph wins and one deploy proposal is queued.

### User Story 8 - A process model becomes an agent DAG plan (Priority: P2)

A caller asks for the plan of a stored workflow. The plan reads only the canonical form. Each step binds by its canonical kind to a tool, a prompt, a decision or an approval; each gate becomes a branch or join.

**Acceptance Scenarios**:

1. **Given** a workflow with four task kinds and an exclusive gateway, **When** `Decide` runs `ProcessPlan`, **Then** the plan holds each step kind and a two-arm branch.
2. **Given** an unbound task with no tool above the floor, **When** a caller asks, **Then** the plan abstains with `PROCESS_STEP_UNBOUND`.

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
- **FR-006**: modeled processes compile to stored workflows, and every process source (BPMN process engine, ARIS EPC repository) imports to and exports from one canonical workflow under stated round-trip and loss rules. Rows: `MEDULLA-CQ-ORCH-R700`, `MEDULLA-CQ-ORCH-R701`.
- **FR-007**: BPMN 2.0 and DMN documents import natively and export round-trip safe, and process models sync two ways with every process source. Rows: `MEDULLA-CQ-ORCH-R702`, `MEDULLA-CQ-ORCH-R703`.
- **FR-008**: a stored canonical workflow becomes one canonical agent DAG plan with tool, agent, decision, approval, branch and join steps, built from the canonical form only. Rows: `MEDULLA-CQ-ORCH-R704`.
- **FR-009**: the architecture model exports and imports in the ArchiMate Open Exchange Format and syncs with the architecture-modelling connector. Rows: `MEDULLA-CQ-ORCH-R705`.

## Success Criteria

- **SC-001**: all seven question rollups have passing acceptance children on a fixture graph. Rows: `MEDULLA-CQ-ORCH-R001.5`.
- **SC-002**: the composed answer carries seven parts with premises and an evidence class each. Rows: `MEDULLA-CQ-ORCH-R008.5`.
- **SC-003**: no refusal changes durable state. Rows: `MEDULLA-CQ-ORCH-R700.4`, `MEDULLA-CQ-ORCH-R701.4`, `MEDULLA-CQ-ORCH-R702.12`, `MEDULLA-CQ-ORCH-R703.4`, `MEDULLA-CQ-ORCH-R705.8`.
- **SC-004**: import, export and import again keep the canonical digest for BPMN, DMN and ArchiMate documents. Rows: `MEDULLA-CQ-ORCH-R702.14`, `MEDULLA-CQ-ORCH-R702.15`, `MEDULLA-CQ-ORCH-R705.10`.
- **SC-005**: every task of a process plan has a binding or a named abstention. Rows: `MEDULLA-CQ-ORCH-R704.8`.
- **SC-006**: one process authored as BPMN and as EPC gives one canonical digest and one plan digest. Rows: `MEDULLA-CQ-ORCH-R701.9`, `MEDULLA-CQ-ORCH-R704.9`.
- **SC-007**: no export drops a construct silently; every loss is listed, and a strict export refuses. Rows: `MEDULLA-CQ-ORCH-R701.8`.

## Assumptions

- A question is answered when one query over the ontology returns a correct, provenance-carrying answer from connected data; otherwise the answer is a typed abstention naming the missing source or capability.
- Virtual-first: the engine ingests a source's ontology and access contract and fetches data at query time; copying a source needs its own stated reason.
- `AgentTemplate` lives in `orchestration-v1.ttl`, and step order reuses the core `:precedes` property.

## Why a new spec

The search of the decision-engine, federated-query, unified-data-plane and typed-packs specs found no owner for the seven task questions as one exit gate: those specs own the mechanisms and this spec owns the questions, fixtures and composed envelope that prove them.

## Scope and interfaces

Owned: ontology gaps, shapes, source-authority mappings, fixtures and acceptance tests, the composed envelope and the job questions. Also owned: the architecture view and its ArchiMate exchange, and the workflow compiler and importer. Also owned: the canonical workflow form and the process source contract, BPMN and DMN document exchange, the process sync adapter and the process-to-plan translation. The EPC format mapping belongs to the ARIS connector (`CONN-ARIS-MCP`). Mechanisms are cited, not re-specified. Non-goals: a new planner, solver changes, and arbora tool implementation.

## Traceability

| Label | Rows | Design section | Test ID | Evidence |
|---|---|---|---|---|
| FR-001 | `MEDULLA-CQ-ORCH-R001` | plan.md Questions 1 to 7 | T-001 | PENDING |
| FR-002 | `MEDULLA-CQ-ORCH-R008` | plan.md Composed answer | T-003 | PENDING |
| FR-003 | `MEDULLA-CQ-ORCH-R009` | plan.md Probes | T-004 | PENDING |
| FR-004 | `MEDULLA-CQ-ORCH-R010` | plan.md Architecture views | T-005 | PENDING |
| FR-005 | `MEDULLA-CQ-ORCH-R011` | plan.md Job and plan questions | T-006 | PENDING |
| FR-006 | `MEDULLA-CQ-ORCH-R700`, `MEDULLA-CQ-ORCH-R701` | plan.md Workflows | T-007 | PENDING |
| FR-007 | `MEDULLA-CQ-ORCH-R702`, `MEDULLA-CQ-ORCH-R703` | plan.md Process documents and engine sync | T-042 | PENDING |
| FR-008 | `MEDULLA-CQ-ORCH-R704` | plan.md Process plans | T-066 | PENDING |
| FR-009 | `MEDULLA-CQ-ORCH-R705` | plan.md Architecture exchange | T-075 | PENDING |
