# MEDULLA-CONTRACT — Implementation plan

Governing spec: [spec.md](spec.md). Rows: [requirements.md](requirements.md).

## Summary

The public engine contract is generated from the Rust method and error declarations, because server dispatch and stable errors are Medulla's authority. Every language projection (Python models, the Go and JavaScript clients, the method codec) binds to one source revision, and the engine serves its method catalog at run time. The live caller is any client of a served method; the owners and reuse candidates are listed under Technical Context.

**State:** PROPOSED. This plan is executable from a fresh public checkout.

## Technical Context

- **Language/Version**: Rust (toolchain pinned by `rust-toolchain.toml`); Python 3.14 with a 3.12 floor for the wheels and tests; Go and JavaScript clients.
- **Primary Dependencies**: maturin and pyo3 for the wheels; the method codec built to WebAssembly for the Go and JavaScript clients.
- **Storage**: N/A (the contract is generated at build time and served at run time).
- **Testing**: `cargo test`, `pytest`, `go test`, `npm test`; proof levels as each row names.
- **Target Platform**: Linux, macOS and Windows on x86_64 and arm64.
- **Project Type**: library and server.
- **Performance Goals**: N/A.
- **Constraints**: the Build contract below.
- **Scale/Scope**: the workspace's crates and every served method in the catalog.

### Existing implementation to reuse

| Authority | Existing component | Rule for changes |
|---|---|---|
| Method and errors | `crates/medulla-capabilities/src/bin/gen_contract.rs`, Rust `ErrorCode` declarations | Extend declarations and generator; never edit generated JSON as the source. |
| Generated catalogs | `contract/methods.json`, `contract/errors.json`, `contract/schemas/`, `contract/fixtures/` | Check digest and fixture parity after regeneration. |
| Python models and package | `medulla/generated/models.py`, `medulla/contract/` | Re-export generated wire classes and verify installed wheel contents. |
| Canonical signing | Rust method codec and `scripts/build_method_codec_wasm.py` | Use its body bytes in Go and JS; do not port canonicalization by hand. |
| Go/JS consumers | `clients/go/`, `clients/js/` | Run vector and error tests against embedded WASM and an installed/served engine. |
| Release checks | `scripts/generated_artifacts.py`, `scripts/check_wheel_completeness.py`, `scripts/check_wheel_privacy.py`, `scripts/ci_parity.sh`, `.github/workflows/release.yml` | Make hosted checks hermetic and source-revision-bound; keep live deployment proof a separate gate. |

### Contract data model

Each method record has a stable `id`, `domain`, `request_schema`, `result_schema`, `policy`, `replay_class`, `stability`, `consumer_profiles`, `is_wire_callable` and `error_set`. Each error record has `code`, `class`, `retryable`, `http_status_hint`. The generator fails if a referenced schema or error is absent, a method ID is duplicated, an allowed wire method has no dispatcher, or a stable code changes meaning without a deliberate compatibility change. The receipt hashes every generated file and identifies the source revision.

The request envelope and signed body are distinct. The codec canonicalizes only the specified body fields. The client attaches identity and transport metadata after encoding. Signatures cover the bytes actually sent. A decoder checks schema before use and maps each error code to its generated typed class. An unknown-code variant supports newer server errors. Unknown fields in strict request/result models still fail closed.

## Constitution Check

Checked against `.specify/memory/constitution.md`: this plan waives no principle. A change that would is recorded under Complexity Tracking before it lands.

## Project Structure

Existing paths only: `crates/` (one crate per subsystem; owners in the architecture map under Phase 1), the root package `src/`, the Python package `medulla/`, `clients/go/`, `clients/js/`, `scripts/` and `.github/workflows/`.

### Fresh checkout developer path

Clone the public repository; install the versions declared by `rust-toolchain.toml`, `Cargo.lock`, `pyproject.toml`, `clients/go/go.mod` and `clients/js/package-lock.json`. Run the repository's normal generators, followed by:

