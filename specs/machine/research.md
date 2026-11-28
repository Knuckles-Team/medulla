# MEDULLA-MACHINE — Research

Governing spec: [spec.md](spec.md). Findings are stated as capabilities and techniques. No external project is named.

## Finding 1: the core ontology holds five overlapping machine concepts

The core modules declare these machine-like classes today.

| Class | Module | Role today |
|---|---|---|
| `infra:Host` | infrastructure | A reachable compute node. Anchors host limits, `isHost`, `runsOn` and the supply-chain chain |
| `core:InfrastructureHost` | company infrastructure | A physical or virtual server. Parent of the SSH and orchestrator host classes of two connectors |
| `core:Server` | core foundation | A software server. Also the node type of the engine fleet registry |
| `infra:Node` | infrastructure | An orchestration cluster member, linked to a host by `isHost` |
| `kg:HardwareNode` | none | Named as a parent and a domain in the infrastructure module, but declared by no module |

Three connectors model the same box as unrelated records. A cross-connector question has no shared parent to ask.

## Finding 2: `core:Server` is two concepts

Seven connector packs use `core:Server` as the parent of a software server. Examples are a vault, a search engine, a message broker and a DNS server. The engine fleet registry stores one `Server` node per registered fleet server. A merge into the machine class turns each of those into a machine. Decision: `core:Server` stays and means a software server that offers a service. Only the machine meaning leaves it.

## Decision 1: the canonical class is `core:Machine`

| Candidate | Verdict | Reason |
|---|---|---|
| `Machine` | chosen | Neutral. It covers physical and virtual machines. It names no role, network position or vendor. No class has this name. |
| `Host` | rejected | It names a network role. Two modules and three connectors already use it with different meanings. |
| `Server` | rejected | It names a software role. See Finding 2. |
| `ComputeNode` | rejected | It collides with the orchestration `Node` in meaning and in reading. |
| `Asset` | rejected | It names an accounting view. `AssetInstance` already exists for that view. |
| `System` | rejected | It is overloaded across the platform. |

The class lives in the core namespace as module `machine`, in `crates/medulla-core/ontology/machine-v1.ttl`.

## Decision 2: part class names

`core:Processor` is taken by a harness pipeline hook. The processor part is `core:CentralProcessor`. `infra:NetworkInterface` and `infra:GPUAccelerator` move into the core module as `core:NetworkInterface` and `core:Accelerator`. The infrastructure copies retire in the same change. A second class with the same meaning in two modules is the defect this program removes.

## Decision 3: identity key order

The key order is hardware system UUID, then hardware serial, then management controller ID, then tenant-scoped hostname.

- A system UUID survives re-imaging, renames and address changes. Some firmware reports placeholder values, so a placeholder list refuses them.
- A serial is the next stable value. It is less unique, because some vendors reuse blank or default strings. The same placeholder rule applies.
- A management controller MAC address identifies the chassis even when the OS is down. It changes when the controller board changes.
- A hostname is the weakest key. It scopes to the tenant, because two tenants may both own a host called `alpha`.

Rejected: a random UUID minted at first sight, because a rebuild from the hosts file alone yields new identities. Rejected: the hosts-file name, because the operator renames it freely.

A tier change changes the IRI. The build reports `identityTier` per machine. An operator pins a higher-tier key in the hosts file to avoid a later tier change.

## Decision 4: IRI shape

`urn:machine:<kind>:<first 32 hex digits of sha256(kind LF scope LF value)>`. A URN has no host, so a rename of any domain never touches stored data. A digest hides the raw key in logs and in the IRI. The 128-bit prefix makes a collision negligible at fleet scale.

## Decision 5: lifecycle shape

The five states come from the program decision: Commissioned, Active, Draining, Maintenance, Retired. Retired is the only terminal state. An Active machine cannot retire directly. It must drain and reach Maintenance first. This forces workload evacuation before retirement.

Evidence is a set of six booleans the build derives from probe snapshots. The engine owns the guard logic. The build derives at most one event per machine per build, so a replay produces the same single step.

Alternative rejected: a status field set by an operator. It is a flag, not a machine, and it cannot be replayed.

## Decision 6: records come from a pure function

The build is a pure function of the hosts file and a probe snapshot bundle. Timestamps enter only through the bundle, so a replay writes the same bytes. Canonical JSON with sorted keys fixes the byte form.

## Decision 7: the ingest path

Inventory records enter through the existing source ingestion method `SourceIngest`, as a `Reconcile` batch for the stream `machines`. A reconcile batch removes a machine that left the hosts file. The caller's tenant graph is the target, as for every other connector record.

## Decision 8: functions reach arbora through the wheel

The identity, lifecycle and hosts-file functions are Rust. Arbora must not copy them. The `medulla.engine` module exposes them as pure functions. A replay therefore needs no network and no engine process.

## Open point resolved: a retired flag replaces a deletion

The hosts file may mark a host `retired`. An erased entry makes the next reconcile drop the machine and its history. A flag keeps the history and drives the machine through the lifecycle first.
