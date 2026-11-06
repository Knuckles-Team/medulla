# MEDULLA-TYPED-PACKS — Typed packs and catalog authority

| Field | Value |
|---|---|
| Stable spec ID | `typed-packs-and-catalog` |
| Owner | `Medulla` |
| Scope | Typed connector and agent components, content packs, schema composition, fleet/catalog authority, generated contract receipts |

## Evidence

The generator owns delivery state in `status.json`.
Record exact source, commands, test results and served evidence for each obligation.
Existing records do not establish execution of newly defined scenarios.

## User Scenarios & Testing

### User Story 1 - A connector author publishes one deterministic pack (Priority: P1)

The user is a connector author. The outcome is one deterministic snapshot of tools, skills, prompts, ontology, shapes, model profiles, A2A cards and the `connector://resources` declaration, with a predictable lifecycle.

**Acceptance scenarios**

1. **Given** a valid pack, **When** the author imports it twice, including after a package-only version bump, **Then** the second call returns `Unchanged` and writes nothing.
2. **Given** an imported pack, **When** a tool is removed and later restored with the same body, **Then** it becomes `Withdrawn`, leaves new selection, stays readable for historical pins and re-publishes under the same ID.
3. **Given** a forged or oversized index, an unsafe ontology import or an unbound importer, **When** it is submitted, **Then** the import returns a stable refusal with at most 256 violations and no durable change.

Independent test: the bound tests of `MEDULLA-TYPED-PACKS-R001` run this story through a production entry point.

### User Story 2 - A tenant administrator operates packs and projections (Priority: P1)

The user is a tenant administrator. The outcome is control of the importer binding, the head, retirement and projection status.

**Acceptance scenarios**

1. **Given** a pack head, **When** the administrator binds the importer, retires an entry or reprojects, **Then** only an admin with the matching tenant succeeds and the head reflects the change.
2. **Given** a worker crash after schema attach and before acknowledgement, **When** the projection retries, **Then** the same record attaches as a no-op and visibility advances once.

Independent test: the bound tests of `MEDULLA-TYPED-PACKS-R007` run this story through a production entry point.

### User Story 3 - A consumer reads typed components and verified bodies (Priority: P1)

The user is a consumer of the catalog. The outcome is typed reads that pass the readiness gate and exact verified bytes.

**Acceptance scenarios**

1. **Given** an authorized reader, **When** it reads a component, **Then** it receives exact verified bytes and a receipt that matches the contract vectors.
2. **Given** an unauthorized reader, **When** it reads the same component, **Then** it receives a stable denial and no importer identity, body bytes or cross-tenant metadata.

Independent test: the bound tests of `MEDULLA-TYPED-PACKS-R042` run this story through a production entry point.

### User Story 4 - An ontology owner keeps packs and vocabulary joined to the core (Priority: P2)

The user is an ontology owner. The outcome is a governed core and packs that extend it through subclass edges.

**Acceptance scenarios**

1. **Given** a pack ontology with a class that has no subclass path to a core class, **When** it is imported, **Then** the import is refused with `ONTOLOGY_ORPHAN` naming the class.
2. **Given** a pack that declares an import of the core ontology, **When** it is imported, **Then** the import is refused with `ONTOLOGY_PACK_IMPORTS_CORE`.

Independent test: the bound tests of `MEDULLA-TYPED-PACKS-R071` run this story through a production entry point.

### User Story 5 - A catalog steward governs glossary, dashboards and classification (Priority: P2)

The user is a data steward. The outcome is a glossary, dashboards, classification claims and an edit history that are first-class catalog assets.

**Acceptance scenarios**

1. **Given** a glossary scheme, **When** a term is proposed, reviewed and transitioned, **Then** each step is a recorded revision readable by UQL.
2. **Given** a promoted marking, **When** it propagates along lineage edges, **Then** downstream assets carry proposed claims with provenance `propagated`.

Independent test: the bound tests of `MEDULLA-TYPED-PACKS-R801` run this story through a production entry point.

### User Story 6 - A skill author ships a skill graph as a pack (Priority: P3)

The user is a skill author. The outcome is a skill graph that validates against an open knowledge-bundle format and layers its ontology on the core.

**Acceptance scenarios**

