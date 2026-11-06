# MEDULLA-TYPED-PACKS — Implementation plan

Governing spec: [spec.md](spec.md). Rows: [requirements.md](requirements.md).

## Summary

Medulla is the typed catalog authority for connector packs and agent components, and the owner of the core ontology, the metadata-catalog vocabulary, the object model and the classification claims that describe them. A pack enters through the served `ConnectorPack` method, passes bounded validation, commits in one Agent Library owner transaction and is projected to a graph schema by an outbox worker. Every operation is served through the dispatch of `crates/medulla-server-router`; the handler crate `crates/medulla-h-admin` holds the pack, component, schema and catalog handlers. Wire types live in `crates/medulla-catalog-types` and `crates/medulla-agent-types`; durable rows live in `crates/medulla-persistence-records`; the shipped Turtle corpus lives in `crates/medulla-core/ontology/`.

```text
Graft SDK / arbora --> ConnectorPack.Import / AgentComponent / GraphSchema / catalog methods
  serve_import > validate_and_commit > run_import > prepare (G1-G21)
    > engine body store > Agent Library owner transaction (head, members, holders, receipt, outbox)
      > project_head (outbox worker) > GraphSchema.AttachPack > visible_record_id > typed reads
```

## Technical Context

- **Language/Version**: Rust 2021 edition on the pinned toolchain of `rust-toolchain.toml`; Python 3.14 (floor 3.12) for the served tests and the `medulla` client package.
- **Primary Dependencies**: The workspace crates named in the Project Structure section, the bounded Turtle and SHACL code of `crates/medulla-rdf`, and the standard library. No generative model and no model server.
- **Storage**: The Agent Library owner tables, `engine_bodies` and the connector-pack tables of `crates/medulla-persistence-records`; graph data through the engine graph store.
- **Testing**: Rust unit and integration tests per crate, served Python tests under `tests/packs/` and `tests/typed_packs/`, and generated-contract checks of `medulla-capabilities`.
- **Target Platform**: Linux x86_64 and aarch64 hosts without AVX2 assumptions.
- **Project Type**: Library crates and a server handler crate.
- **Performance Goals**: Validation stops at 256 violations and at the derivation step budget; `Import` of an unchanged pack writes nothing; each row that sits on a hot path names its own budget and benchmark.
- **Constraints**: A rejected import changes no durable state. Every refusal has one stable code. No dynamic result envelope, no second contract generator, no shadow catalog. No version suffix in a name.
- **Scale/Scope**: The leaf rows of `requirements.md` and 7 user stories; the `FR-` and `SC-` labels of `spec.md` map rows to stories.

**Knob convention.** An engine knob named `knob x.y` in this spec's rows is a typed field of the server configuration, read from the command line or from the environment variable `MEDULLA_<NAME>` (the dotted name upper-cased, dots as underscores) and bounds-checked at startup: a value outside the stated range refuses startup with the knob name. Decision knobs are the exception and live only in the `MEDULLA-DECISION-ENGINE-R179` registry. Rows do not restate this.

## Constitution Check

- **R-I. The engine is native Rust and the only fact store**: pass. Every row lands in a Rust crate or the shipped ontology corpus of the engine.
- **R-II. Every wait has a deadline**: pass. Import observes deadline and cancellation and releases its pack lock; validation has a step budget.
- **R-III. Every row sets budgets and names a benchmark**: pass. Rows on a hot path carry their own budget; validation limits G2 and G21 are fixed.
- **R-IV. Crates are small, layered and cut over hard**: pass. Rows name post-split crates and no handler crate calls a sibling handler crate.
- **Organization standards**: pass. Rows follow the spec standard, the ecosystem boundaries and the wording standard.

## Project Structure

```text
crates/medulla-catalog-types/src/connector_pack/    existing: index, digest, annotations, ops, result, record types
crates/medulla-agent-types/src/                      existing: agent component, library and prompt types
crates/medulla-h-admin/src/admin/connector_pack/     existing: import, validation, planning, projection of packs
crates/medulla-h-admin/src/graph_schema/             existing: schema sources, attach, ontology import
crates/medulla-h-admin/src/catalog/                  new: glossary, dashboard, stewardship handlers
crates/medulla-persistence-records/src/connector_pack/ existing: head, members, receipt, outbox rows
crates/medulla-server-blob/src/blob/                 existing: engine bodies
crates/medulla-server-api/src/                       existing: icv_guard, schema composition
crates/medulla-server-router/src/dispatch/router/    existing: dispatch arms
crates/medulla-rdf/src/                              existing: bounded Turtle, SHACL, pack ontology checks
crates/medulla-core/ontology/                        existing: shipped Turtle corpus and shapes
crates/medulla-capabilities/src/domains/             existing: method descriptors, contract generation
medulla/ontology/                                    existing: Python read API of the corpus
tests/packs/, tests/typed_packs/                     existing: served Python tests
```

New files that rows add are marked `new` in their own `Shape:` sentence.

## Phase 0: Research

No unknowns; no research.md.

## Phase 1: Design

The design below binds every row; the detail sections follow under Design reference.

1. **One typed authority.** The existing pack, component, schema, outbox and capability registry are extended; no separate pack database appears.
2. **One validator.** G1-G21 and K1-K7 run in one bounded pass shared by pack and graph source composition.
3. **Atomic commit, asynchronous projection.** One owner transaction commits rows and outbox intents; `visible_record_id` gates readers until the schema attach commits.
4. **Hard cutover.** A retired behaviour is deleted in the same change: the orphan warn switch of `MEDULLA-TYPED-PACKS-R105.1` and the retired install, registry and workflow rows leave no shim.
5. **Integration-first proof.** A behaviour row is VERIFIED only by a test that enters through the production dispatch with a tenant and principal context.

Data entities are in [data-model.md](data-model.md). The shortest served proof is in [quickstart.md](quickstart.md). Each seam to another repository has a document in `contracts/`.

### Design reference

#### Design decisions

The implementation extends Medulla's current typed pack, component, schema, outbox and capability registry. It adds no separate pack database or dynamic response envelope. It adds no second contract generator or graph projection side channel. The public wire is generated from `medulla-capabilities`; canonical receipt content must be compared byte for byte. Changes to a shared wire shape land as one coordinated revision across Rust, generated Python/Go/JS and the Graft SDK TCK.

The order below minimizes incompatible intermediate states. A step may be a verification or repair of existing code. The existence of a module is not proof that the step is accepted. Each step maps to `tasks.md` and `test-spec.md`.

