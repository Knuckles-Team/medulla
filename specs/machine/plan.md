# MEDULLA-MACHINE — Implementation plan

Governing spec: [spec.md](spec.md). Rows: [requirements.md](requirements.md).

## Summary

The core ontology gains one machine class, seven hardware part classes, an observer link and a lifecycle machine. Pure Rust functions derive identity, evaluate the lifecycle and normalize a hosts file. The Python wheel exposes them. Inventory records enter the tenant graph through the existing `SourceIngest` method. The old host classes retire in the same change.

## Technical Context

- **Language/Version**: Rust 2021 on the pinned toolchain. Python 3.14 (floor 3.12) for served tests.
- **Primary Dependencies**: `medulla-types` (`transition_refused`), `sha2`, `serde`, `serde_yaml_ng`, `medulla-pyengine` (PyO3).
- **Storage**: The engine graph store. No new store.
- **Testing**: Rust unit and integration tests per crate. Served Python tests under `tests/`.
- **Target Platform**: Linux x86_64 and aarch64 without AVX2 assumptions.
- **Project Type**: Ontology modules plus library code in existing crates.
- **Performance Goals**: `machine_iri` p99 under 20 microseconds. `normalize_hosts_file` p99 under 5 ms for 500 hosts. Memory under 4 MiB per call. The bench group is `machine_functions` in `crates/medulla-source-types/benches/machine.rs`.
- **Constraints**: No float in a record. No timestamp outside `:observedAt`. No second class for a machine.
- **Scale/Scope**: 30 rows. Fleets of up to 500 machines.

## Constitution Check

- **`R-I`. The engine is native Rust and the only fact store.**: pass. All logic is Rust. Facts stay in the engine.
- **`R-II`. Every wait has a deadline**: pass. No function waits.
- **`R-III`. Every row sets budgets and names a benchmark**: pass. The Technical Context names both budgets and the bench group.
- **`R-IV`. Crates are small, layered and cut over hard**: pass. The new code is a `machine` module of `medulla-source-types`, split into five files. `medulla-pyengine` gains one dependency that points downward. The old classes retire in the same change.
- **Organization standards**: pass. The lifecycle is an explicit machine. The packs roll up to the core. No generated file is committed.

## Project Structure

```text
crates/medulla-core/ontology/machine-v1.ttl                      new
crates/medulla-core/ontology/machine-v1.shapes.ttl               new
crates/medulla-core/ontology/state-machines-v1.ttl               existing: MachineLifecycle block
crates/medulla-core/ontology/infrastructure-v1.ttl               existing: retire Host, NetworkInterface, GPUAccelerator, hasInterface (replaced by hasNetworkInterface)
crates/medulla-core/ontology/infrastructure-v1.shapes.ttl        existing: shapes target Machine
crates/medulla-core/ontology/company_infra-v1.ttl                existing: retire InfrastructureHost
crates/medulla-core/ontology/core-foundation-v1.ttl              existing: Server comment
crates/medulla-core/ontology/supply_chain-v1.ttl                 existing: range Machine
crates/medulla-core/ontology/placement.ttl                       existing: placement Host extends Machine
crates/medulla-graph-core/src/graph/schema_sources.rs            existing: two CoreSpec entries
crates/medulla-source-types/src/machine/mod.rs                   new
crates/medulla-source-types/src/machine/identity.rs              new
crates/medulla-source-types/src/machine/lifecycle.rs             new
crates/medulla-source-types/src/machine/hosts.rs                 new
crates/medulla-source-types/src/machine/records.rs               new
crates/medulla-source-types/tests/vectors/machine_iri.json       new
crates/medulla-pyengine/src/machine.rs                           new
crates/medulla-pyengine/src/lib.rs                               existing: register functions
crates/medulla-stream/src/telemetry/binding.rs                   existing: Host becomes Machine
crates/medulla-core/tests/machine_vocab.rs                       new
crates/medulla-rdf/tests/machine_competency.rs                   new
crates/medulla-rdf/tests/state_machine_vocabulary.rs             existing: two tests
tests/test_machine_inventory_ingest_served.py                    new
tests/test_machine_wiring.py                                     new
```

## Phase 0: Research

The unknowns are the class name, the key order, the IRI shape, the lifecycle shape and the fate of `core:Server`. [research.md](research.md) resolves each one.

## Phase 1: Design

The vocabulary, the function wire shapes and the identity vectors are in [data-model.md](data-model.md). The shortest proof is in [quickstart.md](quickstart.md).

1. **Owner.** Medulla owns the vocabulary, the identity function, the lifecycle machine and the hosts-file schema. Arbora serves the build. Connectors supply facts.
2. **Cohesion.** Identity, lifecycle, hosts file and records change together with the machine class and live in one module. Telemetry binding changes for another reason and only renames one variant.
3. **Dependency direction.** `medulla-pyengine` depends on `medulla-source-types`. `medulla-source-types` depends on `medulla-types`. No arrow points upward.
4. **Interface depth.** The public surface is six functions. Internals stay private. The new abstraction `MachineDef` is reused from `medulla-types` and has two call sites: `capacity_lease.rs` and `lifecycle.rs`.
5. **Special cases.** A placeholder identity value is absent, not an error path. A host with no hardware key falls to the hostname tier by the same rule.
6. **State.** The lifecycle is an explicit machine of five states and seven edges. The state is a stored property, never a flag.
7. **Environment.** None. The hosts file is data supplied by the caller.

Failure modes: a tier change gives a new IRI, so the build reports the tier. A reconcile batch removes a machine that left the hosts file.

## Phase 2: Task approach

Producers first. The ontology rows (R001, R003, R004) run first. The pure functions (R002, R005, R006.1 to R006.3) come next and run in parallel. R007 follows. R006.4 and R009 prove the ingest last.

## Complexity Tracking

none

## Build contract

- `cargo clippy -- -D warnings`, `cargo nextest run -p medulla-source-types -p medulla-core -p medulla-rdf`.
- KISS limits of the constitution: at most 35 statements per function and 5 levels of indentation. The `derive_event` table is a `match` of five arms.
- `scripts/check_crate_graph.py` passes with the new dependency edge.
- The state-machine-shape hook passes for `lifecycle.rs`.
- No file under `crates/` or `specs/` holds a generated artifact.

## Production caller chain

- Identity: arbora `inventory.build` to `medulla.engine.machine_iri` (`crates/medulla-pyengine/src/machine.rs`) to `machine_iri` in `identity.rs`.
- Lifecycle: `inventory.build` to `machine_lifecycle_derive` and `machine_lifecycle_apply` to `lifecycle.rs`.
- Hosts file: `inventory.build` to `normalize_hosts_file` to `hosts.rs`.
- Records: `inventory.build` to SDK `KnowledgeIngest.submit` to `Method::SourceIngest` to `prepare` in `crates/medulla-h-ingest/src/source_ingestion.rs`.
- Vocabulary: engine startup to `core_specs()` in `crates/medulla-graph-core/src/graph/schema_sources.rs`.

Obsolete paths to drop: `infra:Host`, `core:InfrastructureHost`, `kg:HardwareNode`, `infra:NetworkInterface`, `infra:GPUAccelerator`, `infra:hasInterface` and `EntityClass::Host`.

## Cross-repo seams

none in this repository. Each consumer holds the contract for the seam it uses, in its own `contracts/` directory.