1. **Given** a graph bundle with a wrong hash, **When** it is imported, **Then** the import is refused with `SKILL_GRAPH_HASH_MISMATCH` naming the path and no state changes.
2. **Given** a conformant bundle with an ontology layer, **When** it is imported, **Then** its classes read through a core class.

Independent test: the bound tests of `MEDULLA-TYPED-PACKS-R700` run this story through a production entry point.

### User Story 7 - A contributor extends the catalog without drift (Priority: P3)

The user is a contributor. The outcome is a pack kind or catalog method added through the existing type, registry and handler path with proven parity.

**Acceptance scenarios**

1. **Given** a new method, **When** the contributor adds it, **Then** contract generation in check mode matches across every output directory and every client uses the generated result type.

Independent test: the bound tests of `MEDULLA-TYPED-PACKS-R067` run this story through a production entry point.

### User Story 8 - Sync the metadata catalog with an open-metadata store (Priority: P2)

The user is a data steward. The outcome is that glossary terms, assets, classifications and lineage flow both ways with the catalog as system of record.

**Acceptance scenarios**

1. **Given** recorded glossary terms, assets and lineage of an open-metadata store, **When** sync runs, **Then** the catalog holds them with their GUID and version, and unmapped types are listed.
2. **Given** a term edited on both sides, **When** sync runs, **Then** sync records one conflict and proposes the catalog value back.

Independent test: the bound tests of `MEDULLA-TYPED-PACKS-R110.9` run this story through the served method.

### Edge Cases

- A pack whose digest differs only by archive layout or package version keeps the same semantic digest.
- A crash before commit leaves only reclaimable orphan bodies; a crash during commit exposes no partial head.
- A lost reply replays by operation identity; changed bytes under the same idempotency key are refused.
- Validation that reaches 256 violations or its step budget stops and reports truncation.
- A denied read never reveals importer identity, body bytes or cross-tenant metadata.
- A projection that fails stays visible and retryable without losing later work.
- A retired row leaves no shim; its behaviour is removed in the same change.

## Requirements

- **FR-001**: Pack index, entry, annotation and digest contract. Rows: `MEDULLA-TYPED-PACKS-R001`, `MEDULLA-TYPED-PACKS-R022`, `MEDULLA-TYPED-PACKS-R023`.
- **FR-002**: Typed pack and component operations with declared results. Rows: `MEDULLA-TYPED-PACKS-R007`, `MEDULLA-TYPED-PACKS-R018`, `MEDULLA-TYPED-PACKS-R067`.
- **FR-003**: Atomic import, replay and lifecycle. Rows: `MEDULLA-TYPED-PACKS-R008`, `MEDULLA-TYPED-PACKS-R014`, `MEDULLA-TYPED-PACKS-R015`.
- **FR-004**: Bounded validation with stable refusal codes. Rows: `MEDULLA-TYPED-PACKS-R003`, `MEDULLA-TYPED-PACKS-R031`, `MEDULLA-TYPED-PACKS-R032`.
- **FR-005**: Projection through an outbox with a visibility gate. Rows: `MEDULLA-TYPED-PACKS-R006`, `MEDULLA-TYPED-PACKS-R009`, `MEDULLA-TYPED-PACKS-R012`.
- **FR-006**: Composed graph schema sources and ontology import. Rows: `MEDULLA-TYPED-PACKS-R035`, `MEDULLA-TYPED-PACKS-R103`, `MEDULLA-TYPED-PACKS-R104`.
- **FR-007**: Prompt identity, rendering and blueprint round trips. Rows: `MEDULLA-TYPED-PACKS-R001.6`, `MEDULLA-TYPED-PACKS-R001.8`, `MEDULLA-TYPED-PACKS-R094`.
- **FR-008**: Core ontology corpus, packs extend the core. Rows: `MEDULLA-TYPED-PACKS-R071`, `MEDULLA-TYPED-PACKS-R090`, `MEDULLA-TYPED-PACKS-R105`.
- **FR-009**: Metadata-catalog vocabulary, glossary and dashboards. Rows: `MEDULLA-TYPED-PACKS-R108`, `MEDULLA-TYPED-PACKS-R801`, `MEDULLA-TYPED-PACKS-R109`.
- **FR-010**: Object model, edit chain and classification claims. Rows: `MEDULLA-TYPED-PACKS-R650`, `MEDULLA-TYPED-PACKS-R651`, `MEDULLA-TYPED-PACKS-R652`.
- **FR-011**: Skill-graph packs and skill packages. Rows: `MEDULLA-TYPED-PACKS-R100`, `MEDULLA-TYPED-PACKS-R700`, `MEDULLA-TYPED-PACKS-R701`.
- **FR-012**: The open-metadata store is a federated secondary of the metadata catalog, synced two ways with version-checked write-back. Rows: `MEDULLA-TYPED-PACKS-R110`.