1. **Freeze contract and baseline.** Record the exact default-branch revision, current pack schema version and generated receipt. Run contract generation in check mode, full test inventory, CCCC, jscpd, Dupehound and KISS scans. Compare committed artifact *content* across all output directories. Classify findings as fixed, open or unrelated with exact evidence.
2. **Type and encoding proof.** Audit `medulla-types` bounds, kinds, digest framing, sorted lists, optionals and canonical fixture vectors. Make the Graft SDK's JSON serialization and deterministic idempotency key a shared TCK contract. Extend generated method/result descriptors rather than hand-writing client DTO copies.
3. **Admission and atomicity proof.** Audit G1–G21, tenant/importer checks, reserved IDs, preflight size checks and stable codes. The import planner resolves staged pins and stages engine bodies with holder/refcount invariants. It commits all owner rows plus N outbox intents atomically. Test all crash windows and replay identities.
4. **Lifecycle and catalog proof.** Verify carry-forward, changed pin cascade, reversible withdrawal, permanent retirement, content reads, no-op import and mass-withdrawal override. Confirm typed ServerRegistry owns liveness and component/pack owner owns content; remove any shadow catalog after consumer migration.
5. **Schema and projection proof.** Reuse one G13–G16/K1–K7 validator for operator, pack and ingestion sources. Attach current pack schema in one graph commit. Mark visibility only after attach; use durable failure/retry and least-privilege row-scoped graph grant. Verify static `AttachPack` staleness and `GraphSchemaClasses` output. Preserve exact `IcvConfigure` behavior.
6. **Consumer and release proof.** Run an end-to-end connector fixture from Graft SDK package creation through Import, projection, Arbora typed read and an authorized body read. Run Go/JS/Python/Rust contract vectors and full integration suites on the exact commit. Include scanner and architecture gates, release workflow parity, and a crash/restart restore check. Record exact evidence for each required outcome.

#### Interfaces and ownership

| Interface | Medulla owner | Caller/consumer contract |
|---|---|---|
| Pack wire and digest | `medulla-catalog-types` connector-pack modules; generated fixtures | Graft SDK creates index/archive and uses exact golden vectors. |
| Import and admin ops | `crates/medulla-h-admin/src/admin/connector_pack/` with typed method descriptor | Graft SDK uses `Status` then deterministic Import; admin calls Bind/Unbind/Retire/Reproject. |
| Durable rows and body liveness | Agent Library owner tables plus `engine_bodies` | No client writes component rows or body holder state directly. |
| Projection and graph schema | Outbox worker; `GraphSchema` graph commit; visibility gate | Typed catalog, assembly and search consume only visible head state. |
| Fleet catalog | ServerRegistry liveness plus component content | Arbora joins typed reads and may cache by version, without becoming content authority. |
| Contract receipt | `medulla-capabilities` registry and `gen_contract` | Python/Go/JS senders and Graft SDK verify method/result/signing vectors. |

#### R016 digest v4: package version is provenance only

MEDULLA-TYPED-PACKS-R016 requires equal server pins across package releases with equal content. The v3 definition digest mixes `SourcePackage.package_version` into every server component digest. Two independent stores that import the same content under different releases therefore mint different server pins.

- **Versioned digest.** New writes mint digest v4 under domain `au-eg/agent-component-definition/v4`. v4 omits `package_version` from the provenance input. The entry still stores the package version as provenance text.
- **Read compatibility.** `AgentComponentEntry::validate` accepts a digest that matches v4 or v3. The v3 path keeps domain `au-eg/agent-component-definition/v3` and the old input set. No write mints v3.
- **No migration.** The record shape does not change, so `AGENT_COMPONENT_SCHEMA_VERSION` stays 3. Stored v3 rows validate in place. Retire keeps a row's recorded digest.
- **Domain separation.** Distinct domains stop a v4 digest from colliding with a v3 digest over different inputs.
- **One-time revision.** The first import after the upgrade recomputes the server pin under v4. A changed pin issues one ordinary revision for the server and its dependents. Later releases with equal content stay `Unchanged`.
- **Proof.** A unit test proves v4 equality across package versions and v3 read compatibility. The connector-pack test `package_versions_produce_equal_pins_in_independent_stores` returns.

#### Migration and rollback

The current connector pack schema is version 2. Compatible additions keep existing method semantics and require regenerated artifacts. An incompatible version bump must refuse old payloads with a stable code and be coupled to client TCK changes. Agent Library layout changes use a registered layout lineage and named manifest refusal. No implicit destructive upgrade is allowed. Deployment rolls forward with dual-reader compatibility only if explicitly tested. Rollback to an older binary is rejected when its layout cannot read the new owner file. A projection failure leaves the committed pack invisible and re-drivable. ReconcileBodies reclaims only unheld bodies after the grace period. A static schema attachment on another graph remains at its recorded pack revision until an administrator reattaches it.

##### Turtle parse identity repair

- MEDULLA-TYPED-PACKS-R093.1: Scope the public bounded/unbounded Turtle readers per invocation. Keep the existing label-preserving reader for catalogue composition, whose callers already apply document namespaces.
- MEDULLA-TYPED-PACKS-R093.2: Validate the served connector-pack reproject path with colliding member labels and repeatable composed digests. Complete this before claiming parent acceptance.

#### Architecture

##### Existing path to extend

`crates/medulla-catalog-types/src/connector_pack/{index,digest,annotations,ops,result,record}.rs` defines the wire values and bounds. `crates/medulla-capabilities/src/domains/` owns method descriptors; `gen_contract` emits the public contract, receipt and clients. `crates/medulla-h-admin/src/admin/connector_pack/import/` validates and plans; `crates/medulla-persistence-records/src/connector_pack/` owns the head, membership, receipt, bindings, visibility and outbox; `crates/medulla-server-blob/src/blob/engine_bodies.rs` owns bodies; `crates/medulla-h-admin/src/connector_pack_projection.rs` and `crates/medulla-h-admin/src/graph_schema/attach_pack.rs` provide projection and schema attachment. `crates/medulla-h-admin/src/admin/component_content.rs` serves bodies. New code must enter through these paths, not create a second registry or a detached adapter.

##### Data flow and durable boundaries

```mermaid
flowchart LR
  Graft SDK[Graft SDK builds canonical pack] --> UP[Blob upload]
  Graft SDK --> IM[ConnectorPack.Import]
  UP --> IM
  IM --> V[Bounded validation G1–G21]
  V --> B[Engine body store]
  B --> TX[Agent Library owner transaction]
  TX --> H[Head, members, holders, receipt]
  TX --> O[Import outbox]
  O --> P[Projection worker]
  P --> GS[GraphSchema.Attach pack source]
  GS --> VIS[Visible record + outbox ack]
  VIS --> READ[Typed catalog and component reads]
```

The body store can commit before the Agent Library transaction. An orphan has no holder; reclamation follows the grace window. Agent Library rows and outbox intents commit together. Projection is intentionally asynchronous. `visible_record_id` is the read gate until the graph schema has committed. The projection worker reads the current head, not an obsolete event payload; its graph attach uses the pack record as a monotone source revision. The subsequent owner transaction marks visibility and acknowledges the event. If interrupted between commits, redelivery is idempotent.

##### Pack encoding

Entries are a closed kind set: `mcp_server`, `tool`, `skill`, `prompt`, `ontology`, `shapes`, `model_profile`, `a2a_card`. Each entry has a URI, name, media type, raw-body section, optional input/output schema sections, typed annotations and bounded sibling references. Every section carries offset, length and SHA256. Section ranges use checked arithmetic, cover the uncompressed archive once, and never overlap. Text is UTF-8 without BOM; JSON has no duplicate keys, NaN or infinity and is depth/node bounded. `$ref` remains data rather than a network fetch.