```bash
cargo run -q --locked -p medulla-capabilities --features contract --bin gen_contract -- --check
python3 scripts/build_method_codec_wasm.py --check
go test ./...
npm test
python3 -m pytest -q tests/test_generated_client_contract.py tests/test_contract_method_reachability.py
```

Run Go commands from `clients/go` and npm commands from `clients/js`. If a named Python test is moved, update this plan with its replacement before removing the gate. The complete merge gate is `bash scripts/ci_parity.sh` plus the hosted release workflow. CI must start each integration service from a checked-in container/fixture recipe. The job must tear down each service.

## Phase 0: Research

No unknowns.

## Phase 1: Design

### Design decisions

- Catalog metadata is generated from Rust code because server dispatch and stable errors are Medulla authority.
- The contract receipt binds all language projections to one revision; a package containing current JSON but stale WASM is invalid.
- Hosted CI may split bounded fast checks from long soak. A missing external environment cannot convert a required public gate into a permanent skip.
- Quality scans are differential over changed production code with explicit tracked configuration, and manual scanners remain observable release evidence.

### Capability refusal and decoding contracts

MEDULLA-CONTRACT-R014 publishes the existing governed-actor refusal and routes it only
to GovernedChange, preserving the fail-closed replicated actor boundary.
MEDULLA-IDENTITY-R004 classifies federation:read as an engine user scope. Registry
membership grants no scopes and preserves owner checks and the handler's kg:read
requirement. MEDULLA-DECISION-ENGINE-R063 defines operation-selected typed decision-fit
and decision-eval decoding, including nullable status/receipt results and strict
rejection of wrong operation shapes. The native generator owns all contract
artifacts. Refusal, scope-census, wrapper and byte-for-byte freshness guards
validate their consistency.

Diagnostic and unserved-helper prefixes are not additional wire codes.
The census follows a conservative function/method-reference graph from
Response-returning functions
and Response::err boundaries, includes macro literals, and excludes typed
StatisticalErrorCode diagnostic arguments. Inline cfg(test) modules are excluded
structurally. Regressions retain production before and after test modules,
feature-gated production, called helpers and formatted refusal strings, while
excluding an uncalled helper and typed diagnostic text.

### 2026-10-12: current architecture boundaries

MEDULLA-CONTRACT-R052 documents the current source composition. It proposes no component split, feature change, or release change.
The reviewed source revision is `54aab65786bf72fac94f565796eb21f5d1ec72c4`; all paths below refer to that revision unless updated with new evidence.

#### Model: module owners and dependencies

[Cargo.toml](../../Cargo.toml) owns workspace membership and feature edges.
Derive the inventory by parsing `workspace.members`, then include the root package once.
Expand member globs and honor exclusions if the manifest introduces them.
Do not freeze the current member count into an acceptance test.
Each group below describes responsibility; it does not declare a new deployable unit.

