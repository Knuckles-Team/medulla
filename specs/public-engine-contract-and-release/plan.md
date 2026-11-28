# MEDULLA-CONTRACT — Implementation plan

Governing spec: [spec.md](spec.md). Rows: [requirements.md](requirements.md).

## Summary

The public engine contract is the Rust method and error declarations, because server dispatch and stable errors are Medulla's authority. The engine serves its method catalog at run time (`medulla.engine.method_catalog()`), so no contract file is generated, committed or packaged. Every language projection binds to the Rust serde types: the hand-written Python models are pinned by round-trip vectors, and the Go and JavaScript clients sign through the shared method codec. The live caller is any client of a served method. The owners and reuse candidates are listed under Technical Context.

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
| Method descriptors and scopes | `crates/medulla-capabilities/src/lib.rs::method_descriptors`, `crates/medulla-capabilities/src/scopes.rs::SCOPES` | Extend the descriptor rows; never copy them into a file. |
| Served catalog | `crates/medulla-capabilities/src/catalog.rs::catalog_json`, `crates/medulla-pyengine/src/lib.rs` (`method_catalog`) | Compute at run time; the digest is `contract_catalog_digest`. |
| Error codes | `crates/medulla-types/src/contract/engine_error_code.rs`, `crates/medulla-capabilities/src/error_routing.rs` | Declare a code once; the method `error_set` derives from it. |
| Python models | `medulla/generated/models.py` and the typed senders in `medulla/generated/*.py` | Re-export wire classes; one class per wire shape. |
| Canonical signing | `crates/medulla-method-codec` and `scripts/build_method_codec_wasm.py` | Use its body bytes in Go and JS; do not port canonicalization by hand. |
| Go and JS consumers | `clients/go/`, `clients/js/` | Run vector tests against the embedded WASM. |
| Release checks | `scripts/check_wheel_completeness.py`, `cargo xtask wheel-privacy`, `scripts/ci_parity.sh`, `.github/workflows/release.yml` | Keep hosted checks hermetic and bound to one source revision. |

### Contract data model

Each method record has a stable `id`, `domain`, `policy`, `replay_class`, `stability`, `consumer_profiles`, `is_wire_callable` and `error_set`. Each scope record has `scope`, `class` and `owner`. The catalog digest covers every method row in method-id order. A decoder checks the model before use and maps each error code to its typed class. A code newer than the installed package maps to an unknown-code variant.

The request envelope and signed body are distinct. The codec canonicalizes only the specified body fields. The client attaches identity and transport metadata after encoding. Signatures cover the bytes sent. Unknown fields in strict request and result models fail closed.

## Constitution Check

Checked against `.specify/memory/constitution.md`: this plan waives no principle. A change that would is recorded under Complexity Tracking before it lands.

## Project Structure

Existing paths only: `crates/` (one crate per subsystem; owners in the architecture map under Phase 1), the root package `src/`, the Python package `medulla/`, `clients/go/`, `clients/js/`, `scripts/` and `.github/workflows/`.

### Fresh checkout developer path

Clone the public repository. Install the versions declared by `rust-toolchain.toml`, `Cargo.lock`, `pyproject.toml`, `clients/go/go.mod` and `clients/js/package-lock.json`. Then run:

```bash
cargo test -p medulla-capabilities --test contract_registry
python3 scripts/build_method_codec_wasm.py --check
go test ./...
npm test
python3 -m pytest -q tests/test_decide_auth_boundary.py tests/test_method_catalog.py
```

Run Go commands from `clients/go` and npm commands from `clients/js`. The complete merge gate is `bash scripts/ci_parity.sh` plus the hosted release workflow. CI starts each integration service from a checked-in fixture recipe and tears it down.

## Phase 0: Research

Five unknowns were resolved in [research.md](research.md): whether a release can be checked without trusting the build host (build twice, compare digests), whether every durable artifact is identified by a content digest (artifacts yes, graph facts no, by design), where the Go and JavaScript method-body vectors come from (generated at test time, not committed), what the method codec's size budget should be (14,000,000 bytes after `wasm-opt`), and whether the four unmaintained date-time crates can leave the dependency graph (yes, replaced by `jiff`, gated by third-party releases).

## Phase 1: Design

### Pre-untangled design

One line per answer.

- **Owner.** Rust method and error declarations in `crates/medulla-capabilities` and `crates/medulla-types` own the public contract; `medulla-version` owns the release version; `.github/workflows/release.yml` and `scripts/check_wheel_completeness.py` own release checks. No other file declares a method, scope or error code.
- **Cohesion.** Contract rows, release checks and dependency hygiene (R064) share one subject: what ships and what a client can rely on. Each lives with its owner above. Advisory policy sits in `.config/deny.toml` and `.config/cargo-audit-allow.txt` beside the rest of the build policy.
- **Dependency direction.** Language projections (Python models, Go and JavaScript clients) point down to the Rust types and the method codec; the Rust types never import a projection. Lower contract crates do not depend on the root facade (see Current architecture boundaries).
- **Interface depth.** One served catalog (`method_catalog()`) and one digest (`contract_catalog_digest`) for every consumer. The workspace's own date-time handling uses one crate (`jiff`); no workspace manifest names `chrono` or `humantime` (R064).
- **Special cases.** None per client or per platform. The only exception list is the advisory ignore list, each entry tied to R064 with an expiry within 90 days, renewed by review. It holds only crates that third-party dependents pull in, and an entry leaves when `cargo tree -i` no longer resolves its crate.
- **State.** No contract file is stored; the catalog is computed at run time. The release build keeps no state beyond its pinned inputs. The advisory ledger is the one tracked list and is dated.
- **Environment.** Builds read pinned inputs only: `rust-toolchain.toml`, `Cargo.lock`, `pyproject.toml`, `go.mod` and `package-lock.json`. Hosted CI runs from checked-in fixture recipes. `cargo deny` reads `.config/deny.toml` and no environment variable.

### Design decisions

- Catalog metadata derives from Rust code, because server dispatch and stable errors are Medulla authority.
- The served catalog digest binds replay identities and consumer catalogs to one revision.
- Hosted CI splits bounded fast checks from long soak. A missing external environment never turns a required public gate into a permanent skip.
- Quality scans are differential over changed production code with explicit tracked configuration. Manual scanners stay observable release evidence.

### Capability refusal and decoding contracts

`MEDULLA-CONTRACT-R014` and `MEDULLA-CONTRACT-R014.1` publish the refusal codes. The governed-actor refusal routes only to governed changes, which keeps the replicated actor boundary fail-closed. `MEDULLA-IDENTITY-R004` classifies `federation:read` as an engine user scope. Registry membership grants no scopes and keeps the owner checks. `MEDULLA-DECISION-ENGINE-R063` defines the typed decoding of decision-fit and decision-eval results. Diagnostic and unserved-helper prefixes are not wire codes.

### Current architecture boundaries

MEDULLA-CONTRACT-R052 documents the current source composition. It proposes no component split, feature change, or release change.

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
| Repository checks and CI tools (`cargo xtask`) | `medulla-xtask` (`tools/xtask`) |
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
| Release wheel | [folded-wheel/action.yml](../../.github/actions/folded-wheel/action.yml) builds `medulla-pyengine/python,security` (the `[tool.maturin]` manifest) and folds in `medulla-numeric/python`; it compiles no server crate; it folds in the platform `medulla-server` executable that the release `server-binary` job compiled once for that platform (`server-binary/action.yml`). | The recipe requires both kernels through `check_wheel_completeness.py --require-engine-kernel`. |
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
| Planned query | `crates/medulla-h-query-read/src/query/planning.rs:325`, `run_unified_with`, binds execution through the existing planner. | [MEDULLA-FEDERATED-QUERY](../federated-query-and-reasoning/spec.md), R038–R055 |
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