`Digest256::framed` hashes length-framed fields in domain-separated order. Nested digest values are 32 raw bytes. String lists are UTF-8 byte sorted and deduplicated. Integers are big-endian u64. Absent optionals encode an empty field. Tri-state booleans use `00` absent, `01` false, `02` true. `entry_digest` frames kind, URI, name, media type, body/schema digests, annotations digest and reference digest under `eg/connector-pack-entry/v1`. `pack_digest` frames connector, server-entry digest, count and URI-sorted entry digests under `eg/connector-pack/v1`. Archive offsets and package version are excluded. Tool input/output schema digests are computed by Medulla over the exact received bytes. The Graft SDK must serialize MCP JSON objects deterministically before byte hashing and replay the shared vectors. The Graft SDK's normalized contract pin is a claim, not a Medulla-verified digest.

##### Identity, lifecycle and reads

The pack head uses `(tenant, connector)` as its key. It includes binding revision, content digest, server pin, package provenance, record ID and projection state. Membership is keyed by pack URI and has a component ID, revision, content digest and lifecycle. Bodies are keyed by SHA256 with holder rows to protect live content. A same-digest import produces no write. A changed entry or pinned server revision creates a new component revision; a removed entry is `Withdrawn`. Historical pinned records may resolve withdrawn revisions; new publication, search, assembly and candidate selection cannot. `Retired` is explicit and permanent, and a later return is refused. Pack-generated `mcp:` IDs cannot be directly published by the generic component method.

`Status` is a snapshot read and has no reap/write side effects. `Content` checks component-read authority and the exact revision and returns verified bytes. Bind/Unbind/Retire/Reproject and body reconciliation require admin rights. Import requires `agent:pack-control` and a matching verified principal and tenant. It also requires an expected head and deterministic operation identity. A mass withdrawal requires an explicit override and admin capability. This means an empty pack or over half of published members.

##### Validation table

| Rule | Required refusal or warning |
|---|---|
| G1 | Supported schema, valid connector/name, sorted unique URIs, closed kinds and URI schemes, exactly one server: `MALFORMED_INDEX` or `UNKNOWN_ENTRY_KIND`. |
| G2 | Index ≤1 MiB, entries ≤1024, references/list ≤64, archive ≤16 MiB, body ≤2 MiB, schema ≤1 MiB, record ≤4 MiB, batch within kernel budget: `PACK_TOO_LARGE`. |
| G3–G5 | Owner-scoped archive present, length/hash match, checked nonoverlapping full sections, all section/entry/pack digests recomputed: `ARCHIVE_MISSING`, `ARCHIVE_DIGEST_MISMATCH`, `MALFORMED_SECTIONS`, `PACK_DIGEST_MISMATCH`. |
| G6 | Unique derived IDs and no decision-owned kind: `DUPLICATE_COMPONENT_ID` or `FORBIDDEN_ENTRY_KIND`. |
| G7–G10 | UTF-8 without BOM. Bounded JSON with unique keys and object tool input schema. Restricted `SKILL.md` front matter with matching name and description. Tool input schema present: `MALFORMED_BODY` or `MISSING_TOOL_SCHEMA`. |
| G11 | Empty description is accepted with entry-name fallback and `EMPTY_DESCRIPTION` warning. |
| G12 | Native capability/modality IRIs have the correct root. Foreign absolute capability IRIs are recorded as claims with warning, not fabricated native facts. Malformed IRI, invalid cost/currency/latency/model bounds refuse as `UNKNOWN_CAPABILITY_IRI`, `INVALID_ANNOTATION` or `INVALID_FACTS`. |
| G13–G16 | Turtle ≤100000 triples, bounded IRI/literal, no `@base`/relative IRI/network import. EL+/RL ontology consistency within derivation budget. Per-file scoped shape blank nodes, no `sh:sparql SERVICE`. Bounded SHACL validation: `ONTOLOGY_INVALID`, `SHAPES_INVALID`, `ONTOLOGY_INCONSISTENT`, `VALIDATION_BUDGET_EXCEEDED` or `SHACL_VIOLATION`. |
| G17–G18 | References resolve to stated sibling kinds with no cycles; server-stamped drafts pass `AgentComponentDraft::validate`: `UNRESOLVED_REFERENCE`, `REFERENCE_CYCLE`, `INVALID_COMPONENT`. |
| G19–G20 | Bound importer matches; mass withdrawal requires explicit admin override: `IMPORTER_MISMATCH`, `PACK_MASS_WITHDRAWAL`. |
| G21 | Stop at 256 violations or exhausted step budget and indicate truncation: `VALIDATION_BUDGET_EXCEEDED`. |

##### Schema source composition

One graph owns a bounded ordered map of `GraphSchemaSource { source_id, origin, revision, shapes, ontology }`. The composed digest derives from all source content and revisions; compiled policy and TBox are caches, never authority. Shapes from separate sources retain separate blank-node scopes; their union is conjunctive. Ontology composition adds source triples to the view's TBox. The shared bounded validator serves `IcvConfigure`, `GraphSchema.Attach` and pack G13–G16.

| Rule | Attach contract |
|---|---|
| K1 | One source owns each shape subject; another source cannot deactivate or change it (`SCHEMA_SOURCE_CONFLICT`). |
| K2–K3 | Shape and ontology documents are bounded, parseable, relative/base free. Imports resolve only within the same source and are never fetched. `SERVICE` constraints are refused. |
| K4 | EL+/RL classification is step bounded; new inconsistency or newly unsatisfiable named class is refused with an explanation. The exponential tableau is never invoked for admission. |
| K5 | Shared class/property IRIs are allowed; consistency, not duplicate naming, determines validity. |
| K6 | Pack/ingestion origin revision strictly increases or is an identical no-op; stale projection refuses `SCHEMA_SOURCE_REGRESSION`. |
| K7 | Source count, bytes and triples remain within configured bounds; overflow is `SCHEMA_SOURCES_TOO_LARGE`. |

`GraphSchema.AttachPack` reads visible engine-owned pack schema, never caller-supplied substitute bytes. `pack__<connector>` is reserved and only the authorized worker writes it. Other graph attachments are static snapshots with a visible record ID; automatic following needs a separately specified durable target list. `IcvConfigure` maps to the operator source, preserving its wire contract. Attach validates the staged schema and later writes; it does not retroactively scan all existing data. The `GraphSchemaClasses` read supplies canonical classes/properties and composition digest for consumers.

##### Failure, security and compatibility decisions

The per-tenant lock serializes pack mutations but does not replace in-write head compare-and-swap. Cancellation is checked before expensive reasoning and before atomic commit; a cancelled request must release the lock. Outbox failure is a durable failed state, not a swallowed log; retry is bounded and does not collateral-dead-letter unrelated events. Projection's graph write uses a grant scoped to that outbox row. Clustered writes for this family are `LocalOnly`. Incompatible owner layout fails by named refusal at the manifest read; an offline upgrade path is rejected. The first schema corpus remains generated from typed Rust authority and preserves its public IRI vocabulary. New source and row types require the same generated contract/receipt update and compatibility tests.

#### Measured model and task facts for harness configuration (R048, R066)

Data flow: evaluation run (Meristem) → pack import of a `model_profile` entry → `AgentComponentFacts::ModelProfile` (`crates/medulla-agent-types/src/agent_component/facts.rs:169`) → `AgentLibraryStore` → `Decide(harness_config)` handler (MEDULLA-DECISION-ENGINE-R128.8) → premises.

