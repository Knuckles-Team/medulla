# MEDULLA-INGEST-WORKER — Durable ingest worker

Component: ingest worker queue and source admission

## Outcome

An operator runs one or many ingest worker processes that drain queued ingestion jobs into the graph, survive worker crashes, and expose backlog so producers slow down. Connector batches of every kind (records, access, groups, enumeration, containers, proposals, facts, transcripts) enter through one served method and one admission path, and each batch kind has a typed receipt.

## Scope

Owner: `medulla` (native Rust). The queue, lanes, retry and dead-letter policy, depth read and metrics live in `crates/medulla-h-ingest`, with types in `crates/medulla-source-types` and `crates/medulla-types`. The runner process belongs to the connector SDK (`SCION-SOURCE-INGEST-R010`). Arbora hosts and supervises it. Durable queue primitives belong to `MEDULLA-DURABLE-KERNEL-R072`.

Non-goals: the engine runs no source effect, holds no source credential and calls no language model. Kafka is not used. The in-process Python worker pool and the analytics worker are out of scope.

## User Scenarios & Testing

### User Story 1 - Drain queued ingest jobs and survive a worker crash (Priority: P1)

The user is an operator of ingest workers. The outcome is that every job completes once, or lands in the dead letter, and producers slow down when the backlog is high.

**Acceptance scenarios**

1. **Given** a producer enqueues the same job twice, **When** a worker claims and acknowledges it, **Then** exactly one task exists and `deduplicated` is true on the second receipt.
2. **Given** a worker dies before it acknowledges a claimed job, **When** the lease lapses, **Then** the job redelivers with a higher attempt and completes once.
3. **Given** a job fails past `max_attempts`, **When** the last failure is acknowledged, **Then** one dead-letter record exists and no further delivery happens.
4. **Given** a queue is at `max_queue_depth`, **When** a producer enqueues, **Then** the engine refuses with `INGEST_QUEUE_FULL` and the depth is unchanged.

Independent test: `tests/ingest/test_ingest_queue_served.py::test_enqueue_claim_fail_dead_letter_journey` runs this story through the served methods.

### User Story 2 - Declare data-prep capabilities (Priority: P2)

The user is an arbora host. The outcome is a replaced, validated declaration that the data-prep kernel reads before it builds a plan.

**Acceptance scenarios**

1. **Given** a host sends a valid declaration, **When** it sends an equal one again, **Then** the second call writes nothing and reports one unchanged.
2. **Given** a declaration with a path-like value, **When** the engine validates it, **Then** it refuses with `DATA_PREP_DECLARATION_INVALID` and the stored set is unchanged.

Independent test: `tests/ingest/test_data_prep_declaration_served.py::test_declare_replace_and_refuse` runs this story.

### User Story 3 - Ingest documents and transcripts as knowledge atoms (Priority: P1)

The user is a connector worker and an agent that reads context. The outcome is a document or transcript that becomes atoms with mandatory provenance, and a context read that cites them.

**Acceptance scenarios**

1. **Given** a worker sends a document batch, **When** the engine normalizes, deduplicates and decomposes it, **Then** each atom stores a provenance key that resolves to the document.
2. **Given** a document fails normalization, **When** the route commits the quarantine envelope, **Then** no accepted projection changes and the checkpoint stays as it was.
3. **Given** a near-duplicate document, **When** it is admitted, **Then** the engine links it with `NEAR_DUPLICATE_OF` and stores no atoms for it.

Independent test: `tests/ingest/test_knowledge_atoms_served.py::test_ingest_then_context_cites_provenance` runs this story.

### User Story 4 - Admit induced ontology proposals and grounded facts (Priority: P2)

The user is a connector worker that runs a language model. The outcome is a proposal or fact batch that the engine judges before it becomes part of the graph.

**Acceptance scenarios**

1. **Given** a passing and a failing proposal, **When** the engine judges them, **Then** the passing one is `Admitted`, the failing one stays `Draft` and takes no instances.
2. **Given** a fact batch with a swapped direction, an ungrounded fact and a duplicate, **When** the engine grounds them, **Then** direction is repaired, the ungrounded fact stays a claim and one unit dedupes.

Independent test: `tests/ingest/test_ontology_induction_served.py::test_passing_proposal_admitted_failing_stays_draft` runs the first scenario.

### User Story 5 - Keep source access, deletions, hierarchy and versions true (Priority: P1)

The user is a connector operator. The outcome is that the held copy follows the source: governance, permissions, deletions, containers and stream versions.

**Acceptance scenarios**

1. **Given** a record carries `_governance`, **When** it is admitted, **Then** its policy and access properties exist in the same graph version as the node.
2. **Given** a group batch removes a member, **When** the member reads, **Then** the read is denied without a record rewrite.
3. **Given** a complete enumeration omits a record, **When** the grace window passes, **Then** the sweep deletes it; an incomplete enumeration never deletes.
4. **Given** two batches on one stream, **When** the second names the first digest, **Then** it commits without `STALE_CONTENT_VERSION`.

Independent test: `tests/ingest/test_source_governance_served.py::test_policy_and_access_exist_with_the_node` runs the first scenario.

### Edge Cases

- A forged tenant or a target that holds a credential is refused and writes no task.
- A job that kills its worker reaches the dead letter after `max_attempts` claims.
- A redelivered drain page below the stored page index is dropped and never double-ingests.
- A batch whose kind does not match its extension is refused whole.
- An encrypted, oversize, unsupported or corrupt file quarantines with a reader code and changes no projection.
- An atom without a provenance key is refused and not stored.
- A restricted record whose access snapshot is stale fails closed at read.
- A plan that would delete more than 20 percent of a source refuses unless an admin override lifts the guard.