| Responsibility | Existing modules |
|---|---|
| Composition, listeners, policy, persistence adapters | Root `src/` and its `Medulla` library and executable targets |
| Engine kernel: metrics, tracing export, encryption at rest, audit chain, write coalescer, sealed-row rules, row deltas, lock recovery, host autosizing | `medulla-kernel` (depends only on leaf crates, so these edits do not recompile the server) |
| Wire and domain record types, codecs, resources | `medulla-types`, `medulla-protocol-types`, `medulla-identity-types`, `medulla-agent-types`, `medulla-mutation-types`, `medulla-index-types`, `medulla-result-types`, `medulla-catalog-types`, `medulla-source-types`, `medulla-work-types`, `medulla-decision-types`, `medulla-decision-outcome-types`, `medulla-capabilities`, `medulla-method-codec`, `medulla-resource`, `medulla-version` (the single engine release version) |
| Physical storage, mutation authority, graph state | `medulla-storage`, `medulla-storage-owner`, `medulla-transaction`, `medulla-core`, `medulla-graph-core`, `medulla-isolation`, `medulla-durable`, `medulla-direct-state`, `medulla-sqlite-format`, `medulla-store`, `medulla-store-apply`, `medulla-work-store`, `medulla-resource-store`, `medulla-lane-store` |
| Persistence services and records | `medulla-persistence`, `medulla-persistence-ops`, `medulla-persistence-records`, `medulla-placement`, `medulla-raft`, `medulla-raft-xshard` |
| Query and semantic processing | `medulla-plan`, `medulla-exec`, `medulla-uql`, `medulla-nlq`, `medulla-sql`, `medulla-sql-lang`, `medulla-sql-udfs`, `medulla-cypher`, `medulla-tables`, `medulla-table-store`, `medulla-query-cache`, `medulla-federation`, `medulla-virtual-graph`, `medulla-rdf`, `medulla-shacl`, `medulla-shex`, `medulla-graphql`, `medulla-epistemic`, `medulla-reasoner`, `medulla-enrich`, `medulla-semantic-index`, `medulla-retrieval`, `medulla-vocab`, `medulla-catalog`, `medulla-packs` |
| Compute and indexed data | `medulla-compute`, `medulla-numeric`, `medulla-mining`, `medulla-graph-algos`, `medulla-finance`, `medulla-finance-market`, `medulla-finance-quant`, `medulla-ann`, `medulla-ann-codes`, `medulla-geo`, `medulla-tensor`, `medulla-stream`, `medulla-tsdb`, `medulla-text`, `medulla-kvcache`, `medulla-lake` |
| Decisions and competency | `medulla-decision-plan`, `medulla-decision-policy`, `medulla-decision-rank`, `medulla-decision-records`, `medulla-decision-eval`, `medulla-decision-memory`, `medulla-decision-stats`, `medulla-competency`, `medulla-competency-xsrc` |
| Governed media contracts and processing | `medulla-modality`, `medulla-document`, `medulla-image`, `medulla-audio`, `medulla-video`, `medulla-alignment`, `medulla-asr-whisper`, `medulla-tts-piper` |
| Jobs, programs, state machines, extensions, ingest | `medulla-jobs`, `medulla-program`, `medulla-statechart`, `medulla-wasm`, `medulla-pyengine`, `medulla-ingest`, `medulla-repo-index`, `medulla-sdlc`, `medulla-parity` |
| Request handlers (domain-owned dispatch) | `medulla-h-admin`, `medulla-h-compute`, `medulla-h-decide`, `medulla-h-graph`, `medulla-h-ingest`, `medulla-h-jobs`, `medulla-h-mining`, `medulla-h-packs`, `medulla-h-query`, `medulla-h-retrieve`, `medulla-h-sparql`, `medulla-h-txn` |
| Server runtime, wire protocols and dispatch | `medulla-server`, `medulla-server-api`, `medulla-server-auth`, `medulla-server-blob`, `medulla-server-cdc`, `medulla-server-consensus`, `medulla-server-graph`, `medulla-server-identity`, `medulla-server-lake`, `medulla-server-mutation`, `medulla-server-obs`, `medulla-server-ops`, `medulla-server-router`, `medulla-wire`, `medulla-wire-core`, `medulla-wire-msg` |
| Test-only crates | `medulla-persistence-tests`, `medulla-server-cluster-tests`, `medulla-server-served-tests`, `medulla-server-store-tests`, `medulla-sql-tests`, `medulla-store-apply-tests`, `medulla-test-ambient-auth` |
| Native visualization | `medulla-viz-core`, `medulla-viz-columnstore`, `medulla-viz-kernels`, `medulla-viz-export`, `medulla-viz-graph-tiles` |
| Optional quantum processing | `medulla-quantum-core`, `medulla-quantum-gates`, `medulla-quantum-sim`, `medulla-quantum-hardware`, `medulla-quantum-jobs`, `medulla-quantum-workloads` |