Types and fields:

- `contextWindowTokens` is the existing `context_window_tokens` field (`facts.rs:171`). It is a declared fact and needs no run reference.
- `shellProficiency` (`low`, `medium`, `high`) and `stoppingBehaviour` (`stops_early`, `balanced`, `over_verifies`) are new optional fields.
- Each wraps its value in `MeasuredFact<T>`, which holds an `evaluation_run_ref: ObservationRef`. `ObservationRef` is defined at `facts.rs:142` and already backs `latency_observed_ref`.
- `shellCentric` is a boolean on the task class `swarm:TaskShape` (`crates/medulla-core/ontology/swarm_topology-v1.ttl:80`, the class that `swarm:DecomposableTask` extends).

Attach point: the digest function at `agent_component.rs:748` hashes the new fields only when present. An absent field must keep every existing digest vector.

Failure behaviour: a measured value without a run reference fails validation (R048.2, R066.1). An absent measured fact is valid. It makes the decision abstain for the part that needs it (MEDULLA-DECISION-ENGINE-R128.5 to R128.7).

Reused: `ModelProfile` and `ObservationRef`, the pack import path, `agent_ontology::broader_chain` for task-class inheritance. New: `MeasuredFact<T>`, two enums, `harness-v1.shapes.ttl`, `swarm:shellCentric`.

Decisions:

- Measured facts live on the model profile. Rejected alternative: a separate facts table. Facts then travel with the signed pack and its digest.
- An absent `shellCentric` stays absent. Rejected alternative: default `false`. A default `false` turns a missing fact into a `tool_set` answer without a typed abstention.
- The shapes file is new because `harness-v1.ttl` has no shapes sibling. The `swarm:shellCentric` shape extends `swarm_topology-v1.shapes.ttl` because that file validates task shapes.

Cross-repository rows: the `MERISTEM-HARNESS` row for the matched ablation runner writes `shellProficiency` and `stoppingBehaviour` with their run references. The `MEDULLA-DECISION-ENGINE` row R128 reads them.

#### Tool-contract facts on pack annotations (R045)

Data flow: connector Graft SDK pack (`SCION-CONNECTOR-CONTROL-R045.1` to `.11`) → `PackAnnotations` (`crates/medulla-catalog-types/src/connector_pack/annotations.rs:46`) → `check_annotations` (`crates/medulla-h-admin/src/admin/connector_pack/import/annotations.rs:34`) → `tool_facts` (`crates/medulla-h-admin/src/admin/connector_pack/import/facts.rs:21`) → `AgentComponentFacts::Tool` (`crates/medulla-agent-types/src/agent_component/facts.rs:198`) → `request_body` (`crates/medulla-h-decide/src/decide/stat_tool_subset.rs:99`) → `smallest_covering_subset` (`crates/medulla-decision-types/src/decision/statistical/tool_subset.rs:58`) → `HarnessConfig` action space (`MEDULLA-DECISION-ENGINE-R128.9`).

Types and fields: `side_effects` (`none`, `workspace_write`, `external_write`, `network`), `declared_by` (`connector`, `mcp_hint`, `certified`) and `schema_tokens` (positive integer) are new optional fields beside `read_only_hint` (`annotations.rs:58`). Safe defaults when absent: `read_only` false, `side_effects` external_write, `declared_by` connector, `schema_tokens` absent.

Attach points: the digest at `digest.rs:135` hashes the new fields only when present, so old vectors hold. The facts copy at `facts.rs:41` adds the three fields. The cost read at `stat_tool_subset.rs:91` switches from "0 when undeclared" to "no value", and the candidate loop abstains on it.

Failure behaviour: admission refuses an unknown value or a zero size with a typed refusal naming the field. A tool without a size makes the `tool_subset` part abstain. An `mcp_hint` read-only claim is stored unproven and maps to a write effect, so no premise treats it as certified.

Reused: `PackAnnotations`, `tool_facts`, `ToolCoverageCandidate.token_cost` (`tool_subset.rs:20`), `smallest_covering_subset`, `gen_contract --check`. New: three fields, one default method, one admission check, one abstention path.

Decisions:

- The size is `schema_tokens` on the annotation. Rejected alternative: estimate from the schema digest in the engine. The engine then guesses a size that the connector can state.
- A missing size abstains. Rejected alternative: default 0, as `token_cost` does today. A free tool always wins the greedy density order and silently overruns the budget.
- A `read_only` claim from `mcp_hint` stays unproven. Rejected alternative: trust all hints. An MCP hint is a server claim, so parallel execution of such a tool then rests on it.

Cross-repository rows: the `SCION-CONNECTOR-CONTROL-R045` rows publish the facts. The `MERISTEM-HARNESS` row for parallel read-only execution consumes `read_only` and `declared_by`. The `MEDULLA-DECISION-ENGINE-R128` rows consume the size and read-only facts.

##### Capability warning acceptance (R042.2)

The served fixture must select capability warnings by `UnresolvedCapabilityIri`
and the candidate component URI. It must check both exact foreign claim details
and the persisted native/declared provides/requires partition. Unrelated server
description warnings must not affect the capability-warning count. Assertion
failures must retain the full warning list and served refusal details.

##### Current producer boundary (R048.2)

`MeasuredFact<T>` and `ShellProficiency` live in the existing facts module.
The required evaluation reference reuses measured-latency validation at decode
and through `validate`; digest text retains the existing opaque reference rules.
Profile attachment and digest integration are implemented by R048.3 and R048.4.
Signed pack import and readback remain the separate R048.5 leaf.

##### Measured profile fields (R048.3, R048.4)

`ModelProfile` carries optional measured shell proficiency and stopping behaviour.
Both use the existing evaluation reference validator during decode and draft
validation. Each present field adds its name, value, evaluation ID and evaluation
digest to the canonical component digest. An absent field adds no bytes.
The existing capability projector emits `eg:fact/shellProficiency` and
`eg:fact/stoppingBehaviour` only when the corresponding value is present.

The producer tests cover both digest domains, value and provenance changes,
malformed references, enum decoding and the actual projected atoms. Their test
bindings do not establish signed pack import or served readback. R048.5 adds pack model annotations and import mapping for these fields.
Its native served acceptance passed. Separate Go and JavaScript codec parity checks also passed.
R066.1 owns the separate SHACL shapes and evaluation links.
Before integration, regenerate the contract surfaces from the Rust schema source
and check them. Producer tests do not substitute for that check.

#### Skill packages rendered on demand (R100)

Data flow: fleet catalog list wire method → `skill_row` (`crates/medulla-server-consensus/src/dispatch/consensus/fleet_catalog/project.rs:271`) → `render_skill_package` (new) → `FleetCatalogRow::Skill` with a `RenderedSkillPackage` → listing.

Types: `RenderedSkillPackage { uri, digest, ontology_refs, graph_revision }` and `RenderRefusal::DanglingOntologyRef`. The row type `FleetSkillRow` (`crates/medulla-catalog-types/src/fleet_catalog/row.rs:103`) gains an optional rendered package.

Attach point: `skill_row`, called by the catalog projection for each skill. Today the row carries only classification fields.

Failure behavior: a dangling reference lists the skill without a package and names the missing term. Rendering reads the graph and writes nothing.