## Requirements

- **FR-001**: Jobs enqueue once with a deterministic id, claim with a lease, acknowledge after processing and redeliver when a worker dies. Rows: `MEDULLA-INGEST-WORKER-R001.4`, `MEDULLA-INGEST-WORKER-R001.12`, `MEDULLA-INGEST-WORKER-R001.14`.
- **FR-002**: Hydration jobs use their own queue and lane, and one pool still serves general work. Rows: `MEDULLA-INGEST-WORKER-R001.10`, `MEDULLA-INGEST-WORKER-R604.3`, `MEDULLA-INGEST-WORKER-R604.4`.
- **FR-003**: Failures retry with jittered backoff and dead-letter after the ceiling. Rows: `MEDULLA-INGEST-WORKER-R001.7`, `MEDULLA-INGEST-WORKER-R001.8`, `MEDULLA-INGEST-WORKER-R001.9`.
- **FR-004**: Queue depth is readable, producers are refused above the limit and metrics are exported. Rows: `MEDULLA-INGEST-WORKER-R001.11`, `MEDULLA-INGEST-WORKER-R001.13`, `MEDULLA-INGEST-WORKER-R604.5`.
- **FR-005**: A chunked drain is a chain of bounded page tasks with a routed graph. Rows: `MEDULLA-INGEST-WORKER-R605.2`, `MEDULLA-INGEST-WORKER-R605.3`, `MEDULLA-INGEST-WORKER-R605.4`.
- **FR-006**: One materialization path stamps provenance on every connector record. Rows: `MEDULLA-INGEST-WORKER-R606.2`, `MEDULLA-INGEST-WORKER-R606.3`, `MEDULLA-INGEST-WORKER-R606.4`.
- **FR-007**: Documents and transcripts become atoms or quarantine without partial state. Rows: `MEDULLA-INGEST-WORKER-R003.8`, `MEDULLA-INGEST-WORKER-R003.10.1`, `MEDULLA-INGEST-WORKER-R003.14`.
- **FR-008**: Readers produce typed fragments with span provenance. Rows: `MEDULLA-INGEST-WORKER-R700.1`, `MEDULLA-INGEST-WORKER-R700.3`, `MEDULLA-INGEST-WORKER-R700.4`.
- **FR-009**: Proposals and facts are judged and grounded before admission. Rows: `MEDULLA-INGEST-WORKER-R002.3`, `MEDULLA-INGEST-WORKER-R701.3`, `MEDULLA-INGEST-WORKER-R701.7`.
- **FR-010**: Near duplicates link instead of storing twice. Rows: `MEDULLA-INGEST-WORKER-R702.2`, `MEDULLA-INGEST-WORKER-R702.3`, `MEDULLA-INGEST-WORKER-R702.4`.
- **FR-011**: Source governance, permissions, deletions and containers follow the source. Rows: `MEDULLA-INGEST-WORKER-R900.2`, `MEDULLA-INGEST-WORKER-R902.2`, `MEDULLA-INGEST-WORKER-R903.2`, `MEDULLA-INGEST-WORKER-R904.2`.
- **FR-012**: Source content versions chain per stream. Rows: `MEDULLA-INGEST-WORKER-R910.1`, `MEDULLA-INGEST-WORKER-R910.2`, `MEDULLA-INGEST-WORKER-R910.4`.
- **FR-013**: Data-prep declarations replace atomically and refuse unknown shapes. Rows: `MEDULLA-INGEST-WORKER-R001.1007`, `MEDULLA-INGEST-WORKER-R001.1009`.

## Success Criteria

- **SC-001**: Killing a worker mid-job results in exactly one completed job and no lost job. Rows: `MEDULLA-INGEST-WORKER-R001.12`, `MEDULLA-INGEST-WORKER-R001.14`.
- **SC-002**: A queue at its depth limit refuses every producer with `INGEST_QUEUE_FULL` and keeps its depth. Rows: `MEDULLA-INGEST-WORKER-R001.11`.
- **SC-003**: Every stored atom cites a provenance key that resolves to its document. Rows: `MEDULLA-INGEST-WORKER-R003.3`, `MEDULLA-INGEST-WORKER-R003.6`.
- **SC-004**: A second batch on one stream commits with no stale-version refusal. Rows: `MEDULLA-INGEST-WORKER-R910.3`.
- **SC-005**: A group change applies at read without a record rewrite. Rows: `MEDULLA-INGEST-WORKER-R902.3`.
- **SC-006**: Each nightly bench meets its stated service level. Rows: `MEDULLA-INGEST-WORKER-R604.6`, `MEDULLA-INGEST-WORKER-R606.6`.

## Assumptions

- Arbora supplies a verified principal and tenant on every served request.
- The connector SDK runner (`SCION-SOURCE-INGEST-R010`) follows the engine replies and holds all source credentials.
- The durable task queue, worker registry and scheduler config are delivered by `MEDULLA-DURABLE-KERNEL-R072`, `MEDULLA-DURABLE-KERNEL-R074`, `MEDULLA-DURABLE-KERNEL-R075` and `MEDULLA-DURABLE-KERNEL-R076.1`.
- The reference host runs the nightly bench suite that measures each service level.
