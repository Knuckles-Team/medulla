# MEDULLA-ORCH-VOCAB — Orchestration vocabulary

Component: Orchestration vocabulary

## Outcome

The graph answers how a task runs. It states which steps a task has and in what order, which agents staff a topology, which skills, tools, prompts and models bind to each step, and how each harness run performed. The vocabulary adds no planner, router or generator. `AgentAssemble` (`MEDULLA-DECISION-ENGINE-R036`) reads it for its task list and writes the committed result back.

## Scope

Owner: `medulla` (native Rust and ontology files). The ontology files are in `crates/medulla-core/ontology/`. Writers are the Agent Library projection worker, the topology projection, the assemble commit, the harness admission path and the repository index route. Cross-repo consumers: arbora (`ARBORA-OPS`), meristem (`MERISTEM-HARNESS-CYCLE`) and the connector SDK (`SCION-SOURCE-INGEST-R013`).

Non-goals: the engine never generates text, never executes an agent and never applies an adjustment. Arbora owns admission and the served door. Meristem owns agent execution.

Why a spec of its own: no other spec owns the terms, shapes and run-trace records that the seven part-queries of `MEDULLA-CQ-ORCH` need.

## User Scenarios & Testing

### User Story 1 - Ask what steps a task has and in which order (Priority: P1)

The user is an operator or agent that plans work. The outcome is that one graph query returns the ordered steps and hand-offs of a task.

**Acceptance scenarios**

1. **Given** an operator has a task with `:hasStep` steps ordered by `:precedes`, **When** the Q1 query runs over the graph, **Then** the four steps come back in order, and a planted `:precedes` cycle is rejected by `:TaskStepShape`.

Independent test: the bound tests of `MEDULLA-ORCH-VOCAB-R001.4`, `MEDULLA-ORCH-VOCAB-R006.6` run this story through a production entry point.

### User Story 2 - Staff a topology from agent kinds and the Agent Library (Priority: P1)

The user is an assembly caller. The outcome is that slots, kinds, templates, A2A members, best practices and bindings answer from one graph.

**Acceptance scenarios**

1. **Given** a topology template has slots that require an agent kind and a library with templates and an A2A card, **When** `AgentAssemble` reads the vocabulary, **Then** only candidates of the required kind fill a verifier slot, and the answer abstains when none is visible.

Independent test: the bound tests of `MEDULLA-ORCH-VOCAB-R002.5`, `MEDULLA-ORCH-VOCAB-R003.7`, `MEDULLA-ORCH-VOCAB-R005.6`, `MEDULLA-ORCH-VOCAB-R007.4` run this story through a production entry point.

### User Story 3 - Register and gate the vocabulary (Priority: P1)

The user is a maintainer of the ontology. The outcome is that every ontology file and shape file is composed by the engine and kept in step with its gate.

**Acceptance scenarios**

1. **Given** the orchestration shapes file is a core schema source, **When** the engine composes the core sources, **Then** the shapes load and a stale ledger entry fails the gate.

Independent test: the bound tests of `MEDULLA-ORCH-VOCAB-R008.1` run this story through a production entry point.

### User Story 4 - Ingest and read back harness run traces and steps (Priority: P1)

The user is a harness that runs agents. The outcome is that a trace, its step digests, its observation windows and its durable units are stored once and read back in order.

**Acceptance scenarios**

1. **Given** a harness sends a shape-valid trace with three steps through `add_node`, **When** the engine admits it, **Then** the trace and steps read back in index order, and a replay differs first at the changed step.

Independent test: the bound tests of `MEDULLA-ORCH-VOCAB-R009.15`, `MEDULLA-ORCH-VOCAB-R009.17`, `MEDULLA-ORCH-VOCAB-R013.3`, `MEDULLA-ORCH-VOCAB-R013.6` run this story through a production entry point.

### User Story 5 - Compare configurations by metrics and measured facts (Priority: P2)

The user is an evaluator that tunes harness configuration. The outcome is that per-model and per-task-class metrics and measured model facts close the loop to `HarnessConfig`.

**Acceptance scenarios**

1. **Given** fixture traces are ingested through `AddNode`, **When** a metric statement runs through `Method::Uql`, **Then** the success rate, cost per solved task and overflow rate come back per group, and an empty group abstains.

Independent test: the bound tests of `MEDULLA-ORCH-VOCAB-R010.1`, `MEDULLA-ORCH-VOCAB-R011.9`, `MEDULLA-ORCH-VOCAB-R803` run this story through a production entry point.

### User Story 6 - Index spec folders and policy as typed nodes (Priority: P2)

The user is a repository owner. The outcome is that spec files, requirement rows and policy sources become typed graph nodes behind review.

**Acceptance scenarios**

1. **Given** a scoped `IndexRepository` carries a fixture spec folder, **When** the repository route classifies the files, **Then** one task, goal, document, memory and policy node appear, each naming its rule, and an unknown file becomes a plain document.

Independent test: the bound tests of `MEDULLA-ORCH-VOCAB-R012.3`, `MEDULLA-ORCH-VOCAB-R012.6`, `MEDULLA-ORCH-VOCAB-R012.7.3` run this story through a production entry point.

### User Story 7 - Run an organization, teams and definitions (Priority: P2)

The user is an operator of an agent organization. The outcome is that roles, offers, twins, teams, prompt templates and workflow patterns are graph facts selected by named rules.

**Acceptance scenarios**