The root composes workspace libraries. Lower contracts do not depend on the root facade.
For example, `medulla-types` has no internal dependency; `medulla-modality` and `medulla-capabilities` depend on `medulla-types`.
`medulla-storage` depends on `medulla-types`; `medulla-transaction` depends on both.
`medulla-core` uses graph primitives and optional storage/transaction adapters. `medulla-sql` and `medulla-plan` compose those capabilities.
These examples are manifest observations, not a strict hierarchy for every crate.
Read normal, optional, target-specific, build, and test dependency sections separately when checking an edge.

[The scanner configuration](../../.config/arch-lint.toml) explicitly defines no dependency-direction rule.
Existing scanner success therefore does not prove this dependency direction.
[The component registry](../../architecture/component-registry.yml) remains `draft_unreviewed`; its older measurements do not qualify this map.
The converged-engine design remains owned by [MEDULLA-UNIFIED-DATA-PLANE-R001](../unified-data-plane/requirements.md).

#### Behavior: builds, packages, and deployments

| Boundary | Current source contract | Evidence limit |
|---|---|---|
| Cargo selection | Root `default` includes `full`; `full` includes native visualization. Quantum remains optional. | Old comments excluding visualization do not override feature values. |
| Optional layers | `cluster` extends `full`; `full-extras` extends `full` for additional integrations. | Compilation does not prove listener activation or a deployed cluster. |
| Server executable | The server has a separate no-Python-extension dependency check. | That boundary does not prohibit extensions elsewhere in the wheel. |
| Release wheel | [folded-wheel/action.yml](../../.github/actions/folded-wheel/action.yml) builds `medulla-pyengine/python,security` (the `[tool.maturin]` manifest) and folds in `medulla-numeric/python`; it compiles no server crate or executable. | The recipe requires both kernels through `check_wheel_completeness.py --require-engine-kernel`. |
| Source-build backend | [build_backend.py](../../build_backend.py), `_compose`, folds the numeric kernel at the reviewed revision. | It does not establish parity with the release action's engine-kernel composition. |
| Python extras | [pyproject.toml](../../pyproject.toml) defines `full = []` and `numeric = []`. | These aliases neither change the compiled engine nor select a deployment shape. |
| Runtime shape | A served engine and the embedded binding expose distinct process boundaries. | Configuration, identity, persistence, and runtime checks determine usable capabilities. |

MEDULLA-CONTRACT-R047 already owns rolling-wheel distribution. Package size does not imply a new architectural boundary.

#### Wiring: shared contracts and call sites

| Contract | Existing authority and source route | Owning requirements |
|---|---|---|
| Request identity and policy | `medulla-types` identity records feed server access checks; `medulla-capabilities::policy` declares method policy. Generated clients project the contract. | [MEDULLA-IDENTITY](../engine-identity-and-scope/requirements.md) |
| Artifact identity and provenance | `medulla-modality` exports `OpaqueRef`, `ArtifactBundle`, `EvidenceLocus`, and `Provenance`. `medulla-alignment` relates evidence locations. | [MEDULLA-FEDERATED-QUERY](../federated-query-and-reasoning/requirements.md), especially R039.1; native media coverage remains separate. |
| Epistemic provenance | `medulla-epistemic` operates on graph claims and provenance; `medulla-plan` exposes `KnowledgeSet` and knowledge batches. | [MEDULLA-FEDERATED-QUERY](../federated-query-and-reasoning/spec.md) reasoning and retrieval requirements |
| Physical ownership and mutation | `medulla-storage` owns physical access; `medulla-transaction::MutationKernel` owns admission, ordering, replay, and commit capabilities. | [MEDULLA-DURABLE-KERNEL](../durable-graph-kernel/requirements.md), including R019, R021, and R034 |
| Served mutation | `crates/medulla-server-mutation/src/mutation/commit_entry.rs:264` exports `commit_mutation`; conditional and coalesced routes retain their own entry paths. | [MEDULLA-DURABLE-KERNEL](../durable-graph-kernel/spec.md) |
| Planned query | `crates/medulla-h-query/src/query/planning.rs:325`, `run_unified_with`, binds execution through the existing planner. | [MEDULLA-FEDERATED-QUERY](../federated-query-and-reasoning/spec.md), R038–R055 |
| Direct native access | [engine.md](../../docs/architecture/engine.md) shows dispatch reaching graph state both directly and through planning. | [MEDULLA-DURABLE-KERNEL-R032/R037](../durable-graph-kernel/requirements.md) |