### Build speed design (R065 to R069)

- **Owner.** The root `Cargo.toml`, `Cargo.lock` and `scripts/` own the build graph; `tools/xtask` owns the architecture checks. No other repository is touched.
- **Cohesion.** Dependency unification, the embedded-database gate, the size budget, the deploy profile and the workspace-hack crate all change the cost of a build and live in the build files. Crate splits change source layout and stay with the crates they split.
- **Dependency direction.** Each split crate depends only on crates of its own or a lower layer (`pyproject.toml` `cargo_layers`); a new crate takes the layer of the crate it comes from. The workspace-hack crate has no dependents' code in it and points at nothing but third-party crates.
- **Interface depth.** Each split moves modules with their descendants and rewrites callers; no `pub use` bridge stays. The size check has one helper pair (`count_lines`, `over_budget`) with one caller each.
- **Special cases.** One dependency exception (the Kafka client's TLS) is on the allow-list with its reason; no crate has a size exception.
- **State.** `scripts/cargo_lock_duplicates.toml` is the one tracked list and only shrinks; the machines of the split crates keep their edge tables (no lifecycle is added).
- **Environment.** The embedded database library, its version and its checksums are pinned in `scripts/duckdb-pin.toml`; the deploy profile is a Cargo profile; nothing varies by host.
- **CI fetch (R066.5).** One composite action, `.github/actions/duckdb-prebuilt`, fetches the pinned library for `ci.yml` jobs that enable the DuckDB feature (`gates-crates`, `gates-storage`, `gates-pyo3`, `quality-advisory`) through one anchored step; its cache key hashes `scripts/duckdb-pin.toml`. Release jobs fetch inside `scripts/build_release_server.sh`. `tests/test_ci_duckdb_prebuilt.py` parses the workflows and holds the rule.

### Duplicate census (R065.3)

Packages the lock holds in more than one version, other than the array crates that R065.1 unifies. Each row names the dependents that pin each version; `scripts/cargo_lock_duplicates.toml` carries the reason for each that stays.

| Package | Versions and pinning dependents |
|---|---|
| `aes` | 0.8.4 (pulled by aes-gcm); 0.9.4 (pulled by lopdf) |
| `apache-avro` | 0.21.0 (pulled by iceberg); 0.22.0 (pulled by medulla-lake) |
| `atoi` | 2.0.0 (pulled by arrow-cast, sqlx-postgres); 3.1.1 (pulled by arrow-cast) |
| `atoi_simd` | 0.17.0 (pulled by polars-compute, polars-time); 0.18.1 (pulled by calamine) |
| `base64` | 0.22.1 (pulled by arrow-cast, datafusion-functions, iceberg, and 13 more); 0.23.1 (pulled by arrow-cast, hyper-util, medulla-cypher, and 13 more) |
| `bindgen` | 0.69.5 (pulled by espeak-rs-sys); 0.72.1 (pulled by whisper-rs-sys) |
| `bitflags` | 1.3.2 (pulled by defmt); 2.13.2 (pulled by arrow-schema, bindgen, flatbuffers, and 17 more) |
| `block-buffer` | 0.10.4 (pulled by digest); 0.12.1 (pulled by digest) |
| `chacha20` | 0.10.2 (pulled by proptest, rand); 0.9.1 (pulled by chacha20poly1305) |
| `chrono-tz` | 0.10.4 (pulled by arrow-array, datafusion-functions, orc-rust, and 1 more); 0.8.6 (pulled by display-more) |
| `cipher` | 0.4.4 (pulled by aes, aes-gcm, chacha20, and 2 more); 0.5.2 (pulled by aes, cbc, ecb) |
| `comfy-table` | 7.2.2 (pulled by arrow-cast); 8.0.1 (pulled by arrow-cast) |
| `const-oid` | 0.10.2 (pulled by digest); 0.9.6 (pulled by der, tokio-postgres-rustls, x509-cert) |
| `cpufeatures` | 0.2.17 (pulled by aes, argon2, chacha20, and 4 more); 0.3.1 (pulled by aes, chacha20, sha1, and 1 more) |
| `crypto-common` | 0.1.7 (pulled by aead, cipher, digest, and 1 more); 0.2.2 (pulled by cipher, digest) |
| `darling` | 0.20.11 (pulled by derive_builder_core); 0.24.1 (pulled by bon-macros, serde_with_macros) |
| `darling_core` | 0.20.11 (pulled by darling, darling_macro); 0.24.1 (pulled by darling, darling_macro) |
| `darling_macro` | 0.20.11 (pulled by darling); 0.24.1 (pulled by darling) |
| `der` | 0.7.10 (pulled by spki, x509-cert, x509-certificate); 0.8.2 (pulled by ureq) |
| `digest` | 0.10.7 (pulled by apache-avro, blake2, crc-fast, and 4 more); 0.11.3 (pulled by apache-avro, hmac, md-5, and 3 more) |
| `embedded-io` | 0.4.0 (pulled by postcard); 0.6.1 (pulled by postcard) |
| `fallible-iterator` | 0.2.0 (pulled by postgres-protocol, postgres-types, tokio-postgres); 0.3.0 (pulled by duckdb) |
| `foldhash` | 0.1.5 (pulled by hashbrown); 0.2.0 (pulled by datafusion-common, datafusion-functions-aggregate, hashbrown, and 2 more) |
| `fs4` | 0.13.1 (pulled by tantivy); 1.1.0 (pulled by medulla-engine, medulla-h-query, medulla-h-retrieve) |
| `getrandom` | 0.2.17 (pulled by const-random-macro, jsonwebtoken, polars-arrow, and 2 more); 0.3.4 (pulled by ahash, polars-arrow, polars-core, and 1 more); 0.4.3 (pulled by jobserver, lopdf, quinn-proto, and 4 more) |
| `glam` | 0.30.10 (pulled by nalgebra); 0.31.1 (pulled by nalgebra); 0.32.1 (pulled by nalgebra); 0.33.12 (pulled by nalgebra) |
| `hashbrown` | 0.12.3 (pulled by indexmap); 0.14.5 (pulled by dashmap, ordered-multimap); 0.15.5 (pulled by hashlink, petgraph, planus); 0.16.1 (pulled by gimli, hashlink, lru, and 9 more); 0.17.1 (pulled by arrow-array, cranelift-codegen, cranelift-frontend, and 13 more) |
| `hashlink` | 0.10.0 (pulled by duckdb); 0.11.1 (pulled by sqlx-core) |
| `heck` | 0.4.1 (pulled by ouroboros_macro); 0.5.0 (pulled by clap_derive, cranelift-codegen-meta, pyo3-macros-backend, and 3 more) |
| `hmac` | 0.12.1 (pulled by medulla-core, medulla-engine, medulla-federation, and 14 more); 0.13.0 (pulled by aws-sigv4, hkdf, postgres-protocol, and 2 more) |
| `indexmap` | 1.9.3 (pulled by serde_with); 2.14.2 (pulled by arrow-json, datafusion, datafusion-common, and 26 more) |
| `inout` | 0.1.4 (pulled by cipher); 0.2.6 (pulled by cipher) |
| `itertools` | 0.12.1 (pulled by bindgen); 0.13.0 (pulled by bindgen, criterion, criterion-plot, and 2 more); 0.14.0 (pulled by datafusion, datafusion-catalog, datafusion-catalog-listing, and 19 more); 0.15.0 (pulled by object_store, openraft) |
| `libloading` | 0.8.9 (pulled by clang-sys, cudarc); 0.9.0 (pulled by ort) |
| `linux-raw-sys` | 0.12.1 (pulled by rustix); 0.4.15 (pulled by rustix) |
| `lz4_flex` | 0.11.6 (pulled by orc-rust); 0.13.1 (pulled by arrow-ipc, parquet, tantivy); 0.14.0 (pulled by medulla-kvcache) |
| `md-5` | 0.10.6 (pulled by medulla-cypher, medulla-rdf, medulla-sql, and 3 more); 0.11.0 (pulled by lopdf, object_store, opendal-core, and 3 more) |
| `md5` | 0.7.0 (pulled by dust_dds); 0.8.1 (pulled by pgwire) |
| `miniz_oxide` | 0.8.9 (pulled by apache-avro); 0.9.1 (pulled by apache-avro, flate2) |
| `ndarray` | 0.16.1 (pulled by piper-rs); 0.17.2 (pulled by medulla-ann, medulla-finance, medulla-finance-quant, and 5 more) |
| `nom` | 7.1.3 (pulled by cexpr, tantivy-query-grammar); 8.0.0 (pulled by lopdf) |
| `num-bigint` | 0.4.8 (pulled by apache-avro, arrow-buffer, bigdecimal, and 3 more); 0.5.1 (pulled by arrow-buffer) |
| `object` | 0.39.1 (pulled by ar_archive_writer); 0.40.0 (pulled by wasmtime, wasmtime-environ, wasmtime-internal-cranelift, and 1 more) |
| `object_store` | 0.13.2 (pulled by datafusion, datafusion-catalog, datafusion-catalog-listing, and 6 more); 0.14.2 (pulled by medulla-engine, medulla-persistence, medulla-persistence-ops, and 3 more) |
| `ordered-float` | 3.9.2 (pulled by thrift); 4.6.0 (pulled by iceberg); 5.5.0 (pulled by tantivy-query-grammar) |
| `phf` | 0.11.3 (pulled by chrono-tz, chrono-tz-build); 0.12.1 (pulled by chrono-tz); 0.13.1 (pulled by tokio-postgres) |
| `phf_shared` | 0.11.3 (pulled by phf, phf_codegen, phf_generator); 0.12.1 (pulled by phf); 0.13.1 (pulled by phf) |
| `prettyplease` | 0.2.37 (pulled by bindgen); 0.3.0 (pulled by bon-macros) |
| `prost` | 0.13.5 (pulled by orc-rust); 0.14.4 (pulled by medulla-engine, medulla-server, medulla-server-obs, and 5 more) |
| `prost-derive` | 0.13.5 (pulled by prost); 0.14.4 (pulled by prost) |
| `quick-xml` | 0.41.0 (pulled by medulla-core, medulla-document, medulla-rdf, and 3 more); 0.42.0 (pulled by calamine, reqsign-aws-core, reqsign-aws-v4) |
| `r-efi` | 5.3.0 (pulled by getrandom); 6.0.0 (pulled by getrandom) |
| `rand` | 0.10.3 (pulled by apache-avro, lopdf, object_store, and 10 more); 0.8.8 (pulled by medulla-ann, medulla-compute, medulla-decision-policy, and 21 more); 0.9.5 (pulled by apache-avro, datafusion-datasource, datafusion-execution, and 9 more) |
| `rand_chacha` | 0.3.1 (pulled by medulla-ann, medulla-compute, medulla-decision-policy, and 6 more); 0.9.0 (pulled by rand) |
| `rand_core` | 0.10.1 (pulled by chacha20, getrandom, rand, and 2 more); 0.6.4 (pulled by crypto-common, password-hash, rand, and 2 more); 0.9.5 (pulled by rand, rand_chacha) |
| `reqwest` | 0.12.28 (pulled by iceberg, iceberg-catalog-rest); 0.13.5 (pulled by medulla-engine, medulla-quantum-hardware, object_store, and 3 more) |
| `rustc-hash` | 1.1.0 (pulled by bindgen); 2.1.3 (pulled by bindgen, cranelift-codegen, quinn, and 3 more) |
| `rustix` | 0.38.44 (pulled by which); 1.1.6 (pulled by fs4, medulla-direct-state, medulla-engine, and 9 more) |
| `schemars` | 0.9.0 (pulled by serde_with); 1.2.3 (pulled by byte-unit, medulla-agent-types, medulla-capabilities, and 15 more) |
| `sha1` | 0.10.7 (pulled by medulla-compute, medulla-cypher, medulla-engine, and 18 more); 0.11.0 (pulled by reqsign-aws-core, reqsign-core, sqlx-mysql, and 1 more) |
| `sha2` | 0.10.9 (pulled by cranelift-codegen, medulla-agent-types, medulla-ann-codes, and 91 more); 0.11.1 (pulled by aws-sigv4, libduckdb-sys, lopdf, and 5 more) |
| `shlex` | 1.3.0 (pulled by bindgen); 2.0.1 (pulled by cc) |
| `socket2` | 0.5.10 (pulled by dust_dds); 0.6.5 (pulled by hyper-util, quinn, quinn-udp, and 2 more) |
| `strum` | 0.27.2 (pulled by apache-avro, duckdb, iceberg); 0.28.0 (pulled by apache-avro, medulla-protocol-types, medulla-result-types, and 1 more) |
| `strum_macros` | 0.27.2 (pulled by apache-avro, polars-arrow, polars-compute, and 4 more); 0.28.0 (pulled by strum) |
| `syn` | 2.0.119 (pulled by aws-smithy-runtime-api-macros, bindgen, cyclonedds-derive, and 41 more); 3.0.9 (pulled by async-trait, bon-macros, borsh-derive, and 25 more) |
| `thiserror` | 1.0.69 (pulled by cyclonedds, lzokay-native); 2.0.21 (pulled by apache-avro, cobs, defmt-parser, and 47 more) |
| `thiserror-impl` | 1.0.69 (pulled by thiserror); 2.0.21 (pulled by thiserror) |
| `untrusted` | 0.7.1 (pulled by aws-lc-rs); 0.9.0 (pulled by ring, rustls-webpki) |
| `wasi` | 0.11.1+wasi-snapshot-preview1 (pulled by getrandom, mio); 0.14.7+wasi-0.2.4 (pulled by wasite) |
| `wasm-encoder` | 0.258.3 (pulled by wasmtime-environ); 0.262.0 (pulled by wast) |
| `wasmparser` | 0.258.3 (pulled by wasm-encoder, wasmprinter, wasmtime, and 2 more); 0.262.0 (pulled by wasm-encoder) |
| `windows-core` | 0.61.2 (pulled by windows, windows-collections, windows-future, and 1 more); 0.62.2 (pulled by iana-time-zone) |
| `windows-link` | 0.1.3 (pulled by windows, windows-core, windows-future, and 4 more); 0.2.1 (pulled by chrono, jiff, jni, and 6 more) |
| `windows-result` | 0.3.4 (pulled by windows-core); 0.4.1 (pulled by windows-core) |
| `windows-strings` | 0.4.2 (pulled by windows-core); 0.5.1 (pulled by windows-core) |
| `windows-sys` | 0.52.0 (pulled by ring, socket2); 0.59.0 (pulled by fs4, rustix); 0.61.2 (pulled by anstyle-query, anstyle-wincon, console, and 25 more) |
| `zip` | 2.6.1 (pulled by medulla-document, medulla-xtask); 9.8.1 (pulled by calamine) |
| `zstd` | 0.13.5 (pulled by orc-rust); 0.14.3 (pulled by apache-avro, arrow-ipc, iceberg, and 4 more) |
| `zstd-safe` | 7.4.1 (pulled by zstd); 8.2.0 (pulled by zstd) |

### Crate split maps (R067.2 to R067.13)

Sizes are physical lines under `src/` on the current main. Paths use the directory form `parent/{a,b}`; `./` is `src/`, `(root)` is `src/lib.rs`. Each crate keeps its own `lib.rs`; the original `lib.rs` goes to the crate that keeps the original name. The map is a dependency-ordered partition of the static module graph (references through `crate::` and `super::` paths). Each target stays below 22,000 lines. The listed upward references are the only static edges that point to a higher crate; the lane resolves each by moving the referenced item down, confirms with `cargo check`, and updates the map in the same change if a module changes crate.

#### `medulla-decision-types`: 63,097 lines into 4 crates

| Target crate | Lines | Modules (paths under `src/`, `::` form) |
|---|---|---|
| `medulla-decision-statistical-types` | 16,657 | ./{(root),claims,context_facts,decision,failure_patterns,friction,knowledge_curation,knowledge_flag,loops,memory_events,skill_telemetry,source_toc,usage}; decision/{answer_ranking_request,branch_fusion,changeset_review,coding_agent,entity_resolution,entry_types,errors,execution_part,feature_match,held_out,ingest_shortfall,item_recommendation,load_set,numeric,planner_cost_head,planning_admission,primitive,process_decision,promotion_answer,promotion_tier,recursive_trace,reputation,research_voi,size,skill_evolution,task_plan,verifier_receipt}; decision/statistical/{belief,body,capacity,context_search,dataset,declared,enrichment_schedule,errors,execution_routing,features,graph_rows,grounded_eval,ingestion_lane,keyed,lifecycle_record,nl,ranking_eval,retrieval_drift,retrieval_pointer,retrieval_refresh,scale,scorer,shard_monitor,topology,writeback}; decision/task_plan/{aggregation,call,context,control,skill_binding,trajectory_projection,trajectory_refusal} |
| `medulla-decision-policy-types` | 16,649 | decision/{approval,checker_feedback,context_package,delegation,delegation_quality,digest,evidence_stream,harness_contrast,held_item,importance_penalty,jobs,knobs,limit_types,local_certification,memory_benchmark,memory_rewrite,package_validation,policy,program,rapid_r015,record,replay,request,run_governance,safety,skill_upgrade_request,skill_workflow,team_coordination,thought_search,trajectory_ladder,transfer,usage}; decision/statistical/{distilled,feedback,head,retrieval,retrieval_adapter,retrieval_calibration,retrieval_generation,run_credit,tool_subset}; ./{decision_fixtures,skill_eval,solve} |
| `medulla-decision-program-types` | 16,670 | decision/{autonomy,capability_designation,comparative_analysis,context_replacement,curriculum,field_shielding,graph_intake,instruction_disclosure,paper_admission,population,program_store,reward,statistical,temporal_query,tool_action,tool_relevance,trace_fix_binding,trajectory_verdict,usage_guidance}; decision/statistical/{harness_config,log,placement,placement_plan}; decision/task_plan/{branch_breadth,fanout,horizon,pairwise,skill_plan,skill_tree,trajectory,trajectory_budget} |
| `medulla-decision-types` | 13,121 | ./{belief_revision,correction,ontology,participant_memory}; decision/{a2a_route,analyst_synthesis,capability_rank,codes,command_config,context_bundle,context_source,council,defect_triage,indexless_retrieval,ingestion_admission,knob_families,lifecycle,load_set_knobs,merge_arming,model_capacity,path_success,playbook,process_plan,process_remediation,recruit,resolve_conflict,revert_or_fix_forward,run_adjustment,serving_technique,sub_call,tool_necessity,verifier_case_machine}; decision/statistical/{outcome,reasoning_paradigm,retrieval_plan}; decision/task_plan/{critical_path,decompose,tests} |

Upward references to resolve (35 module pairs, 47 references): `decision::record` (crate medulla-decision-policy-types) uses `decision::statistical` (crate medulla-decision-program-types) x4; `decision::record` (crate medulla-decision-policy-types) uses `decision::codes` (crate medulla-decision-types) x3; `decision::package_validation` (crate medulla-decision-policy-types) uses `decision::codes` (crate medulla-decision-types) x3; `decision::harness_contrast` (crate medulla-decision-policy-types) uses `decision::statistical` (crate medulla-decision-program-types) x3; `decision_fixtures` (crate medulla-decision-policy-types) uses `decision::statistical::log` (crate medulla-decision-program-types) x2; `decision::jobs` (crate medulla-decision-policy-types) uses `decision::statistical` (crate medulla-decision-program-types) x2; `decision::statistical::tool_subset` (crate medulla-decision-policy-types) uses `decision::codes` (crate medulla-decision-types) x2; `decision::skill_upgrade_request` (crate medulla-decision-policy-types) uses `decision::statistical` (crate medulla-decision-program-types) x1; `decision::trace_fix_binding` (crate medulla-decision-program-types) uses `decision::codes` (crate medulla-decision-types) x1; `decision::delegation_quality` (crate medulla-decision-policy-types) uses `decision::statistical` (crate medulla-decision-program-types) x1; `decision::program` (crate medulla-decision-policy-types) uses `decision::codes` (crate medulla-decision-types) x1; `decision::package_validation` (crate medulla-decision-policy-types) uses `decision::knob_families` (crate medulla-decision-types) x1; `decision::digest` (crate medulla-decision-policy-types) uses `decision::statistical` (crate medulla-decision-program-types) x1; `decision::limit_types` (crate medulla-decision-policy-types) uses `decision::statistical` (crate medulla-decision-program-types) x1; `decision::usage` (crate medulla-decision-policy-types) uses `decision::codes` (crate medulla-decision-types) x1; `decision::skill_workflow` (crate medulla-decision-policy-types) uses `decision::statistical` (crate medulla-decision-program-types) x1; `decision::skill_workflow` (crate medulla-decision-policy-types) uses `decision::program_store` (crate medulla-decision-program-types) x1; `decision::rapid_r015` (crate medulla-decision-policy-types) uses `decision::codes` (crate medulla-decision-types) x1; `decision::knobs` (crate medulla-decision-policy-types) uses `decision::knob_families` (crate medulla-decision-types) x1; `decision::thought_search` (crate medulla-decision-policy-types) uses `decision::statistical` (crate medulla-decision-program-types) x1; `decision::context_package` (crate medulla-decision-policy-types) uses `decision::statistical` (crate medulla-decision-program-types) x1; `decision::team_coordination` (crate medulla-decision-policy-types) uses `decision::statistical` (crate medulla-decision-program-types) x1; `decision::trajectory_ladder` (crate medulla-decision-policy-types) uses `decision::statistical` (crate medulla-decision-program-types) x1; `decision::statistical::retrieval` (crate medulla-decision-policy-types) uses `decision::statistical::log` (crate medulla-decision-program-types) x1; `decision::statistical::head` (crate medulla-decision-policy-types) uses `decision::statistical::outcome` (crate medulla-decision-types) x1; `decision::statistical::run_credit` (crate medulla-decision-policy-types) uses `decision::codes` (crate medulla-decision-types) x1; `decision::task_plan::fanout` (crate medulla-decision-program-types) uses `decision::knob_families` (crate medulla-decision-types) x1; `decision::task_plan::trajectory_budget` (crate medulla-decision-program-types) uses `decision::codes` (crate medulla-decision-types) x1; `decision::task_plan::trajectory_budget` (crate medulla-decision-program-types) uses `decision::knob_families` (crate medulla-decision-types) x1; `decision::task_plan::skill_tree` (crate medulla-decision-program-types) uses `decision::knob_families` (crate medulla-decision-types) x1; and 5 more pairs in the same form.

#### `medulla-catalog-types`: 28,750 lines into 2 crates

| Target crate | Lines | Modules (paths under `src/`, `::` form) |
|---|---|---|
| `medulla-catalog-source-types` | 14,738 | ./{(root),architecture_view,asset_usage,attached_catalog,attached_source_dialect,attached_source_governance,await_watermark,catalog_relevance,clickhouse_attached_catalog,consolidation_isolation,consumer_contract,data_product,dialect_conformance,distribution,document_source_catalog,embedding_promotion,engine_capabilities,entity_link_proposal,external_schema,foreign_source_catalog,glossary,gramps_pilot_phase,immich_pilot_phase,lake_catalog,lineage,metadata_acceleration,mssql_attached_catalog,mssql_driver_connection,mysql_mariadb_connection,ontology_diff,ontology_edit,ontology_inspection,open_metadata,pg_compat_feature,pg_diff_baseline,pgrx_spike,pgrx_spike_review,postgres_adapter,probe_machine,profile,quality,relational_attached_catalog,schema_boundary,schema_inference,shared_db_platform,skill_admission,skill_edit,sqlite_attached_catalog,sqlite_live_attach,stewardship,virtual_graph_view,web_source} |
| `medulla-catalog-types` | 14,012 | ./{catalog_fixtures,catalog_read,connector_pack,document_conformance,domain_pack,fleet_catalog,foreign_read,gramps_baseline_digest,gramps_cutover_gate,gramps_gap_fix,graph_schema,incident,incident_record_machine,pitr_schedule,replay_conformance,restore_drill,schema_context,warehouse_conformance} |

Upward references: none.

#### `medulla-identity-types`: 25,454 lines into 2 crates

| Target crate | Lines | Modules (paths under `src/`, `::` form) |
|---|---|---|
| `medulla-identity-core-types` | 13,420 | ./{(root),acl,authority,authority_audit,credential_receipt,identity,identity_fixtures,source_coverage,tenancy}; identity/{access,approval_key,audit,config,credentials,delegation,denials,model,ownership,password_policy,requests,requests_admin,requests_provision,scope,segment,stamp,store,tests,text,wire_login}; identity/store/{ownership,session_reference}; identity/tests/{access,auth,credentials,delegation_agent_run,factors,mfa_totp,provision,recovery,revocation,session_evidence,sql_dump,tokens,user_status_machine,verdicts,wire_logins} |
| `medulla-identity-types` | 12,034 | ./{application_admission_machine,assistant_profile,classification,platform_admission,rbac_elevation,resource_grant}; identity/{metadata,ops,policy_bundle,projection,relations,sql_dump,views}; identity/store/{access_ops,approval_keys,auth,authority,credentials,delegation,external,import,invariants,mfa,modes,provision,recovery,sessions,tenant,throttle,tokens,users,verdict,webauthn,wire_logins}; identity/tests/{approval_keys,modes,policy_bundle} |

Upward references to resolve (4 module pairs, 8 references): `identity::store` (crate medulla-identity-core-types) uses `identity::ops` (crate medulla-identity-types) x5; `identity::store` (crate medulla-identity-core-types) uses `identity::views` (crate medulla-identity-types) x1; `identity::store` (crate medulla-identity-core-types) uses `identity::policy_bundle` (crate medulla-identity-types) x1; `identity::requests_provision` (crate medulla-identity-core-types) uses `identity::views` (crate medulla-identity-types) x1.

#### `medulla-decision-stats`: 29,136 lines into 2 crates

| Target crate | Lines | Modules (paths under `src/`, `::` form) |
|---|---|---|
| `medulla-decision-stats-records` | 15,267 | ./{(root),conformal,decision}; decision/{adapter,admission,analyst_synthesis,autonomy,bm25,branch_outcomes,candidate,changeset_review,cluster_knobs,comparative_analysis,context_cursor,context_partition,context_search,correction,curriculum,delegation,distilled,dmn_eval,entity_resolution,evolution_round,feature_match,grounded_eval,importance_penalty,insight,instruction_disclosure,knowledge_curation,knowledge_flag,legal_option_filter,memory,memory_benchmark,memory_rewrite,nl,participant_ask,participant_card,participant_derive,playbook,population,promotion_tier,ranking_eval,recommend,refusal,research_voi,retrieval,reward,route,run_credit,safety,schema_mapping,session_note,skill_eval} |
| `medulla-decision-stats` | 13,869 | decision/{aggregate,branch_select,drift,evaluate,evaluate_bandit,exploration,feature_power,features,fit,fit_calibrate,head_eval,ladder,promotion,quant,rapid_r083,replay,scorer,skill_evolution,skill_plan,skill_telemetry,source_trust,step_verify,targets,temporal_basis,thought_search,tool_relevance,track_record,trajectory,trajectory_join,trajectory_ladder,transfer,usage_guidance,verifier_receipt,web_fetch_selection,writeback}; ./{ope,sparse_select,viewport} |

Upward references: none.

#### `medulla-compute`: 26,004 lines into 2 crates

| Target crate | Lines | Modules (paths under `src/`, `::` form) |
|---|---|---|
| `medulla-compute-assemble` | 13,235 | ./{(root),assemble,contract,datascience,line_merge,node_labels,parser,probabilistic,process_plan,screen,sketch,source_bisect,source_git_objects,source_history,source_symbol_edit,source_tree,test_support}; assemble/{agent,coding_agent,eliminate,facts,model,objective,primitive,profile,prompt,search}; parser/{branch_scope,enrichment_snapshot,enrichment_work_machine,resolve_imports,spec_artifacts,tree_sitter_grammars_extended,tree_sitter_sql,tree_sitter_walk} |
| `medulla-compute` | 12,769 | assemble/{conclude,seal,template,tests,topology,uniqueness,validate,why_not}; ./ast; parser/{branch_index,branch_index_tests,branch_projection,enrichment_admission,enrichment_machine,enrichment_reactivation,resolve,tree_sitter,tree_sitter_ast} |

Upward references to resolve (3 module pairs, 4 references): `contract` (crate medulla-compute-assemble) uses `ast` (crate medulla-compute) x2; `assemble::model` (crate medulla-compute-assemble) uses `assemble::topology` (crate medulla-compute) x1; `assemble::profile` (crate medulla-compute-assemble) uses `assemble::conclude` (crate medulla-compute) x1.

Resolution as landed: the referenced items are moved down as whole modules. `ast::symbol`, `assemble::topology` (with `topology::tables`), `assemble::conclude`, `assemble::template`, `assemble::uniqueness`, `assemble::why_not`, `assemble::seal` and `assemble::validate` live in `medulla-compute-assemble`; the `parser::tree_sitter_*` modules declared by path under `parser::tree_sitter`, and `parser::resolve_imports`, stay with their declaring module in `medulla-compute`.

#### `medulla-h-decide`: 131,816 lines into 7 crates

| Target crate | Lines | Modules (paths under `src/`, `::` form) |
|---|---|---|
| `medulla-h-decide-base` | 19,845 | ./{(root),decide,solve}; decide/{assemble_shield,belief_revise,capacity_premise,claims,connector_points,correction,coverage,coverage_tests,definitions_read,drift_registry,features,guardrail,knowledge_flag,loops,participant_ask,participant_card,participant_derivation,planned_write,program_embedding,prompt_candidates,served_adapter,shape,stat_classes,stat_delegation,stat_generation_space,stat_gold_guarantee,stat_grounded_eval,stat_held_item_critique,stat_memory_change,stat_penalty,stat_placement,stat_proposals,stat_ranking_eval,stat_skill_eval,stat_skill_tier,stat_slate,stat_support,stat_tool_subset,stat_vectors,statistical,telemetry,tool_index,topology_read,topology_schema,triage_read,windows_read}; decide/family/{assemble,classify,connector_event_triage,connector_tool_choice,connector_write_back,pre_tool_risk,template,template_choice}; decide/stat_tests/{a2a_route_tests,belief_revision_tests,capacity_tests,claims_tests,classes_tests,connector_triage_tests,connector_writeback_tests,context_replacement_tests,council_tests,credential_reuse_tests,delegation_tests,graph_intake_tests,ingest_shortfall_tests,kernel_family_support,loops_tests,model_capacity_tests,nl_tests,population_tests,pretool_risk_tests,process_remediation_tests,reasoning_paradigm_tests,revision_proposal_tests,reward_decomposition_tests,run_adjustment_tests,run_observation_tests,schema_mapping_tests,serving_technique_tests,step_verification_tests,synthesis_tests,telemetry_tests,topology_tests,trajectory_join_tests} |
| `medulla-h-decide-runlog` | 19,405 | decide/{assemble,candidates,commit,family,jobs,log,policy_activation,run_record,stat_adapter,stat_capability_outcome,stat_certification,stat_decision_reads,stat_designation,stat_evaluator,stat_executor,stat_generation,stat_harness_adoption,stat_jobs,stat_lifecycle_record,stat_load_set,stat_log,stat_model_capacity,stat_pointer,stat_recommend,stat_reputation,stat_resolve,stat_retention,stat_retrieval,stat_retrieval_views,stat_rule_promotion,stat_skill_ablation,stat_slate_attribution,stat_usage_guidance,stat_verdict_dataset,stat_view,stat_walk_forward,stat_writeback,steps_read}; decide/family/{feedback_loop,support}; decide/stat_tests/{commit_admission_tests,log_tests,retrieval_tests} |
| `medulla-h-decide-learning` | 19,442 | decide/{entry_stage,limit_stage,safety_stage,stat_belief,stat_calibration,stat_capacity,stat_concentration,stat_decide,stat_distilled,stat_feedback,stat_freshness,stat_generation_cycle,stat_harness_contrast,stat_learning,stat_nl,stat_package_run,stat_placement_plan,stat_promotion,stat_replay,stat_run_credit,stat_session_note,stat_task_plan,stat_team,stat_test_support,stat_topology,stat_trajectory}; decide/family/{comparative_analysis,context_package,harness_config,planning_admission,process_decision,process_plan,promotion,route,scale_intent,skill_upgrade,task_context,tool_necessity,tool_relevance}; decide/stat_tests/{assemble_tests,consumer_tests,placement_tests}; ./usage |
| `medulla-h-decide-stat-tests` | 19,788 | decide/{capability_feed,execution_answer,failure_ingest,feature_ledger,friction,knowledge_curation,skill_telemetry,stat_tests,stat_thought_search,triage}; decide/family/{capability_designation,kernel}; decide/stat_tests/{generation_cycle_tests,harness_contrast_tests,learning_tests,path_preference_tests,promotion_tests,rank_tests,scorer_tests,skill_tier_tests,skill_upgrade_tests,skill_workflow_tests,sub_call_admission_tests,task_plan_tests,team_coordination_tests,temporal_basis_tests,thought_search_tests,tool_action_admission_tests,tool_necessity_tests,tool_relevance_tests,tool_subset_tests,trajectory_ladder_tests,trajectory_tests,transfer_tests,usage_guidance_tests,writeback_queue_tests} |
| `medulla-h-decide-family-tests` | 19,744 | decide/stat_tests/{importance_penalty_tests,insight_tests,instruction_disclosure_tests,item_recommendation_tests,knob_tests,knowledge_curation_tests,knowledge_flag_tests,lifecycle_record_tests,limit_guard_tests,load_set_tests,memory_change_tests,merge_arming_tests,org_runtime_tests,over_window_plan_tests,package_run_tests,paper_admission_tests,placement_plan_tests,planning_admission_tests,playbook_triage_tests,policy_activation_tests,probe_selection_tests,probe_verdict_tests,process_decision_tests,process_plan_tests,program_match_tests,program_store_tests,prompt_candidate_tests,recursion_mode_tests,registry_tests,replay_tests,reputation_tests,research_voi_tests,resolve_conflict_tests,resolve_entity_tests,retention_tests,retrieval_adoption_tests,retrieval_eval_tests,retrieval_plan_tests,revert_family_tests,route_tests,rule_promotion_tests,run_credit_tests,runtime_quota_tests,safety_bound_tests,scale_intent_tests,scope_bound_tests,scoped_adapter_tests,session_note_tests,skill_ablation_tests,skill_eval_tests,skill_evolution_tests,skill_telemetry_tests} |
| `medulla-h-decide-families` | 19,944 | decide/family/{schema_mapping,serving_technique,skill_evolution,skill_workflow,step_verification,sub_call_admission,task_plan,team_coordination,temporal_basis,thought_search,tool_action_admission,tool_subset,topology,trajectory_join,trajectory_ladder,usage_guidance}; decide/stat_tests/{answer_ranking_tests,assemble_size_tests,assemble_vocabulary_tests,attribution_tests,autonomy_tests,belief_tests,branch_outcomes_tests,calibration_tests,capability_designation_tests,certification_tests,changeset_review_tests,command_config_tests,commit_tests,comparative_analysis_tests,concentration_tests,connector_toolchoice_tests,context_budget_tests,context_package_tests,correction_admission_tests,correction_tests,curriculum_promotion_tests,defect_triage_tests,delegation_quality_tests,distilled_rank_tests,drift_refresh_tests,entry_trust_tests,er_proposals_tests,evaluator_tests,evolution_round_tests,exploration_pool_tests,failure_patterns_tests,fanout_tests,feature_match_tests,feedback_loop_tests,feedback_tests,field_shielding_tests,freshness_refresh_tests,friction_report_tests,generated_code_admission_tests,grounded_eval_tests,harness_adoption_tests,held_item_admission_tests} |
| `medulla-h-decide` | 13,648 | decide/execution_answer_tests; decide/family/{a2a_route,adversarial_admission,analyst_synthesis,answer_ranking,autonomy,branch_outcome,capacity_proposal,changeset_review,command_config,context_budget,context_replacement,correction_admission,council,credential_reuse,curriculum_promotion,defect_triage,delegation_admission,edit_concentration,enrichment_schedule,evolution_round,fanout_aggregate,fanout_width,feature_match,field_shielding,generated_code_admission,graph_intake,harness_contrast,held_item_admission,ingest_shortfall,ingestion_lane,insight_selection,instruction_disclosure,item_recommendation,load_set,merge_arming,over_window_plan,paper_admission,path_preference,placement,placement_plan,playbook_triage,population_drift,probe_selection,probe_verdict,process_remediation,program_store,rank,reasoning_paradigm,recruit,recursion_mode,research_voi,resolve_conflict,resolve_entity,retrieval_plan,revert_or_fix_forward,reward_decomposition,run_adjustment,runtime_quota} |

Upward references to resolve (15 module pairs, 21 references): `decide::stat_log` (crate medulla-h-decide-runlog) uses `decide::stat_learning` (crate medulla-h-decide-learning) x3; `decide::policy_activation` (crate medulla-h-decide-runlog) uses `decide::stat_promotion` (crate medulla-h-decide-learning) x2; `decide::stat_log` (crate medulla-h-decide-runlog) uses `decide::stat_replay` (crate medulla-h-decide-learning) x2; `decide::stat_support` (crate medulla-h-decide-base) uses `decide::stat_replay` (crate medulla-h-decide-learning) x2; `decide::statistical` (crate medulla-h-decide-base) uses `decide::stat_decide` (crate medulla-h-decide-learning) x2; `decide::stat_retention` (crate medulla-h-decide-runlog) uses `decide::stat_replay` (crate medulla-h-decide-learning) x1; `decide::stat_skill_ablation` (crate medulla-h-decide-runlog) uses `decide::stat_promotion` (crate medulla-h-decide-learning) x1; `decide::stat_memory_change` (crate medulla-h-decide-base) uses `decide::stat_jobs` (crate medulla-h-decide-runlog) x1; `decide::served_adapter` (crate medulla-h-decide-base) uses `decide::stat_adapter` (crate medulla-h-decide-runlog) x1; `decide::stat_ranking_eval` (crate medulla-h-decide-base) uses `decide::stat_jobs` (crate medulla-h-decide-runlog) x1; `decide::candidates` (crate medulla-h-decide-runlog) uses `decide::stat_decide` (crate medulla-h-decide-learning) x1; `decide::stat_generation_space` (crate medulla-h-decide-base) uses `decide::stat_generation` (crate medulla-h-decide-runlog) x1; `decide::stat_generation_space` (crate medulla-h-decide-base) uses `decide::stat_generation_cycle` (crate medulla-h-decide-learning) x1; `decide::stat_grounded_eval` (crate medulla-h-decide-base) uses `decide::stat_jobs` (crate medulla-h-decide-runlog) x1; `decide::stat_tests::retrieval_tests` (crate medulla-h-decide-runlog) uses `decide::stat_tests::consumer_tests` (crate medulla-h-decide-learning) x1.

#### `medulla-h-admin`: 41,256 lines into 2 crates

| Target crate | Lines | Modules (paths under `src/`, `::` form) |
|---|---|---|
| `medulla-h-admin-schema` | 21,757 | ./{(root),admin,agent_library_projection,connector_pack_projection,graph_schema,knowledge_bundle,namespace_migration,raft_admin}; admin/{agent,backup,component_content,concept_lineage,connector_pack,describe_contract,introspect,ownership,package_validation,profile,prompt_receipt,saga,skill_admission,skill_source_edit,workflow_registry}; admin/connector_pack/{catalog_authority,contract_diff,import,reproject,seed,surface_vocab}; admin/connector_pack/contract_diff/{tests}; admin/connector_pack/import/{annotations,capability_tests,catalog_attributes,concept_path,contract_rdf,contract_tests,crosswalk,desired,external_schema,facts,front_matter,json,modality_tests,nutrition,planning,release,seed_entries,skill_files,skill_graph,validation,vocabulary}; admin/connector_pack/import/validation/{core_classes,surface}; admin/connector_pack/reproject/{capability,surface}; agent_library_projection/{map,topology}; connector_pack_projection/{blueprint}; graph_schema/{approved,attach_pack,edits,ontology_diff,ontology_diff_serve,ontology_import}; graph_schema/approved/{tests} |
| `medulla-h-admin` | 19,499 | ./{(root),admin,boot_maintenance,connector_pack_projection,graph_schema}; admin/{cluster,component_content_tests,connector_pack,package_validation_tests,workflow_registry_tests}; admin/connector_pack/{admin,approval,conformance,reconcile,release_approval,reproject,seed_tests,self_served_catalog,tests}; admin/connector_pack/tests/{admin,concept_lineage,content_model,embedding_set,external_schema,model_input_capabilities,ontology,open_metadata_sync,orphan_policy,outbox,pack_source,process_sync,scale,self_served_catalog,skill_package_render,source_approval,source_sync,sql_columns,storage_acceptance,sync_harness,undecodable,withdrawal}; admin/connector_pack/tests/storage_acceptance/{receipt}; connector_pack_projection/{grant,surface_tests}; connector_pack_projection/worker/{native}; graph_schema/{authority_attach_tests,finance_tests,ontology_diff_tests,ontology_import_tests,swarm_topology_tests,terms_served_tests,test_support,tests,virtual_graph_tests,world_model_tests} |

Upward references: none. Items a lower module used from a higher one moved down with their module, or the using module moved up, as the table lists.

#### `medulla-h-ingest`: 39,090 lines into 2 crates

| Target crate | Lines | Modules (paths under `src/`, `::` form) |
|---|---|---|
| `medulla-h-ingest-repository` | 17,278 | ./{(root),asr,atom_context,blob,defect_intake,gate_verdict,harness_session,ingest_drain,ingest_queue,ingestion_admission,ingestion_rung,modality,ontology_slice,repository_enrichment_worker,repository_index,semantic_enrichment,source_batch,source_bisect,source_ingestion}; modality/{dispatch,ingest,migration,response}; repository_enrichment_worker/{grant,raft_top_up}; repository_index/{changeset,delta,ecosystem,enrich,scope_facts}; repository_index/enrich/{fingerprint,hooks}; source_batch/{authorization,tests}; source_batch/tests/{served,support}; source_ingestion/{action_record,bookmark,containers,drift_gate,drift_methods,facts,governance,permissions,policy_candidates,row_version,skill_document} |
| `medulla-h-ingest` | 21,812 | ./{(root),data_prep_declaration,document_delete,ingest_readback,lifecycle_status,lifecycle_tests,lineage,meeting_transcript,paper_ingest,source_changeset,source_history,source_ingestion,source_revert,source_symbol_edit,source_tree,web_ingest}; meeting_transcript/{commit,context,read}; paper_ingest/{cites,map,validate}; source_changeset/{git,provenance}; source_ingestion/{documents,media_derivation,media_tag,proposals,quality,spec_projection,sweep,transcript}; web_ingest/{tests} |

Upward references: none. Items a lower module used from a higher one moved down with their module, or the using module moved up, as the table lists.

#### `medulla-h-graph`: 36,273 lines into 2 crates

| Target crate | Lines | Modules (paths under `src/`, `::` form) |
|---|---|---|
| `medulla-h-graph-gateway` | 18,695 | ./{(root),graph_ops,harness_admission,pipeline,placement,supersession_admission,topology,write_back}; graph_ops/{argument_map,branch,bus,discover_plan,domain_pack,finance_alerts,finance_factors,finance_forecast_gate,finance_performance,finance_receipts,gateway,gateway_broker,gateway_graph,gateway_mining_ml,graph_change_feed,memory_cursor,participant_card,participant_read,policy_source,process_compile,read_span,retention_cleanup,retrieval_feedback,run_failure_defect,source_sync,subgraph,terminal,union,wallet_trust} |
| `medulla-h-graph` | 17,578 | graph_ops/{algorithms,architecture_exchange,assistant_profile,atom_context,broker,context_facts,context_facts_tests,discover_generation_tests,discover_quality,discover_seed_tests,edges,finance_accounting,finance_order_limit_check,finance_order_proposal,finance_records,finance_regime,finance_regime_gate,gateway_graph_routes,harness_edit,harness_edit_tests,harness_session,hierarchy,memory,memory_ingest,memory_snapshot,memory_snapshot_tests,memory_surface,memory_surface_tests,nl_correction,nodes,observation_state,observation_state_tests,participant_derive,participant_memory_tests,participant_surface,process_document,read_span_tests,schema_boundary,semantic,workflow_registry,workflow_registry_tests} |

Upward references to resolve (25 module pairs, 45 references): `graph_ops::terminal` (crate medulla-h-graph-gateway) uses `graph_ops::memory` (crate medulla-h-graph) x8; `graph_ops::terminal` (crate medulla-h-graph-gateway) uses `graph_ops::nodes` (crate medulla-h-graph) x4; `graph_ops::terminal` (crate medulla-h-graph-gateway) uses `graph_ops::broker` (crate medulla-h-graph) x4; `graph_ops::terminal` (crate medulla-h-graph-gateway) uses `graph_ops::algorithms` (crate medulla-h-graph) x4; `graph_ops::terminal` (crate medulla-h-graph-gateway) uses `graph_ops::edges` (crate medulla-h-graph) x3; `graph_ops::gateway` (crate medulla-h-graph-gateway) uses `graph_ops::memory_surface` (crate medulla-h-graph) x2; `graph_ops::participant_read` (crate medulla-h-graph-gateway) uses `graph_ops::participant_derive` (crate medulla-h-graph) x2; `graph_ops::participant_read` (crate medulla-h-graph-gateway) uses `graph_ops::participant_surface` (crate medulla-h-graph) x1; `graph_ops::gateway_graph` (crate medulla-h-graph-gateway) uses `graph_ops::semantic` (crate medulla-h-graph) x1; `graph_ops::terminal` (crate medulla-h-graph-gateway) uses `graph_ops::context_facts` (crate medulla-h-graph) x1; `graph_ops::terminal` (crate medulla-h-graph-gateway) uses `graph_ops::atom_context` (crate medulla-h-graph) x1; `graph_ops::terminal` (crate medulla-h-graph-gateway) uses `graph_ops::architecture_exchange` (crate medulla-h-graph) x1; `graph_ops::terminal` (crate medulla-h-graph-gateway) uses `graph_ops::process_document` (crate medulla-h-graph) x1; `graph_ops::terminal` (crate medulla-h-graph-gateway) uses `graph_ops::memory_surface` (crate medulla-h-graph) x1; `graph_ops::terminal` (crate medulla-h-graph-gateway) uses `graph_ops::assistant_profile` (crate medulla-h-graph) x1; `graph_ops::terminal` (crate medulla-h-graph-gateway) uses `graph_ops::harness_edit` (crate medulla-h-graph) x1; `graph_ops::terminal` (crate medulla-h-graph-gateway) uses `graph_ops::observation_state` (crate medulla-h-graph) x1; `graph_ops::terminal` (crate medulla-h-graph-gateway) uses `graph_ops::semantic` (crate medulla-h-graph) x1; `graph_ops::terminal` (crate medulla-h-graph-gateway) uses `graph_ops::hierarchy` (crate medulla-h-graph) x1; `graph_ops::terminal` (crate medulla-h-graph-gateway) uses `graph_ops::finance_records` (crate medulla-h-graph) x1; `graph_ops::terminal` (crate medulla-h-graph-gateway) uses `graph_ops::finance_accounting` (crate medulla-h-graph) x1; `graph_ops::terminal` (crate medulla-h-graph-gateway) uses `graph_ops::finance_regime` (crate medulla-h-graph) x1; `graph_ops::terminal` (crate medulla-h-graph-gateway) uses `graph_ops::finance_order_limit_check` (crate medulla-h-graph) x1; `graph_ops::terminal` (crate medulla-h-graph-gateway) uses `graph_ops::finance_order_proposal` (crate medulla-h-graph) x1; `graph_ops::finance_performance` (crate medulla-h-graph-gateway) uses `graph_ops::finance_accounting` (crate medulla-h-graph) x1.

#### `medulla-h-query`: 29,493 lines into 2 crates

| Target crate | Lines | Modules (paths under `src/`, `::` form) |
|---|---|---|
| `medulla-h-query-read` | 17,046 | ./{(root),nl,query,request_cancel}; federation/{source_query_tests}; query/{access_port,argument_map,asset_usage,base,catalog_search,code_context,code_map,column_lineage,connector_tool,corpus_retrieve,cypher,document_encoding,entity_link,expansion,explain_handlers,explain_wire_a,explain_wire_b,foreign_leg,foreign_watermarks,freshness_feed,graphql,harness_metrics,node_body,overlay,planning,projection,run_metrics,run_trace_export,schema_context,schema_projections,segment_filter,source_coverage,spec_intelligence,sql_catalog,sql_dispatch,sql_matview,sql_nearest,sql_read,sql_write,standards}; query/planning/{federation_served_tests,plan_deps_tests} |
| `medulla-h-query` | 12,447 | ./{(root),knowledge_stream,query,streaming}; knowledge_stream/{families,stream,tests}; query/{bookmark_relations,catalog_glossary,catalog_governance,catalog_read,context_compile,context_search,dispatch,indexless,live_artifact,nl,retrieval_round,search_task,state_machine_next,stored_query,uql_statement,viewport}; query/dispatch/{edge_index} |

Upward references: none. Items a lower module used from a higher one moved down with their module, or the using module moved up, as the table lists.

#### `medulla-server-router`: 27,064 lines into 2 crates

The static map put the request-boundary entry points, the seam and `router.rs` in the lower crate, but the entry
points call the composed handler registry, which names every arm. A lower crate may not name a higher one, so those
three modules, which compose the arms, stay in the crate that keeps the name. The `sdlc` family moves down with the arms
that call its provenance check, and `read_path_history` moves down with the arms that read it.

| Target crate | Lines | Modules (paths under `src/`, `::` form) |
|---|---|---|
| `medulla-server-router-arms` | 15,967 | ./{(root),dispatch}; dispatch/{incident,quality,request_boundary,router,telemetry}; dispatch/request_boundary/{authorization,consensus,delegated_actor,method_errors,saga}; dispatch/router/{bookmark,bookmark_binding,branch_merge,branch_write,catalog_emit,changeset_index,channels,data_plane_arms,git_export_pack,git_refs_list,graph_source,import_git_history,incident,knowledge_bundle,lifecycle,list_path_grants,memory_change,memory_events,quality,read_blame,read_path_history,read_tree,resource_cost,revert_changeset,schema_drift,sdlc,service_control,set_path_grant,share,source_bisect,source_ingest,submit_changeset,submit_symbol_edit,telemetry} |
| `medulla-server-router` | 11,636 | ./{(root),dispatch}; dispatch/{dispatch_e2e_tests,replicated_tenant_tests,request_boundary,router,seam,test_support}; dispatch/router/{control_plane,data_plane,data_product,decision_plane,graph_lifecycle,identity_access} |

Upward references: none.

#### `medulla-server-served-tests`: 30,400 lines into 3 crates

| Target crate | Lines | Files under `src/` |
|---|---|---|
| `medulla-served-test-harness` | about 2,600 | `lib.rs` modules `refusal_tap` and `log_capture`; `split_tests/connector_pack_harness.rs`; `split_tests/semantic_live_generation.rs` |
| `medulla-server-served-handler-tests` | 10,640 | every `split_tests/medulla_h_*` file |
| `medulla-server-served-tests` | about 17,000 | `tests.rs`, `tests/` (4 files), the other `split_tests` files, `split_tests.rs` |

## Phase 2: Task approach

### Work sequence

1. **Catalog:** the descriptor rows and error sets back the served catalog (R044, R054, R057).
2. **One Python shape:** one generated class per wire shape; strict models pinned by vectors (R001.3 to R001.5, R005, R048).
3. **Codec parity:** build the WASM from the pinned toolchain; compare both client copies; replay vectors in Rust, Python, Go and JS (R004, R006.x).
4. **Package:** one `medulla` distribution per platform, with the server folded in (R053.x).
5. **CI:** the full root Python suite, static gates and the feature matrix run in hosted CI (R009, R018, R037).
6. **Split gates:** crate graph, build profiles and spec path checks (R056.x).
8. **Build speed:** unify dependencies (R065) and gate the embedded database (R066) first, independent of each other; then the size gate (R067.1); then the twelve splits (R067.2 to R067.13), types crates first; then enforce the gate (R067.14); the workspace-hack crate (R069) last because it depends on the final crate set; the deploy profile (R068) any time.
7. **Dependency hygiene:** the workspace's own manifests move to one maintained date-time crate (`jiff`). The four advisory ignores stay in `.config/deny.toml` as a reviewed ledger for the copies that third-party dependents pull in (R064).

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
- arbora reads the run-time method catalog `medulla.engine.method_catalog()` for its served operations and scope registry (`ARBORA-OPS-R006`, `ARBORA-IDENTITY-R002.1`): [contracts/arbora-method-catalog.md](contracts/arbora-method-catalog.md).