## Success Criteria

- **SC-001**: An identical pack imported twice leaves head, revisions, receipts and outbox unchanged. Rows: `MEDULLA-TYPED-PACKS-R004`, `MEDULLA-TYPED-PACKS-R014`.
- **SC-002**: No rejected import changes the head, members, holders, body liveness, record, receipt or outbox. Rows: `MEDULLA-TYPED-PACKS-R003`, `MEDULLA-TYPED-PACKS-R031`.
- **SC-003**: Every digest of a pack is recomputed and matches the golden vectors on every client. Rows: `MEDULLA-TYPED-PACKS-R001`, `MEDULLA-TYPED-PACKS-R067`.
- **SC-004**: A denied read returns a stable code and no bytes. Rows: `MEDULLA-TYPED-PACKS-R012`, `MEDULLA-TYPED-PACKS-R038`.
- **SC-005**: A pack class without a subclass path to a core class is always refused. Rows: `MEDULLA-TYPED-PACKS-R105`, `MEDULLA-TYPED-PACKS-R102.3`.
- **SC-006**: The projection visibility gate survives a crash and a retry. Rows: `MEDULLA-TYPED-PACKS-R006`, `MEDULLA-TYPED-PACKS-R009`.
- **SC-007**: Contract generation in check mode matches across every output directory. Rows: `MEDULLA-TYPED-PACKS-R067`, `MEDULLA-TYPED-PACKS-R068`.
- **SC-008**: Each refusal code is unique and registered once. Rows: `MEDULLA-TYPED-PACKS-R003`, `MEDULLA-TYPED-PACKS-R105`.

## Assumptions

- Arbora supplies a verified principal and tenant on every served request.
- The Graft SDK builds the canonical pack and its digest vectors; Medulla recomputes and never trusts them.
- Foreign capability claims in a pack enter as claims until an independent evaluation raises them.
- The Rust engine owns the object model; `medulla/ontology` is a read-only Python API (`MEDULLA-TYPED-PACKS-R081`). The alignment axioms of `MEDULLA-TYPED-PACKS-R105.5` are planned work.

## Normative requirements