Repository scope is not RBAC scope: `engine-identity-and-scope` owns authorization identity.
Artifact references, principals, graph records, replay identities, and query plans remain distinct typed contracts.
A shared store does not imply one global snapshot or an unbounded transaction across all stores.
R034 retains its cross-store crash and anomaly acceptance. A scope model alone does not satisfy that acceptance.
Federation retains capability-checked pushdown, exact residuals, source grants, budgets, and the shared outbound guard.
This map adds no planner, identity registry, provenance store, or transaction authority.

#### Refusal and evidence

Reject architecture claims that have only a module name, feature flag, diagram arrow, or generated status as support.
Use `status.json` for generator-owned delivery state. Preserve requirement acceptance and public receipts independently.
`VERIFIED` identifies a landed row with a bound test; it does not establish a successful current deployment.
Existing owner tests retain all required served, negative, recovery, and release observations.

#### Diagram coverage and owner follow-ons

[The engine diagram](../../docs/assets/engine-architecture.svg) groups responsibilities; it does not establish conformance for every labeled interface.
The following map covers its five pillars, interface band, planner/mutation band, and durable-store band.
Owner links identify existing definitions. Missing clauses identify documentation work, not absent implementation.
Consult each owner's generated `status.json` for current delivery state and its test specification for acceptance.