Reused: the catalog projection, the skill type overrides and the snapshot digest at `project.rs:336`. New: one type, one render function and one refusal.

Decision: render on read from the graph. Rejected: keep static package snapshots, because they fall behind the code between releases.

Cross-repository rows: the `AU-SKILLS` row that consumes the package depends on the contract name `RenderedSkillPackage`.

##### Signed model facts source (R048.5)

The pack model DTO now carries both measured fields. Rust and Python pack digest
writers append present values and evaluation references. Absent values retain
the existing signed digest vector. The normal importer copies the measured facts
into the model profile. The served regression uses the existing signed client,
Blob CAS upload, pack importer and component read operation.

Native acceptance passed with regenerated contract schemas and Python DTOs.
The run passed 105 contract tests, 18 pack tests, and the real served regression.
Both the native method-body codec and server were rebuilt from the integrated source.
The fixture uses the canonical MCP server URI required by the importer.
Structured results are in `evidence/gr1080-r048-5-20261011/results.json`.
Subsequent Go and JavaScript codec parity is recorded in `evidence/gr1080-r048-5-20261011/client-parity.json`.
The regression sends each unknown enum through the raw MessagePack transport.
It expects `INVALID_ARGUMENT` with detail `invalid request encoding`, then checks
that no pack head, receipt, server component or model component exists.
The decoder rejects these enums before signature or digest validation. The test
then submits the original signature with all three camelCase aliases and requires
successful import and canonical readback. Alias decoding must preserve the signed
canonical body. These aliases do not establish R048.1 ontology or Python naming.
Test bindings alone do not establish served acceptance.

#### R071.1 and R097.2: governed vocabulary crosswalks

Candidates EG:34 and EG:58 extend the existing core ontology and domain-pack owners. Schema.org and SKOS provide interoperability terms. They do not become ontology authorities. Medulla retains RDF/OWL semantics, upper-ontology disjointness, schema validation and activation decisions.

Existing code: `VocabularyModule::to_turtle` in `crates/medulla-core/src/ontology_codegen.rs:45` emits deterministic Turtle from typed declarations. Its current vocabulary model has no versioned crosswalk records. `crates/medulla-catalog-types/src/domain_pack.rs` defines HR, legal, medical and government kinds with local consistency validation. Its own module documentation leaves import wiring to later requirements. Neither mechanism proves the new crosswalk contract.

Reuse `core-foundation-v1.ttl` person, event and document terms without changing their IRIs. Reuse `medical-v1.ttl`, `software-v1.ttl` and `hr-v1.ttl` as current domain vocabulary inputs. Extend the typed authority and generator under R090; never hand-edit generated artifacts as the authority. Existing `rdfs:seeAlso` statements provide candidate alignments, not automatic exact matches. Add the missing Technology domain kind. Labor coverage belongs to Hr.

Data flow: typed crosswalk declarations -> generated RDF artifacts -> signed pack import -> schema attachment -> committed schema. Read flow: authenticated OntologyInspect -> revision-bound mappings and coverage. GraphSchemaClasses provides independent class and digest readback.

The production pack route is `handle_connector_pack` -> `serve_import` -> `import::validate_and_commit`. Add validation before any new pack head becomes visible. `crates/medulla-h-admin/src/graph_schema.rs::apply_pack_source` checks composition before schema activation. `crates/medulla-h-sparql/src/rdf/dispatch.rs` routes OntologyInspect to its current inspection handler. Extend that handler and its generated DTO contract. The new domain crosswalk path has no production caller today; these requirements create the missing calls.

Use bounded pinned target declarations, not runtime vocabulary downloads. Preserve explicit relation direction and vocabulary revisions. For broadMatch the target is broader; for narrowMatch the target is narrower. The test specification fixes exact tuples, including conservative SeeAlso references for all three core metadata classes. Document includes physical documents, so its DigitalDocument reference cannot justify an exact match. These fixtures preserve existing core OWL axioms; they do not derive new axioms from crosswalks. Crosswalk relations do not generate identity or class-equivalence entailments. Reject conflicting or category-incompatible declarations before activation. Apply the existing bounded EL+/RL and tableau consistency checks to the resulting schema.

Unmapped reporting covers the exact declared class set of the selected pack revision. A valid pack can contain unmapped classes; coverage must name each one. Resolve each source class against the pinned revision and selected authorized source set. Unknown sources return `CROSSWALK_SOURCE_UNKNOWN`; known sources outside that set return `CROSSWALK_SOURCE_OUT_OF_SCOPE`. A valid target cannot rescue an invalid source. Invalid mappings cannot masquerade as unmapped classes. Rejected publication preserves the old pack head and active schema. Repeated import and restart readback must retain the same digest-bound report.

The acceptance suite uses real signed import, schema attachment and generated client calls against the server. Separate reads inspect committed mappings and list classes. Cover every named domain, all three core metadata classes, unmapped classes, conflicts, bounds, stale revisions and disjointness violations. The plan defines required evidence and claims no runtime completion.

Cross-repository boundary: connectors and applications supply candidates and consume governed pack views. They own no RDF/OWL authority, target-vocabulary activation or duplicate crosswalk registry. This change adds no external source mutation.