| ID | Contract | Stable obligations |
|---|---|---|
| PACK-01 | One `ConnectorPackIndex` identifies a connector, exactly one server entry, sorted unique typed entries and bounded archive sections. Medulla recomputes raw-body SHA-256 and domain-framed entry, annotation, reference and pack digests; mismatches are refused. Archive layout and package version do not change the semantic pack digest. Keep golden vectors at `contract/fixtures/connector_pack_digest_vectors.json`. | MEDULLA-TYPED-PACKS-R001, MEDULLA-TYPED-PACKS-R022–MEDULLA-TYPED-PACKS-R026 |
| PACK-02 | `ConnectorPack` uses typed `Status`, `Import`, `Bind`, `Unbind`, `Retire`, `Reproject`, `ReconcileBodies`, and catalog reads. `AgentComponent.Content` supplies a verified body through the existing component method. Every operation has a declared result, action, durability and routing policy; dynamic result fallbacks are forbidden. | MEDULLA-TYPED-PACKS-R001, MEDULLA-TYPED-PACKS-R007, MEDULLA-TYPED-PACKS-R018, MEDULLA-TYPED-PACKS-R067 |
| PACK-03 | Import checks verified tenant and importer binding, expected head, archive, validation and policy before writing. Engine bodies may be staged first. One Agent Library owner transaction commits components, memberships, body holders, head, import record, receipt and ordered outbox intents. A crash before commit leaves only reclaimable orphan bodies; a crash during commit exposes no partial head. A lost reply replays by operation identity; changed bytes under the same idempotency key are refused. | MEDULLA-TYPED-PACKS-R002, MEDULLA-TYPED-PACKS-R003, MEDULLA-TYPED-PACKS-R008, MEDULLA-TYPED-PACKS-R010, MEDULLA-TYPED-PACKS-R041 |
| PACK-04 | A pack is a content snapshot. Identical head digest returns `Unchanged` with no version bump. An unchanged entry and pins carry forward; changed content or a changed server pin creates a revision. Absence becomes reversible `Withdrawn`; explicit admin retirement is permanent. New candidate/search reads hide withdrawn and unready records, while historical pinned reads remain resolvable. Direct component writes to reserved pack IDs are refused. | MEDULLA-TYPED-PACKS-R014, MEDULLA-TYPED-PACKS-R016, MEDULLA-TYPED-PACKS-R041, MEDULLA-TYPED-PACKS-R049, MEDULLA-TYPED-PACKS-R050 |
| PACK-05 | Each pack entry's references resolve only to a sibling of the stated kind, without cycles. Validation and the atomic writer both resolve pins against staged rows before stored rows. The server pin binds declared contract version, name and instructions; package version is provenance only. `contract_pin` is an Graft SDK claim, and Medulla must not pretend to recompute Graft SDK normalization. | MEDULLA-TYPED-PACKS-R016, MEDULLA-TYPED-PACKS-R036, MEDULLA-TYPED-PACKS-R049, MEDULLA-TYPED-PACKS-R050, MEDULLA-TYPED-PACKS-R064, MEDULLA-TYPED-PACKS-R065 |
| PACK-06 | Validation applies rules G1–G21 in [architecture.md](plan.md), collecting at most 256 violations and reporting budget exhaustion. No rejected import changes the head, members, holders, body liveness, record, receipt or outbox. Bounded parsers, no remote ontology import, and step-limited reasoning are mandatory; the OWL consistency algorithm itself is owned by `federated-query-and-reasoning`. | MEDULLA-TYPED-PACKS-R003, MEDULLA-TYPED-PACKS-R022–MEDULLA-TYPED-PACKS-R033, MEDULLA-TYPED-PACKS-R034–MEDULLA-TYPED-PACKS-R040, MEDULLA-TYPED-PACKS-R086 |
| PACK-07 | An Agent Library outbox worker projects the current committed head into a reserved `pack__<connector>` graph. One graph-schema attach carries the pack record revision; a subsequent Agent Library transaction marks `visible_record_id` and acknowledges the outbox item. Read paths filter against visibility. A failed projection is visible, rejected for retry without losing later work, and can be reprojected. A stale worker cannot overwrite a newer source. | MEDULLA-TYPED-PACKS-R006, MEDULLA-TYPED-PACKS-R051, MEDULLA-TYPED-PACKS-R052, MEDULLA-TYPED-PACKS-R053, MEDULLA-TYPED-PACKS-R054–MEDULLA-TYPED-PACKS-R059, MEDULLA-TYPED-PACKS-R074 |
| PACK-08 | Graph schema is a bounded map of named sources. Each source carries shapes and ontology bytes, origin and revision; the composed digest and policy are derived. K1–K7 conflict/consistency rules in [architecture.md](plan.md) apply on attach, detach and pack projection. `IcvConfigure` maps to the operator source. `GraphSchema.List` and `GraphSchemaClasses` expose versioned source/class/property facts. Attach to another graph is a snapshot whose staleness is visible. | MEDULLA-TYPED-PACKS-R051, MEDULLA-TYPED-PACKS-R061, MEDULLA-TYPED-PACKS-R062, MEDULLA-TYPED-PACKS-R071, MEDULLA-TYPED-PACKS-R077 |
| PACK-09 | Medulla is the typed catalog authority: `RegisterServer`/ServerRegistry owns liveness; `ConnectorPack`/`AgentComponent` owns content and lifecycle. The Graft SDK creates/imports packs; Arbora joins typed reads for serving. No SQL shadow catalog or independent Meristem content authority may appear. Catalog reads carry tenant, visibility, contract and version provenance. Python model convergence is owned by `public-engine-contract-and-release`. | MEDULLA-TYPED-PACKS-R042–MEDULLA-TYPED-PACKS-R048, MEDULLA-TYPED-PACKS-R066, MEDULLA-TYPED-PACKS-R072 |
| PACK-10 | Pack methods consume the shared generated wire/client artifacts and receipt. Pack-specific OpKind/PredKind additions are exhaustively enumerated by canonical printer and parser checks. The Go/JS codec, Python model convergence and generated-client profile audit belong to `public-engine-contract-and-release`. | MEDULLA-TYPED-PACKS-R068, MEDULLA-TYPED-PACKS-R070, MEDULLA-TYPED-PACKS-R080 |
| PACK-11 | Served and embedded routes enforce identical tenant, graph and action authority for the methods they expose. A denied pack or component read never leaks importer identity, body bytes or cross-tenant metadata. Long import work observes deadline/cancellation and releases its pack lock. Projection uses a least-privilege grant tied to the outbox row and records failures. | MEDULLA-TYPED-PACKS-R012, MEDULLA-TYPED-PACKS-R038, MEDULLA-TYPED-PACKS-R074, MEDULLA-TYPED-PACKS-R084, MEDULLA-TYPED-PACKS-R087, MEDULLA-TYPED-PACKS-R088 |
| PACK-12 | Pack contracts are verified through planted invalid cases, mutation tests, crash/retry tests, tenant and cluster tests. They also require Graft SDK/client conformance. Full Rust/Python suites and quality/release gates complete the verification. General Python CI and dispatch-decomposition gates are owned by `public-engine-contract-and-release`; their results are required here as consumer evidence. | MEDULLA-TYPED-PACKS-R005, MEDULLA-TYPED-PACKS-R075, MEDULLA-TYPED-PACKS-R076, MEDULLA-TYPED-PACKS-R086 |