| Diagram label | Existing definition owner | Coverage boundary and next definition |
|---|---|---|
| Connected data: graph, RDF, relational, vector, text | [MEDULLA-FEDERATED-QUERY](../federated-query-and-reasoning/requirements.md), R038–R055; [MEDULLA-UNIFIED-DATA-PLANE](../unified-data-plane/requirements.md), R001 and source contracts | Preserve exact results, residuals, grants, and bounded federation. Native query conformance needs its own clauses. |
| Reasoning: OWL, SHACL, ShEx, claims, belief, provenance | [MEDULLA-FEDERATED-QUERY](../federated-query-and-reasoning/spec.md) | ShEx lacks an explicit requirement across the reviewed specs. Use `medulla-shex` and existing reasoning dispatch as source. |
| Analytics: SQL, graph, vector, numeric, programs | [MEDULLA-FINANCE-PRIMITIVES](../finance-primitives/requirements.md), [MEDULLA-DECISION-ENGINE](../decision-engine/requirements.md), and [MEDULLA-FEDERATED-QUERY](../federated-query-and-reasoning/requirements.md) | Separate numeric kernels, UQL programs R027/R065, native `ProgramOptimize`, and MEDULLA-DECISION-ENGINE-R135 compiled-run replay. |
| Signals and time: events, streams, metrics, bitemporal state, CDC | [MEDULLA-FEDERATED-QUERY](../federated-query-and-reasoning/requirements.md), R023; [MEDULLA-DURABLE-KERNEL](../durable-graph-kernel/requirements.md), R078–R081 | Stream grammar does not define event-time, window, or watermark lifecycle. Bitemporal served requirements still need their specified evidence. |
| Multimodal: document, image, audio, video, blobs, evidence | [MEDULLA-FEDERATED-QUERY](../federated-query-and-reasoning/requirements.md), R039.1; [MEDULLA-UNIFIED-DATA-PLANE](../unified-data-plane/requirements.md), R040 | Four-modality retrieval differs from native lifecycle. R040 names text/image/audio; video promotion needs an explicit reconciliation. |
| Authenticated Python interface | [MEDULLA-IDENTITY](../engine-identity-and-scope/requirements.md) | Preserve installed-client, signed-body, strict-decoding, and served authorization evidence. |
| UQL, SQL, SPARQL interfaces | [MEDULLA-FEDERATED-QUERY](../federated-query-and-reasoning/spec.md); [MEDULLA-UNIFIED-DATA-PLANE](../unified-data-plane/requirements.md) | Coverage of a dialect or foreign adapter does not qualify every native interface operation. |
| Cypher, GraphQL, Bolt interfaces | [MEDULLA-FEDERATED-QUERY](../federated-query-and-reasoning/requirements.md) for semantics | R053 and kernel R024.3 cover foreign Cypher. R073 and decision R064 cover other GraphQL uses. Bolt lacks an explicit requirement. |
| Streams interface | [MEDULLA-FEDERATED-QUERY-R023](../federated-query-and-reasoning/requirements.md) and [MEDULLA-DURABLE-KERNEL](../durable-graph-kernel/requirements.md) | Existing owners must define stream lifecycle acceptance. Parser coverage does not prove runtime behavior. |
| Unified planner and governed mutations | [MEDULLA-FEDERATED-QUERY](../federated-query-and-reasoning/spec.md); [MEDULLA-DURABLE-KERNEL-R034/R043](../durable-graph-kernel/requirements.md) | Prove common policy, authority, and state across interfaces. Preserve direct native paths and cross-modal atomicity acceptance. |
| Authoritative durable store | [MEDULLA-DURABLE-KERNEL](../durable-graph-kernel/spec.md); [MEDULLA-IDENTITY](../engine-identity-and-scope/spec.md) | Commit-before-ack, audit, isolation, and optional replication retain their existing owner acceptance. |
| Read-through recovery | [MEDULLA-DURABLE-KERNEL](../durable-graph-kernel/requirements.md) | R039 rebuild and federation R036 index reopening do not explicitly prove safe eviction followed by a durable read. |

Keep each follow-on in its existing owner and split it into five-minute definition children.
Each child must state Model, Behavior, Wiring, Refusal, and Evidence before implementation.
For runtime rows, name the actual production call site and its served integration proof.

| Follow-on owner | Source to reuse | Narrow definition sequence |
|---|---|---|
| Federated query and reasoning | `src/server/bolt_wire/`, native Cypher handlers, `crates/medulla-graphql/`, `crates/medulla-shex/` | Define each native interface separately. Include GraphQL mutations, pagination, federation, and authenticated subscriptions where source supports them. |
| Federated query and reasoning | [modality_serving.md](../../docs/architecture/modality_serving.md) and `crates/medulla-modality/` | Define native ingest, typed predicates, concurrent updates and removals, cold restoration, and evidence-location contracts separately. |
| Decision engine and federated query | [native-program-optimization.md](../../docs/architecture/native-program-optimization.md), `crates/medulla-program/` | Confirm one semantic owner for `ProgramOptimize`; link the other owner. Preserve UQL and compiled-run contracts separately. |
| Unified data plane | R040 and federation R039.1 | Resolve video promotion scope without silently expanding the existing acceptance. |
| Federated query and durable kernel | `crates/medulla-stream/`, `crates/medulla-tsdb/`, existing stream dispatch | Define runtime windows, watermarks, state recovery, and refusal separately from stream grammar. |
| Durable graph kernel | [engine.md](../../docs/architecture/engine.md), `crates/medulla-graph-core/src/read_through.rs`, `src/redb_store/` | The durability owner covers safe eviction and read-through. Add cache-miss and restart observations. |

No follow-on above asserts a passed test or changes a subsystem's acceptance in this batch.

## Phase 2: Task approach