1. **Given** an organization chart, a team store and published definitions exist, **When** `recruit`, team scoring and template rendering run through the decision ladder, **Then** the answer names its ranking, and an edit creates a new revision without changing a published one.

Independent test: the bound tests of `MEDULLA-ORCH-VOCAB-R750.751`, `MEDULLA-ORCH-VOCAB-R751.751`, `MEDULLA-ORCH-VOCAB-R800.3`, `MEDULLA-ORCH-VOCAB-R616.2` run this story through a production entry point.

### User Story 8 - Keep harness components and lessons as versioned facts (Priority: P2)

The user is an improvement loop. The outcome is that component versions, adopted harnesses, contrasts and lessons keep their evidence and history.

**Acceptance scenarios**

1. **Given** two versions of a component exist with evidence runs, **When** the second is promoted and then rolled back, **Then** the pointer history shows both moves and the older version stays readable.

Independent test: the bound tests of `MEDULLA-ORCH-VOCAB-R801.7`, `MEDULLA-ORCH-VOCAB-R802.2` run this story through a production entry point.

### Edge Cases

- An unknown measurement stays unknown and never becomes zero or true.
- A refused write stores no partial node.
- A cross-tenant reference reads as absent and leaks no count.
- A duplicate requirement row id refuses that file and the other files continue.
- A version of a record that the decoder does not know fails closed.
- A stale review receipt or invariant generation keeps the prior policy set.

## Requirements

- **FR-001**: Every term, shape and import of the vocabulary exists in the named ontology file with its domain, range and constraint. Rows: `MEDULLA-ORCH-VOCAB-R001`, `MEDULLA-ORCH-VOCAB-R002`, `MEDULLA-ORCH-VOCAB-R003`, `MEDULLA-ORCH-VOCAB-R004`, `MEDULLA-ORCH-VOCAB-R005`, `MEDULLA-ORCH-VOCAB-R006`.
- **FR-002**: Assembly reads steps, kinds and bindings and writes the committed result back through the commit path. Rows: `MEDULLA-ORCH-VOCAB-R007`, `MEDULLA-ORCH-VOCAB-R616`.
- **FR-003**: The ontology sources are registered and the ledger stays in step. Rows: `MEDULLA-ORCH-VOCAB-R008`.
- **FR-004**: Run traces, steps, windows and durable units are admitted by shape and digest and read back in order. Rows: `MEDULLA-ORCH-VOCAB-R009`, `MEDULLA-ORCH-VOCAB-R013`.
- **FR-005**: Run-trace metrics and measured model facts are read and written through governed paths. Rows: `MEDULLA-ORCH-VOCAB-R010`, `MEDULLA-ORCH-VOCAB-R011`, `MEDULLA-ORCH-VOCAB-R803`.
- **FR-006**: Spec folders and policy sources become typed nodes; policy activates only behind review. Rows: `MEDULLA-ORCH-VOCAB-R012`, `MEDULLA-ORCH-VOCAB-R806`.
- **FR-007**: Organization, team, definition and workflow records are graph facts with named selection rules. Rows: `MEDULLA-ORCH-VOCAB-R750`, `MEDULLA-ORCH-VOCAB-R751`, `MEDULLA-ORCH-VOCAB-R800`, `MEDULLA-ORCH-VOCAB-R808`, `MEDULLA-ORCH-VOCAB-R809`.
- **FR-008**: Harness components and the improvement library keep versioned, evidenced history. Rows: `MEDULLA-ORCH-VOCAB-R801`, `MEDULLA-ORCH-VOCAB-R802`, `MEDULLA-ORCH-VOCAB-R810`.

## Success Criteria

- **SC-001**: Every closed set of the vocabulary (agent kinds, termination reasons, measured properties) is declared once and enforced by a shape. Rows: `MEDULLA-ORCH-VOCAB-R002.1`, `MEDULLA-ORCH-VOCAB-R009.1`, `MEDULLA-ORCH-VOCAB-R011.6`.
- **SC-002**: A planted `:precedes` or `swarm:feeds` cycle is rejected. Rows: `MEDULLA-ORCH-VOCAB-R001.3`, `MEDULLA-ORCH-VOCAB-R006.4`.
- **SC-003**: A refused run-trace write leaves the stored graph unchanged. Rows: `MEDULLA-ORCH-VOCAB-R009.15`, `MEDULLA-ORCH-VOCAB-R013.4`.
- **SC-004**: A metric group with no solved trace abstains and never reports zero cost. Rows: `MEDULLA-ORCH-VOCAB-R010.2`.
- **SC-005**: A measured fact without run evidence is refused. Rows: `MEDULLA-ORCH-VOCAB-R011.8`.
- **SC-006**: A replay of a stored trace names the first differing step. Rows: `MEDULLA-ORCH-VOCAB-R013.6`.
- **SC-007**: A policy rule that contradicts an invariant is never activated. Rows: `MEDULLA-ORCH-VOCAB-R012.7.3`.
- **SC-008**: A published definition revision is never changed by an edit. Rows: `MEDULLA-ORCH-VOCAB-R800.3`, `MEDULLA-ORCH-VOCAB-R800.4`.

## Assumptions

- Arbora supplies a verified principal and tenant on every served request.
- The reference host runs the nightly benchmark suite that measures each budget.
- Harness producers compute step digests with the one published recipe of `MEDULLA-ORCH-VOCAB-R013.2`.
- The generated-artifact ledger covers every generated artifact, each with its regeneration command and digest (`MEDULLA-ORCH-VOCAB-R008.2`).