## Entry and validation boundaries

The current wire schema constant is `CONNECTOR_PACK_SCHEMA_VERSION = 2`; a version change requires a coordinated generated contract and TCK bump, not an unreviewed rewrite. `Import` is a single operation, not a prepare/commit/abort protocol. Pack and the four agent-layer methods remain `LocalOnly` until a separately specified clustered writer exists. An existing incompatible Agent Library layout is refused with `AGENT_LIBRARY_FORMAT_UPGRADE_REQUIRED`; this spec does not promise an offline migration. Only `SKILL.md` is imported as skill content; supporting files require a new bounded contract. Source authorization uses the bound importer with an environment-configured initial default; binding and retirement require `admin:connector-pack`.

## Acceptance scenarios

| Scenario | Expected observable result |
|---|---|
| Identical pack is imported twice, including after a package-only version bump | Second call is `Unchanged`. Head, component revisions, receipt count and outbox remain unchanged. |
| A tool is removed, then restored with the same body | It becomes `Withdrawn`, disappears from new selection, remains available for historical pins, then re-publishes under the same ID. |
| A worker crashes after schema attach and before Agent Library acknowledgement. | Retry attaches the same record as a no-op and advances visibility once. No older record replaces the newer one. |
| A forged or oversized index, unsafe ontology import or unbound importer is submitted. | Return stable refusal with bounded violations and zero catalog mutation. |
| A client signs a generated method body or reads component content | The sender matches contract vectors. Authorized reader receives exact verified bytes, unauthorized reader receives stable denial without bytes. |

## Traceability and ownership

The IDs in the requirement table are stable work identifiers. The full behavior is defined here and in this directory. Graft SDK package construction and connector certification belong in the Graft SDK's own public specs. Arbora serving and the browser belong in their own repositories. This spec defines the Medulla boundary those clients must call. Unit and contract proof do not require private documentation, a particular operator deployment, or a live external service. Specific deferred adjacent work is listed in [tasks.md](tasks.md). This prevents a broad deferred-work entry from being mistaken for completed pack behavior.

## Delivery criteria

Every PACK requirement has an exact merged commit, passing mapped tests, matching generated contract artifacts, and zero new unjustified duplication/complexity. A served-path result covers Import, Status, Content and projection. The visibility gate survives crash and retry. Unauthorized and malformed input never changes durable state. No public client must infer a wire result. Record source revision, command, test count, environment and scenario results in this spec bundle.
These records must establish every required served and durable outcome.

Requirement IDs are defined in [requirements.md](requirements.md); delivery state per ID is in `status.json`.


## Native prompt blueprints

