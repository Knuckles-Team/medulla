# MEDULLA-DATAPLAT-VOCAB — Data-platform vocabulary

**Owner:** Medulla (native Rust) · Cross-repo IDs: CONN-OBJECTSTORE-MCP-DATA-PLATFORM-OPERATIONS-R001, CONN-VECTOR-MCP-DATA-PLATFORM-OPERATIONS-R001, CONN-OPENBAO-MCP-DATA-PLATFORM-OPERATIONS-R001, CONN-CLARITY-API-DATA-PLATFORM-OPERATIONS-R001, CONN-DATA-SCIENCE-MCP-DATA-PLATFORM-OPERATIONS-R001.

## Outcome and boundary

The engine needs one typed vocabulary for the data-platform stores (sql, documentdb, objectstore, vector, kafka, openbao, clarity, data-science) so a competency question over them is answered through each connector's access contract (`specs/connector-competency`). Several of these stores are themselves the contract kinds through which other data is reached, so the vocabulary describes a store as a source and says how its contents map to domain concepts through approved virtual mappings.

Owned here: the `core:data-platform@1` classes, properties and shapes, the `dp:StoreMapping` fixtures per connector, the guards and the alignment tables. Not owned here: discovery, R2RML compilation, pushdown, acceleration (`specs/unified-data-plane`), cross-source joins (`specs/federated-query-and-reasoning`), the competency questions (`specs/connector-competency`) and connector code (the connector repositories).

## User Scenarios & Testing

### User Story 1 - Name a data store and its contents in shared terms (Priority: P1)

The user is an ontology owner. The outcome is one registered module that types stores, tables, collections, buckets, vectors, topics and models, so that a question reads one set of terms across all eight stores.

**Acceptance scenarios**

1. **Given** the engine bootstraps its core schema sources, **When** the `data-platform` module loads, **Then** each shared, relational, document, object, vector, stream, analytics and ML term resolves with its domain and range.
2. **Given** a source connection of kind `document`, **When** it is validated against the virtual-graph shape, **Then** it is accepted and a connection of kind `ftp` is still refused.

Independent test: `MEDULLA-DATAPLAT-VOCAB-R001.1` and `MEDULLA-DATAPLAT-VOCAB-R001.3`.

### User Story 2 - Hold secret metadata and never a secret value (Priority: P1)

The user is a security reviewer. The outcome is that no term, shape or mapping can carry a secret value, so that a question about secret paths and policies cannot leak one.

**Acceptance scenarios**

1. **Given** a `dp:SecretPath` carrying a `value` triple or a `token` triple, **When** the graph validates it, **Then** the closed shape refuses it.
2. **Given** a `dp:StoreMapping` whose `dp:viaOperation` reads a secret value, **When** it is validated, **Then** the denylist refuses it and a metadata-only operation is accepted.

Independent test: `MEDULLA-DATAPLAT-VOCAB-R005.3` and `MEDULLA-DATAPLAT-VOCAB-R005.4`.

### User Story 3 - Map each connector's operations virtual-first (Priority: P2)

The user is an operator attaching a connector. The outcome is one validated mapping fixture per connector that names its operations and its acceleration mode.

**Acceptance scenarios**

1. **Given** a mapping fixture for one of the eight stores, **When** the fixture test runs, **Then** each mapping names a closed contract kind, a closed acceleration mode and the listed operations.
2. **Given** a mapping marked `materialised`, **When** it has no reason, **Then** validation refuses it.

Independent test: `MEDULLA-DATAPLAT-VOCAB-R006.1` to `R006.8`, `MEDULLA-DATAPLAT-VOCAB-R007.1`.

### User Story 4 - Keep one discovery path and virtual by default (Priority: P2)

The user is the engine maintainer. The outcome is that this vocabulary adds no second discovery path and no copy without a stated reason.

**Acceptance scenarios**

1. **Given** the vocabulary file and every fixture, **When** the scan runs, **Then** no term or mapping declares a discovery operation.
2. **Given** the eight fixtures, **When** the scan runs, **Then** every mapping not marked `materialised` has no acceleration policy.

Independent test: `MEDULLA-DATAPLAT-VOCAB-R007.3` and `MEDULLA-DATAPLAT-VOCAB-R007.4`.

### User Story 5 - Align package-local ontologies to the shared terms (Priority: P3)

The user is a connector maintainer. The outcome is that each package term maps to a `dp:` term, so the two vocabularies cannot drift.

**Acceptance scenarios**

1. **Given** an alignment table, **When** the alignment test runs, **Then** every package term resolves to a `dp:` term through `rdfs:subClassOf` or `owl:equivalentProperty`.
2. **Given** a package term with no `dp:` target, **When** the test runs, **Then** it fails and names the term.

Independent test: `MEDULLA-DATAPLAT-VOCAB-R008.1` to `R008.3`.

### Edge Cases