### Work sequence

1. **Catalog audit:** enumerate every Rust-dispatched method and error. Compare generated descriptors to reachable server paths. Correct missing transaction conflict error mapping.
2. **One Python shape:** replace duplicate hand-written DTO definitions with generated re-exports; preserve import compatibility through aliases only where named and tested.
3. **Codec parity:** build Rust WASM from pinned tool versions. Compare both embedded copies. Run Go/JS/Python/Rust vectors and record source and artifact hashes. Reduce size by removing unused decode paths before changing optimization flags.
4. **Package:** include catalogs, schemas and receipt in wheel data. Clean-install into a new virtual environment and verify loader digests and typed operations. Avoid source-tree fallbacks.
5. **CI:** run full root Python tests with the kernel/engine build in hosted CI, static contract reachability, Go/JS tests, all release feature profiles, and differential quality scanners. Tests needing a service start one from checked-in fixtures; deployment/soak has its own documented workflow.
6. **Release:** build the wheel twice in clean environments and compare normalized artifacts. Install the built wheel, run consumer parity and mark a precise commit/tag. Record evidence per requirement.

## Complexity Tracking

None.

## Build contract

The crate split (`MEDULLA-CONTRACT-R056`) must pass the repository quality gates unchanged.
The numbers come from `.config/kiss.toml`, `.config/pre-commit.yaml` and `[tool.medulla.scanners]`.

- **KISS:** at most 35 statements, 8 arguments, 20 local variables and 45 calls per function.
  At most 900 lines, 250 statements and 40 functions per file. Indentation depth 5. No cycles.
- **cccc:** no new function over cyclomatic 10 or cognitive 15, and none may regress.
- **dupehound:** similarity threshold 0.85 with at least 40 tokens. A move must not create a clone.
- **jscpd:** at least 50 tokens and 5 lines in mild mode. New clone pairs fail the gate.

Shapes for every move lane:

- A move is re-export-free. Callers are rewritten to the new path; no `pub use` bridge stays.
- Widen `pub(crate)` to `pub` only where a caller sits in another crate.
- Move helpers, do not copy them. A copied helper fails dupehound and jscpd.
- A lane touches the files of its crate and the callers it rewrites.
- The workspace builds with no dependency cycle before the train is enqueued.

## Production caller chain

A client calls a served method through `medulla.MedullaClient` (Python) or the Go and JavaScript clients, which encode the body with the shared method codec. `crates/medulla-server-router/src/dispatch/router/` dispatches it to the owning handler crate (`medulla-h-*`), which reaches state through `medulla-persistence`; refusal codes are routed by `crates/medulla-capabilities/src/error_routing.rs`. The engine's release version comes from `medulla-version`.

### North star

Deployment comes only from published package releases. Releases ship delivered cross-repository spec bundles. Development is graph-native. The whole ecosystem is ingested into the engine. Developers work through arbora. The repository connector reads code from the graph.

### Delivery phase

Rows `MEDULLA-CONTRACT-R056.14` to `.16` serve P5, scale and breadth. They lower build cost for every later delivery lane.

Crate split: the generated-contract machinery goes first. Then each subsystem moves behind the handler registry (`MEDULLA-CONTRACT-R056.1` to `.10`). Build profiles, test layout and feature layout are part of the split, not follow-ups. A timings report before and after proves the gain.

## Cross-repo seams

- scion-sdk's engine client adapter imports the published `medulla` package's client and request and result models (`SCION-CENTRALIZED-CAPABILITIES-R018`): [contracts/scion-sdk-engine-client.md](contracts/scion-sdk-engine-client.md).

arbora also reads the run-time method catalog, `medulla.engine.method_catalog()`, for its served operations and scope registry (`ARBORA-OPS-R006`, `ARBORA-IDENTITY-R002.1`). This seam has no contract document yet: the catalog row does not yet carry the caller chain and shape that document cites.