R001, R002 and R094 also own typed versioned prompt objects, runtime rendering and native blueprint round trips.
Reuse component identity, graph projections, engine bodies and generated operations.
A prompt must pass its pinned schema before rendering or model dispatch.
Optimization emits proposals; existing policy controls adoption.
Repository-source writes require separate approved Graft SDK execution under Arbora authority.
R094.7 now references merged SCION-GOVERNED-WRITEBACK-R004.5 through R004.12 and ARBORA-FLEET-R003.4 through R003.10.
The exact producer revisions and contracts are pinned in plan.md; runtime acceptance still requires the actual served journey.
R002.10 retains its missing Meristem model-dispatch consumer requirement and call-site prerequisite.
Original pack types, digest vectors, clients, body holders, grace reconciliation and Stable content serving remain mandatory.

<!-- spec-definition-patch:owner-batch-20261012-typed-packs-and-catalog -->
## Owner definitions

### `MEDULLA-TYPED-PACKS-R100.7` — AU:31

The existing rendered-package path must gate catalog exposure on a `RecordedRunValidationV1`. Bind it to tenant, skill, candidate revision, exact rendered content digest, graph revision, and validation-policy reference `{policy_iri, policy_revision}`. Each sorted unique `run_refs` entry carries `{run_ref, package_content_digest, graph_revision, policy_revision, outcome, evidence_digest}`. `run_set_digest` hashes its canonical encoding. Require `run_set_complete=true`. Each run attests the same package and policy revisions. Missing, failed, partial, stale, mismatched, or cross-tenant evidence refuses publication before `skill_row` returns a publishable package. Meristem `record_outcome`/`SkillGraphPipeline` remains the run/candidate producer. Medulla `render_skill_package` and `skill_row` bind and enforce the receipt.

**Open dependency:** the reviewed Meristem rows do not define the validation operation, receipt producer fields, run-set selection, or pass criterion. Meristem must publish those semantics before a passing cross-owner journey can be accepted. Until then absence of a receipt refuses; this does not close the positive producer path. Preserve `MERISTEM-HARNESS-R022.2/.5/.6`, `MERISTEM-HARNESS-R004.1`, and `MEDULLA-TYPED-PACKS-R100.1/.5/.6`. Source hash: `96ff38761f9e21d52f608ba593c902aff4c93ca7b1ed13088ad39efd3e968a3a`.

### `MEDULLA-TYPED-PACKS-R097.2.7` — EG:35

External ontology ingestion is directional from the pinned external schema into Medulla. The import contract covers classes, object/datatype/annotation properties and named individuals. An enumeration is a class with named individuals. Map each declared term by a pinned, importer-authorized, kind-compatible `ExternalTermMappingV1` to a platform IRI. Retain source IRI and kind as provenance. Otherwise, return revision-bound coverage as `Unmapped{reason: NoDeclaredMapping}` and do not adopt the term. Sort coverage by external IRI and kind. Partial unmapped coverage does not invalidate a pack under R097.2. Malformed, duplicate, unknown-kind, incompatible, forged or stale mappings refuse before commit/activation. Validate in `ConnectorPack.Import::validate_and_commit`. Recheck in `apply_pack_source`. Return full per-term coverage on `OntologyInspect`. Use `GraphSchemaClasses` as independent readback for adopted classes and non-adoption of unmapped classes. Keep R071.1 as the reverse Medulla-class-to-external-vocabulary alignment; do not broaden it.

Preserve `MEDULLA-TYPED-PACKS-R051`, `MEDULLA-UNIFIED-DATA-PLANE-R005/R037`, `MEDULLA-TYPED-PACKS-R071.1.1-.6`, and `MEDULLA-TYPED-PACKS-R097.2.1-.6`. Source hash: `318723aee15c476039eca669a3230b206532c0c704b37efad5a7c49690bbb9c2`.

### `MEDULLA-TYPED-PACKS-R100.8` — EG:56

Add `ExportWorkflowRdfV1(workflow_iri, requested_workflow_version, stored_version_ref) -> WorkflowRdfDocumentV1` and `ImportWorkflowRdfV1(document) -> WorkflowImportReceiptV1{workflow_iri, committed_workflow_revision, graph_revision, dataset_digest, importer_id}` to the existing skill-package owner. The versioned `WorkflowRdfDocumentV1` contains workflow/skill IDs and workflow/stored-version/graph/ontology revisions. It also contains canonical N-Quads, dataset digest and serializer identity. `dataset_digest` is SHA-256 of the canonical UTF-8 N-Quads bytes selected by the document-pinned serializer. Export preserves every workflow triple, named graph, typed/language-tagged literal, ontology IRI and unknown workflow predicate. Import validates the canonical-byte dataset digest and restores an isomorphic graph through the existing graph write boundary. Keep the original strict-newer check at export.