- A negative `dp:lag`, a zero `dp:dimensions` and a `dp:presenceRatio` of 2 are rejected.
- A `dp:StoredObject` has no content property by design; object bytes are never mapped.
- A mapping with `dp:contractKind` `mcp-operation` and no `dp:viaOperation` is refused.
- One policy may grant read on path A and deny on path B; each grant keeps its own policy and path.
- An operation the competency questions need that a connector lacks is a row in the connector repository, and the question abstains typed until it lands.

### Assumptions

- Questions run over recorded fixtures; no live credential or network enters the pull-request gate.
- Package-local ontologies stay as the connector's emitted shape; this spec adds alignment only.
- The blocked operations (last access, vector statistics, KV metadata, click-quality metrics, run registry) are owned by the connector rows named in the cross-repo IDs.
- Every term the rows read is declared in `data_platform-v1.ttl`; there are no proposed terms.

## Requirements

- **FR-001**: `core:data-platform@1` defines the classes and properties of the plan and is registered next to `core:virtual-graph@1`. Rows: `MEDULLA-DATAPLAT-VOCAB-R001.1`, `MEDULLA-DATAPLAT-VOCAB-R001.2`, `MEDULLA-DATAPLAT-VOCAB-R001.3`, `MEDULLA-DATAPLAT-VOCAB-R001.4`, `MEDULLA-DATAPLAT-VOCAB-R002.1`, `MEDULLA-DATAPLAT-VOCAB-R002.2`, `MEDULLA-DATAPLAT-VOCAB-R002.3`, `MEDULLA-DATAPLAT-VOCAB-R002.4`, `MEDULLA-DATAPLAT-VOCAB-R003.1`, `MEDULLA-DATAPLAT-VOCAB-R003.2`, `MEDULLA-DATAPLAT-VOCAB-R003.3`, `MEDULLA-DATAPLAT-VOCAB-R004.1`, `MEDULLA-DATAPLAT-VOCAB-R004.2`, `MEDULLA-DATAPLAT-VOCAB-R004.3`, `MEDULLA-DATAPLAT-VOCAB-R004.4`.
- **FR-002**: Secret metadata only: no term, shape or mapping can carry a secret value. Rows: `MEDULLA-DATAPLAT-VOCAB-R005.1`, `MEDULLA-DATAPLAT-VOCAB-R005.2`, `MEDULLA-DATAPLAT-VOCAB-R005.3`, `MEDULLA-DATAPLAT-VOCAB-R005.4`, `MEDULLA-DATAPLAT-VOCAB-R005.5`.
- **FR-003**: Every store is virtual-first; a `materialised` mapping states a reason. Rows: `MEDULLA-DATAPLAT-VOCAB-R006.1`, `MEDULLA-DATAPLAT-VOCAB-R006.2`, `MEDULLA-DATAPLAT-VOCAB-R006.3`, `MEDULLA-DATAPLAT-VOCAB-R006.4`, `MEDULLA-DATAPLAT-VOCAB-R006.5`, `MEDULLA-DATAPLAT-VOCAB-R006.6`, `MEDULLA-DATAPLAT-VOCAB-R006.7`, `MEDULLA-DATAPLAT-VOCAB-R006.8`, `MEDULLA-DATAPLAT-VOCAB-R007.1`, `MEDULLA-DATAPLAT-VOCAB-R007.2`.
- **FR-004**: Schema discovery is engine-side; this spec adds no discovery path. Rows: `MEDULLA-DATAPLAT-VOCAB-R007.3`, `MEDULLA-DATAPLAT-VOCAB-R007.4`.
- **FR-005**: Package-local ontologies align to the `dp:` terms. Rows: `MEDULLA-DATAPLAT-VOCAB-R008.1`, `MEDULLA-DATAPLAT-VOCAB-R008.2`, `MEDULLA-DATAPLAT-VOCAB-R008.3`.
- **FR-006**: Operations the competency questions assume that do not exist are recorded as rows in the owning connector repository. Rows: `MEDULLA-DATAPLAT-VOCAB-R006.3`, `MEDULLA-DATAPLAT-VOCAB-R006.4`, `MEDULLA-DATAPLAT-VOCAB-R006.6`, `MEDULLA-DATAPLAT-VOCAB-R006.7`, `MEDULLA-DATAPLAT-VOCAB-R006.8`.

## Success Criteria

- **SC-001**: Every leaf row names a test that passes under `cargo test -p medulla-core --test data_platform_vocab` or `cargo test -p medulla-shacl --test data_platform_vocab`. Rows: `MEDULLA-DATAPLAT-VOCAB-R001.1`.
- **SC-002**: No violating triple or value-reading mapping is accepted in 100 percent of the refusal cases. Rows: `MEDULLA-DATAPLAT-VOCAB-R005.3`, `MEDULLA-DATAPLAT-VOCAB-R005.4`.
- **SC-003**: Every fixture mapping not marked `materialised` has no acceleration policy. Rows: `MEDULLA-DATAPLAT-VOCAB-R007.4`.

## Traceability

See [test-spec.md](test-spec.md). The design is in [plan.md](plan.md); entities are in [data-model.md](data-model.md); the shortest proof is in [quickstart.md](quickstart.md).