The direction contract follows the [SKOS mapping reference](https://www.w3.org/TR/skos-reference/#mapping). Its broadMatch relation points to the broader concept; narrowMatch is its inverse.

##### R066.2 shell-centric task fact

The registered `core:swarm-topology@1` source now declares `swarm:shellCentric` as an optional boolean.
Its shape permits at most one value. It checks both task instances and task-class declarations.
Absent values remain unknown; R066.3 owns inheritance.

Three native tests use the registered shape with Medulla's existing SHACL validator.
They cover absent values, booleans, incorrect datatypes and competing values.
The reference validator passed nine cases. Five malformed cases passed the baseline but fail the updated shape.
Native execution remains pending. Reference validation does not establish native acceptance.
Run `cargo test --locked -p medulla-shacl --test swarm_topology_shapes --offline` on an admitted build lane.


#### Native prompt identity, rendering and blueprint round trips

This definition extends R001, R002 and R094 without changing their original acceptance cells.
Each child names one producer, transition or proving journey.
No runtime scenario receipt accompanies these definitions.
Existing source names establish reuse boundaries, not implementation of the new prompt operations.

##### Original acceptance crosswalk

| Original obligation | Required residual children |
|---|---|
| ConnectorPackIndex and entry types | R001.1 |
| Section and annotation types | R001.2 |
| ConnectorPackOp and AgentComponent.Content shapes | R001.3 |
| Published framed SHA256 sample vectors, byte-for-byte | R001.4 |
| All generated clients share one wire contract | R001.5 |
| SHA256 engine body staging | R002.1 |
| Guarded holder/refcount updates for unchanged components | R002.2 |
| Revised components and historical holders | R002.3 |
| Withdrawn components and historical holders | R002.4 |
| Orphan fault recovery and grace-window reclamation | R002.5 |
| Verified body serving | R002.6 and R094.1 |
| Stable Python-visible content method, never a refusal-only success | R094.1 |

Prompt-specific tests cannot substitute for these original obligations.
Every prior landing receipt and test binding remains evidence at its original scope.

##### Existing code and reachability

Inspection base: `38cae80eb37eba97a273e2a6259e2eb0dfe46b02`.

| Source | Observed role and required extension |
|---|---|
| `crates/medulla-agent-types/src/agent_component.rs:130` | Existing SystemPrompt/McpPrompt kinds, component identity, revisions and dependency pins. Reuse these types. |
| `crates/medulla-agent-types/src/agent_component/facts.rs:395` | Existing static/dynamic variable validation. This does not establish full blueprint validation or rendering. |
| `crates/medulla-h-admin/src/admin/agent.rs:142` | Authenticated component dispatch. Publish binds verified scope before the owner write at line 184. |
| `crates/medulla-persistence-records/src/agent_component/write.rs:39` | Existing component publication authority. Extend it for validated native prompt bodies and replay. |
| `crates/medulla-h-admin/src/admin/component_content.rs:55` | Tenant-scoped content lookup uses connector_pack_content_pointer and verifies engine-owned body bytes. |
| `crates/medulla-server-blob/src/blob/engine_bodies.rs:202` | Existing verified engine-body reader. Reuse its digest and holder authority. |
| `crates/medulla-h-admin/src/connector_pack_projection.rs` | Existing pack projection machinery. Native prompt projection requires an explicit admitted extension. |

The inspected dispatch has Publish, Retire, Current, History, Status, Search and Content branches.
It does not establish a prompt render or blueprint import/export method.
R001.9 must declare and generate those operations before served acceptance.
Native SystemPrompt bodies require R002.7; a connector-pack content pointer alone does not establish that capability.
The intended renderer path in R001.8 is a work target, not an existing implementation.
Do not copy prompt content into component metadata or introduce a second registry.
Graph nodes retain typed identity, revisions, schema and body dependencies; engine-owned bodies retain the complete payload.

##### Typed payload and render policy

A blueprint envelope carries format revision, component identity, exact base revision, schema pin, payload and provenance.
The server derives tenant, actor and permitted scope from verified context.
A schema pin contains component ID, entry revision and definition digest.
The payload requires a nonempty task string.
Optional topic, tone, style, audience, goal, input and format fields follow the pinned schema.
The task remains required when other fields are absent.
The qualification schema identifies itself as prompt-blueprint/v1.
Task contains 1 through 4096 UTF-8 bytes. Optional text fields accept strings or explicit null.
Constraints and deliverables accept arrays of at most 1024 strings; structure accepts a bounded JSON object.
Custom fields preserve bounded JSON values without overriding the declared types of core fields.
The envelope keeps identity and authority fields separate from the custom payload.
Nested structure, constraints, deliverables and custom extension values remain typed JSON data.
The supported schema profile must declare field types, required fields, extension policy and limits.
Unsupported schema rules refuse; no validator silently ignores them.
Schema references resolve only to admitted pinned graph objects, without network access.
Reject duplicate keys, nonfinite numbers, invalid Unicode and exceeded byte, depth or node limits.
Use the existing pack limits as ceilings: body 2 MiB, schema 1 MiB and depth 64.
The qualification profile limits payload and variable trees to 65536 JSON nodes each and rendered output to 2 MiB.
A node is one scalar, array or object; object keys do not add nodes.
The root has depth one; each nested array or object adds one level.
These limits are admission maxima, not performance claims. Test each exact limit and limit-plus-one.

Rendering resolves declared variables from the request or explicitly pinned graph context.
Each binding names one variable and an existing payload path represented by string keys or nonnegative array indices.
Reject duplicate variable names, duplicate target paths and overlapping ancestor/descendant targets.
Bindings replace whole typed values; they never interpolate text or create missing paths.
The binding declaration and variable schema belong to the pinned blueprint revision.
Missing or unknown variables refuse; strings never become executable templates, code, files or network requests.
A variable is inserted as a typed JSON value, with JSON escaping for strings.
Revalidate the completed object against its exact schema before serialization.
The qualification renderer sorts object keys by Unicode scalar value and emits compact UTF-8 JSON without a trailing newline.
It uses required JSON escapes, preserves other Unicode, and normalizes exact decimals without binary floating-point conversion.
Equivalent decimal spellings normalize to the same exact decimal value; zero uses 0 and insignificant fraction zeros are removed.
The rendering revision fixes this serialization policy.
It must preserve numeric values beyond binary64 precision without rounding; unsupported representations refuse.
No host clock, ambient environment or unpinned latest revision may affect rendering.
Return rendered bytes, body and schema pins, variable digest, renderer revision and render digest.
The result is data, not an approval or an authority token.

Lossless export/import means equality of every admitted JSON value, including custom fields, array order and null-versus-absence.
Whitespace and object-key order may differ under the declared serialization policy.
Exact numeric values and Unicode strings must not change.
An export reads a single exact component revision and all dependencies pinned by that revision.
A modified import names that identity and expected base; an admitted change creates a new revision.
An unchanged import follows existing no-op semantics, and a lost-reply retry returns the same operation receipt.
Reserved pack identities still use pack admission; native import cannot bypass importer binding or retirement rules.

##### Producer dependencies and authority

| Consumer obligation | Required producer or boundary | Evidence still required |
|---|---|---|
| R001.6-R001.9 | Existing component authority and generated contract; MEDULLA-CONTRACT-R005 remains a referenced producer. | New schema, supported rules, operations and rendered-result contract. |
| R002.7-R002.9 | Existing owner write, body store and projection admission. | Native prompt holders, graph readiness and authenticated render dispatch. |
| R002.10 | R002.9 render result and the actual Meristem model-dispatch consumer. | Name its owning requirement and production call site before delivery; observe zero invalid model requests. |
| R094.7 | SCION-GOVERNED-WRITEBACK-R004.5 through R004.12 and ARBORA-FLEET-R003.4 through R003.10. | Actual served composition, approved conditional effect, independent readback and durable replay remain required. |

R002.10 must bind the dispatched bytes and variable digest to the rendered result and authorized source revision.
A production consumer must reject a substituted body, schema, scope or revision before model transport.
The integration fixture must traverse the real consumer; a helper counter cannot establish absence of dispatch.
Missing consumer wiring remains an explicit prerequisite, not inferred implementation.
This batch does not edit decision, codec, query or harness sources.
Any related prompt-evolution overlap is a dependency note only, not another absorbed input.

R094.7 retains the original repository-source persistence obligation.
A graph write or exported JSON file does not complete that obligation.
The Graft SDK producer must accept repository identity, allowed path, exact base revision, proposed bytes and their digest.
Arbora must bind human approval to that exact effect, actor and scope before execution.
The producer must refuse absent approval, changed base revision, path traversal and a mismatched payload.
R094.7 is a retained dependency acceptance obligation, not an assignment to implement another repository.
Its receipt must bind the approved request, actual repository revision and written-content digest.
Medulla may retain that receipt as evidence; it must not grant approval or write repository files.
The source-effect definitions now have exact merged producers:

- [Graft SDK governed write-back](https://github.com/Knuckles-Team/agent-connector-sdk/tree/0c6c53dd63fe2a8f9b2ac3568b6d0e8f2c602734/specs/SCION-GOVERNED-WRITEBACK) at `0c6c53dd63fe2a8f9b2ac3568b6d0e8f2c602734`.
- [Arbora fleet composition](https://github.com/Knuckles-Team/graph-os/tree/86737b45aaceb2eebe29b3a98ada5a55f8b71955/specs/fleet-catalog-and-tools) at `86737b45aaceb2eebe29b3a98ada5a55f8b71955`.

SCION-GOVERNED-WRITEBACK-R004.1 through R004.4 retain generic approval, replay, audit reservation and business-rule routing.
R004.5 through R004.8 bind the typed request, trusted path scope, exact approval and provider conditional mutation.
R004.9 and R004.10 bind actual immutable revision, durable attempt and independent readback; R004.11 and R004.12 require recovery and served composition.
Arbora ARBORA-FLEET-R003.4 retains original fleet-wide acceptance. R003.5 through R003.7 bind the full request, trusted configuration and commit confirmation.
R003.8 through R003.10 require real effect/readback evidence, zero-effect refusal parity and durable replay after restart.
ARBORA-OPS-R009 remains that producer's existing invoke authority, not a new Medulla approval boundary.

R094.7 consumes Graft SDK RepositorySourceEffectIntent, RepositorySourceEffectRequest and RepositorySourceEffectReceipt through Arbora RepositoryWriteBackParams.
The existing operation IDs are fleet.write_back.preview and fleet.write_back.commit.
Arbora RepositoryWriteBackPreview carries the actual Graft SDK DryRunObservation; RepositoryWriteBackResult carries the Graft SDK receipt and its digest.
These are defined producer contracts; their names and merge commits do not prove runtime availability.
Their published definitions identify missing exports, provider adapters, registry composition and qualified replay wiring as implementation prerequisites.

Use the exact exported blueprint bytes as Graft SDK payload_base64, with matching SHA256 and byte count.
Bind the immutable repository base, relative path, mode, actor, scope, canonical change-set digest, approval and Graft SDK effect key.
Require request.intent to equal durable.desired_patch["repository_effect"], preserving the Graft SDK's sole-field patch, scope and provenance constraints.
Arbora binds the complete RepositoryWriteBackParams projection and trusted configuration digest to the COMMIT operation and existing confirmation.
A preview-operation lease cannot authorize commit. A Arbora plan reference cannot replace Graft SDK approval or its effect key.
Accept source persistence evidence only from the actual immutable provider revision and independent readback, joined to the durable Graft SDK attempt.
Unknown outcomes require producer reconciliation; an echoed key, applied flag or matching current bytes cannot establish this request's effect.
Medulla retains its original graph identity, revision, complete payload and body authority throughout that journey.
It may store the bound producer receipt as evidence; it neither approves nor performs the repository write.

This resolves the previously unnamed producer-definition dependency for EG:71 without establishing runtime acceptance.
R002.10 still lacks the exact Meristem owning requirement and production model-dispatch call site.
Those must consume the render receipt before transport and prove one valid request versus zero invalid requests.
The source-effect producers do not fill that boundary. No other prompt-evolution or Meristem audit input is absorbed by these references.

##### Source provenance and limitations

The two original field-array hashes were checked against the coordinator recipe before writing.
EG:15 hash: `e58f10a2b4e4daa2525f90a8ac283d43a43bb695c635305900acd420fac669ba`.
EG:71 hash: `6aeae9e1b05f52baa200ab74a9ddca6feb76bbc068a9a92be43802ecedf392a5`.
Both original text sources were available and read in full.
The structured-prompt source motivates schema validation, graph storage, rendering and native typed consumption.
Its performance claims and historical backend descriptions do not establish current implementation or measured quality.
The blueprint source requests complete custom-field preservation across load, modification and persistence.
Its earlier direct-file-write proposal does not override the separate approved-effect boundary.
Neither accepted input supplies a research citation; no research result is claimed here.

<!-- spec-definition-patch:owner-batch-20261012-typed-packs-and-catalog -->
#### Connector packs as native OWL/RDF (R101, R102)

##### Vision

Every connector ships one OWL/RDF ontology pack. The pack holds the connector's domain ontology and its ingest contract. It also holds the connector's features, tools, prompts and skills. The engine imports the pack and stores every element as native relationships in the pack graph. Selection is an engine decision. The engine answers which algorithm, tool or skill applies by SPARQL and `Decide` over that ontology. Connectors and agents ask; the engine decides and computes. The Graft SDK authors the pack from the connector's typed declarations (`SCION-CONNECTOR-CONTROL-R067`). The competency questions in `MEDULLA-CQ-CONN-R084` accept this design.

##### Current state

Ontology and shapes entries are native RDF. They become one graph schema source through `projection_documents_from_store` (`crates/medulla-h-admin/src/admin/connector_pack/reproject.rs:227`).

A connector serves its data sources and mappings as JSON at `connector://resources`. The pack holds that body as a `resource` entry. Import decodes its `schema_mappings` with a closed JSON decoder (`crates/medulla-persistence-records/src/connector_pack/declaration.rs`). Import reads relations from the ontology entries' object properties. Both projections commit beside the declaration member.

`SourceIngest` resolves `connector:<connector>#mappings/<type>` and `connector:<connector>#relations/<resource>/<relation>`. The resolvers are `resolve_connector_schema_mapping` and `resolve_connector_relationship_mapping` (`crates/medulla-persistence-records/src/connector_pack.rs`). The engine holds no manifest entry kind and no YAML parser.

##### Design

The `connector_pack-v1.ttl` vocabulary of R101.1 and R101.2 and the surface triples of R102 remain the planned direction for graph-resident contracts. Until they land, the declaration and the pack ontology are the only mapping authority.

The outbox consumer runs `project` (`crates/medulla-h-admin/src/connector_pack_projection.rs:220`) and `project_head` (`reproject.rs:72`). Projection commit upserts the capability index (`MEDULLA-DECISION-ENGINE-R172.12.3`). `Decide` answers `CapabilityDesignation` questions at `crates/medulla-h-decide/src/decide/stat_decide.rs:137`. Reads use the SPARQL handler (`crates/medulla-h-sparql/src/rdf/sparql.rs:30`) and `OntologyInspect` (R098).

##### Runtime ontology import (R103)

A connector imports or revises its domain ontology at runtime without a full pack import. The served `GraphSchema` method reaches the gateway route at `crates/medulla-h-graph/src/graph_ops/gateway_graph_routes.rs:656`. `handle_gateway` (`crates/medulla-h-admin/src/graph_schema.rs:191`) matches `ImportOntology`. The engine parses and validates the body. It commits one tenant-scoped schema source revision under `ontology:<connector>/<ontology IRI>`. The content digest makes a repeated import a no-op.

#### MEDULLA-TYPED-PACKS-R104 — connect-time seed graph import

Component: Connector packs
References: MEDULLA-TYPED-PACKS-R101, MEDULLA-TYPED-PACKS-R103, [ARBORA-FLEET-R036](https://github.com/Knuckles-Team/graph-os/blob/main/specs/fleet-catalog-and-tools/spec.md), [SCION-CONNECTOR-CONTROL-R068](https://github.com/Knuckles-Team/agent-connector-sdk/blob/main/specs/SCION-CONNECTOR-CONTROL/spec.md)

A connector establishes its contract on first connect. It serves the manifest, the ontology pack and the seed graph as MCP resources (`SCION-CONNECTOR-CONTROL-R068`). The Arbora multiplexer reads them on first connect and on `resources/list_changed` (`ARBORA-FLEET-R036`). The engine owns validation, storage and idempotency.

Call path: Arbora `onboard` -> served `ConnectorPack.Import` -> `run_import` (`import.rs:63`) -> seed validation -> `project_head` (`reproject.rs:72`) -> one commit.

Crates: `medulla-types` for `SeedGraphImport`, `medulla-core` for the seed vocabulary and shapes, `medulla-shacl` for validation, the root server package for the handler.

Data structures: one `BTreeMap<SeedPartKey, SeedPart>` per import. The stored part digests are keyed by tenant, connector and part. A digest compare decides each part before any parse.

Budget: 50,000 triples and 8 MiB per seed graph. Engine time stays under 200 ms per 10,000 triples. These are starting defaults; a measurement on this platform replaces them.

Tenant isolation: the seed graph resolves inside the importing tenant.

No bulk data: the seed graph names objects and their references. Entity rows stay at the source and answer through virtual mappings (`MEDULLA-UNIFIED-DATA-PLANE-R038`).

#### Catalog and governance records (R801 to R805)

| Row | Record set | Producer | Served through |
|---|---|---|---|
| R801 | Glossary terms in SKOS schemes | Glossary pack import, `GlossaryTermPut` | `ARBORA-OPS-R855` |
| R802 | Stewardship roles, change requests, certification | R801, R805 changes | `ARBORA-OPS-R855` |
| R803 | Sensitive data detections as R652 claims | MEDULLA-UNIFIED-DATA-PLANE-R761 profiling | claims of R652 |
| R804 | Ontology revision diff and change impact | R103 import | `OntologyDiff` |
| R805 | Data products, contracts and access requests | Product registration | `ARBORA-OPS-R855` |

Masking enforcement lives in MEDULLA-FEDERATED-QUERY-R706. Quality scores live in
MEDULLA-UNIFIED-DATA-PLANE-R762. Approval admission lives in arbora; the record and the reviewer
choice stay here.

### Open metadata federation (R110)

The catalog of R106 is the system of record. An open-metadata store is a federated secondary through `MEDULLA-SRC-AUTH-R012`; nothing is copied outside the catalog's own nodes. The `cat:openMetadataType` annotation maps open metadata type names to catalog classes, so the ontology drives the mapping. `OpenMetadataRecord` mirrors the open metadata (OMRS) instance header: GUID, type name and version. Inbound sync writes `cat:externalGuid` and `cat:externalVersion`. Outbound proposals carry the last synced version as the expected version. The connector refuses a write over a concurrent store change. The next sync records the conflict. Call path: `Method::SourceSync` → `handle_source_sync` → `OpenMetadataSyncAdapter` (`crates/medulla-catalog-types/src/open_metadata/adapter.rs`). The open-metadata connector `egeria-mcp` owns the pack sources and the version-checked write sink; R110 names its rows.

## Phase 2: Task approach

`tasks.md` holds one task per leaf row, grouped by user story and ordered producer first: a model row precedes the rows that wire it. A row lands in one slice with its caller, so no type lands without a production caller. Tasks that edit the shared dispatch file or `metadata_catalog-v1.ttl` run in series; tasks in different crates run in parallel.

## Complexity Tracking

none

## Build contract

- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` pass for every touched crate.
- The architecture, complexity and duplication gates of `.config/pre-commit.yaml` pass with no baseline entry and no suppression.
- Validation keeps its fixed limits: index 1 MiB, 1024 entries, 64 references per list, archive 16 MiB, body 2 MiB, schema 1 MiB, record 4 MiB, 256 violations.
- Contract generation runs in check mode and the generated artifacts match across every output directory.
- A public item in a pack or catalog crate has a non-test caller under a handler or server crate.
- Spec rows pass `spec-standard`, `plan-conformance` and `spec-decomposition` with zero findings.
- Dispatch stays in the router crate; a handler crate never calls a sibling handler crate.

## Production caller chain

- `Method::ConnectorPack` arm of `dispatch_catalog_admin_methods` (`crates/medulla-server-router/src/dispatch/router/decision_plane.rs:150`) to `handle_connector_pack` (`crates/medulla-h-admin/src/admin/connector_pack.rs:43`) to `serve_import` (`connector_pack.rs:146`) to `validate_and_commit` (`crates/medulla-h-admin/src/admin/connector_pack/import.rs:48`) to `run_import` (`import.rs:66`) to `prepare` (`crates/medulla-h-admin/src/admin/connector_pack/import/planning.rs:95`).
- Commit to projection: the owner transaction leaves an outbox row; `project_head` (`crates/medulla-h-admin/src/admin/connector_pack/reproject.rs:74`) projects the head and `GraphSchema.AttachPack` attaches the pack schema.
- `AgentComponent` ops to `handle_agent_component` (`crates/medulla-h-admin/src/admin/agent.rs:99`) for content, render and receipt verification.
- `GraphSchema` ops (`ImportOntology`, `WithdrawOntology`, `AttachApproved`) to `handle_gateway` (`crates/medulla-h-admin/src/graph_schema.rs:195`).
- Every graph write to `check_before_write` (`crates/medulla-server-api/src/icv_guard.rs:133`), which enforces the composed shapes.
- Obsolete paths to delete: the `MEDULLA_ONTOLOGY_ORPHANS` switch and `OrphanPolicy::Warn` in `import/validation.rs`, and `PackWarningCode::OntologyOrphan` (`MEDULLA-TYPED-PACKS-R105.1`).

## Cross-repo seams

- Skill graph packs: `SCION-CONNECTOR-CONTROL-R071.1` provides, `MEDULLA-TYPED-PACKS-R700.8` consumes. Contract: [skill-graph-pack.md](contracts/skill-graph-pack.md).
- Connector onboarding and seed graphs: `MEDULLA-TYPED-PACKS-R104.3` provides, `ARBORA-FLEET-R036.3` consumes. Contract: [arbora-connector-onboarding.md](contracts/arbora-connector-onboarding.md).
- Glossary and dashboard operations: `MEDULLA-TYPED-PACKS-R801` provides, `ARBORA-OPS-R855` consumes. Contract: [arbora-catalog-operations.md](contracts/arbora-catalog-operations.md).
- Connector conformance evidence: `MEDULLA-TYPED-PACKS-R807` provides, `ARBORA-OPS-R067.10` consumes. Contract: [arbora-conformance-evidence.md](contracts/arbora-conformance-evidence.md).
- Skill upgrade admission: `MEDULLA-TYPED-PACKS-R100.101` provides, `ARBORA-FLEET-R034.9` consumes. Contract: [arbora-skill-admission.md](contracts/arbora-skill-admission.md).
- Repository-source effects for prompt blueprints: `SCION-GOVERNED-WRITEBACK-R004.5` provides, `MEDULLA-TYPED-PACKS-R094.7` consumes. Contract: [governed-writeback-effects.md](contracts/governed-writeback-effects.md).
- Generated ontology artifacts: `MEDULLA-TYPED-PACKS-R090` provides, `ARBORA-DEPLOY-R009.3` consumes. Contract: [arbora-ontology-artifacts.md](contracts/arbora-ontology-artifacts.md).