**Open dependency:** source review does not identify the stored workflow version record to compare or its ordering grammar. It also does not identify the existing R100 consumer import operation. Do not shift comparison to import. Do not claim a complete round-trip until the owner decisions and exact attach points are identified. Preserve `MEDULLA-TYPED-PACKS-R051/R090/R100.1-.6`. Source hash: `620b1ae05d8e35696c7522f207672ef6f374525004d5488fe62fb4b27fc6cf5a`.
<!-- /spec-definition-patch -->

<!-- spec-definition-patch:reviewed-mapped-subsets-20261012 -->
## Reviewed mapped children

The children below extend existing rollups. Existing parent requirements, children, policy authority and refusal behavior remain in force. These are SPECIFIED definitions only; no code, producer receipt, consumer receipt or served result is claimed.

### Extension of R069

Preserve the existing `R069` obligation and all prior children; add `MEDULLA-TYPED-PACKS-R069.100`. The child definitions below state only the mapped residual and its exact owner boundary.

#### `MEDULLA-TYPED-PACKS-R069.100` — AU:69

Define `FingerprintIdentityV1` with repository id, branch/ref id, normalized path and commit/source revision. Include blob/raw digest, language id, parser implementation/version, normalization-policy revision and grammar digest. The versioned profile registry lists each supported language and the trivia/comment forms removed from its canonical token stream. Preserve strings, literals, token order and all non-trivia tokens. Hash the canonical token stream with language/parser/profile versions. Unsupported languages use raw-content digest only. Never classify them as cosmetic. An identical complete source identity and profile is idempotent. The same blob at a new branch, path or source revision MUST create/update that source binding and provenance. Reuse parsed output only under identical parser/profile identity; a matching raw digest alone is never a full no-op. A changed parser or normalization profile forces reparse and fingerprint recomputation. Then update the binding even if semantic content remains equivalent. Control semantic graph mutation by the recomputed semantic fingerprint. Cosmetic-only and profile-only changes with equal canonical semantics update source provenance without changing semantic graph revision. Structural changes update both. Graft SDK transports source identity/raw digest only. Medulla owns semantic classification.

Producer/consumer: RepositorySnapshotProvider -> Graft SDK transport -> generated IndexRepository -> Medulla parser/index mutation; R079 embedding dedupe is not structural-mutation suppression.

Source record SHA-256: `8ca81dc16448fefc73440f2eb9ea07b2e433f7e38075563f86800071029846e9`. Verification remains planned until independently served by each named owner.

### Extension of R100

Preserve the existing `R100` obligation and all prior children; add `MEDULLA-TYPED-PACKS-R100.100`, `MEDULLA-TYPED-PACKS-R100.101`. The child definitions below state only the mapped residual and its exact owner boundary.

#### `MEDULLA-TYPED-PACKS-R100.100` — AU:90

Define an edit representation bound to the original SKILL.md byte digest and source revision. Bind it to frontmatter field spans, step spans, line endings and unknown/custom-field spans. Apply edits as bounded source-range patches. Do not parse and reserialize the entire document. Preserve all untouched bytes, ordering, spacing, comments, line endings and unknown frontmatter fields. Emit a new content digest and source provenance. Continue to enforce R019 SKILL.md-only import and R029 restricted YAML validation.

Producer/consumer: Medulla owns skill parse/admission and R100 rendering; Graft SDK owns source transport/edit write-back edge. No prompt blueprint or whole-document normalization path is substituted.

Source record SHA-256: `1b717c4a284896dbf8c7b4fb1f00ebba80fa6a75e930a020c5bc01706634f582`. Verification remains planned until independently served by each named owner.

#### `MEDULLA-TYPED-PACKS-R100.101` — AU:98

Define an admission record bound to prior/new skill content digests and consumer-declared input/output behavior contract revisions. Include the observed consumer cohort, compatibility verdict, migration id/digest and admission revision. Check compatibility against consuming-code behavior. Package version equality, contract-pin equality, endpoint certification or dependency digest alone do not establish compatibility. Refuse breaking changes before publication or invocation unless a declared migration binds the exact old/new digests and consumer contract. Require the migration receipt to pass.

Producer/consumer: Medulla skill package read/admission -> Arbora invoke certification and consumer code; Arbora owns its invocation gate. Do not equate existing version/dependency pins with compatibility.

Source record SHA-256: `531cb0ba1accd9e88833249fb253a9f5f7a3c1a911f653b3c9a5bfc42341f978`. Verification remains planned until independently served by each named owner.

### Extension of R001

Preserve the existing `R001` obligation and all prior children; add `MEDULLA-TYPED-PACKS-R001.100`. The child definitions below state only the mapped residual and its exact owner boundary.

#### `MEDULLA-TYPED-PACKS-R001.100` — AU:52

Extend existing ConnectorPack prompt and tool definition entries. Include explicit schema version, nested field schema, required/optional status and compatibility revision. Incremental evolution records base digest, changed paths and compatibility verdict. Admit additive optional fields only when consumer policy says compatible. Removed or changed required fields refuse before publication. Pack index/entry digest rules remain canonical. Graft SDK owns tool execution/schema compatibility authority. Medulla records and serves the versioned declaration and does not authorize Graft SDK calls.

Producer/consumer: Medulla ConnectorPack typed authority -> generated public pack APIs; Graft SDK retains tool compatibility and execution authority. Preserve R001 index/entry/section/annotation and digest contract.

Source record SHA-256: `e03b25156a4f84d27c4afba63d0cd9e526b549b5905994a53664d193416d048c`. Verification remains planned until independently served by each named owner.

### Extension of R072

Preserve the existing `R072` obligation and all prior children; add `MEDULLA-TYPED-PACKS-R072.100`. The child definitions below state only the mapped residual and its exact owner boundary.

#### `MEDULLA-TYPED-PACKS-R072.100` — AU:97

Extend the typed fleet catalog with workflow registration identity, version/content digest, owner/tenant scope, schema and lifecycle state. Registration is persisted through the existing SourceIngest authority; invocation resolves by registry id and exact admitted revision rather than a caller-supplied body. Unknown, withdrawn, stale or unauthorized workflow ids refuse before execution. Engine/client initialization remains lazy at the Meristem first-use boundary and is not performed at registration or process startup.

Producer/consumer: Medulla typed registry and SourceIngest R002 own durable registration; Meristem owns lazy client initialization/execution; preserve ARBORA-HOST-R022 workflow-store semantics.

**Open owner prerequisite:** MERISTEM-CONTROL-R037.5 only fills a requirement gap; it does not define lazy initialization timing, the first-use operation, or the execution receipt. No exact Meristem companion requirement ID is identified. Keep that consumer as an explicit open dependency until its owner publishes the operation and receipt; no invocation evidence is claimed.
Source record SHA-256: `f73767d1a3fdba29a702254ed1db2c9904158b6136f6f4b38a3c9a0d3e4fcb03`. Verification remains planned until independently served by each named owner.

### Extension of R071

Preserve the existing `R071` obligation and all prior children; add `MEDULLA-TYPED-PACKS-R071.100`. The child definitions below state only the mapped residual and its exact owner boundary.

#### `MEDULLA-TYPED-PACKS-R071.100` — EG:8

Define one versioned concept-path grammar and canonicalization rule shared by Medulla vocabulary producers. Include stable segment identity, parent path and canonical concept IRI. Resolve the entire path against the pinned vocabulary revision during admission. Check repository-wide uniqueness and parent/child consistency. Refuse duplicate aliases, paths that contradict an existing parent/child relationship, or two IRIs for the same canonical path. Return the conflicting ids. Keep existing published ontology IRIs unchanged and crosswalk them rather than rewriting them.

Producer/consumer: Medulla vocabulary validation owns canonical identity; repository producers submit proposed paths. Preserve existing published IRIs and federated query vocabulary.

**Open owner decision:** The canonical segment grammar is not approved for separators/escaping, Unicode normalization, case, or whitespace. Keep admission SPECIFIED until the vocabulary owner publishes one versioned grammar shared by producers; preserve existing published IRIs.
Source record SHA-256: `525e39d1d194ac8a820bbd741a1c261ceab0c7d564981c33c731c06c1eb641d8`. Verification remains planned until independently served by each named owner.
<!-- /spec-definition-patch -->
