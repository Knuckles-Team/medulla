# MEDULLA-MACHINE — One machine concept, hardware parts, lifecycle and inventory records

Owner: Medulla. Cross-repo IDs: ARBORA-OPS-R959 (arbora, build and describe), SCION-CENTRALIZED-CAPABILITIES-R057 (scion-sdk, observer helper), CONN-TUNNEL-MANAGER-INVENTORY-R008, CONN-CONTAINER-MANAGER-MCP-R025, CONN-FAN-MANAGER-R006, CONN-SYSTEMS-MANAGER-CONNECTOR-CONTENT-R006, CONN-ARCHIVEBOX-API-CONNECTOR-CONTENT-R010, CONN-ANSIBLE-TOWER-MCP-CONNECTOR-CONTENT-R003, CONN-SERVICENOW-API-CONNECTOR-CONTENT-R012 (connector roll-ups).
Public provenance: the operator decision to model inventory as native graph data.

## Delivery mapping

Component: Machine inventory vocabulary

Extends: [Supply-chain vocabulary](../supply-chain-vocabulary/spec.md)
References: [Placement](../placement-and-worker-policy/spec.md)
Code: ../../crates/medulla-core/ontology/machine-v1.ttl

## User Scenarios & Testing

### User Story 1 - One machine for every connector view (Priority: P1)

An operator asks about a machine and gets one answer. Three connectors describe the same box, and each links its record to one `core:Machine`.

**Acceptance Scenarios**:

1. **Given** three connector records for one machine, **When** the operator asks who observes machines, **Then** one machine lists three observers.
2. **Given** two tenants with a host named `alpha`, **When** both build, **Then** each tenant holds a different IRI.

### User Story 2 - A stable identity from hardware facts (Priority: P1)

An operator renames a host and changes its address. The machine keeps its IRI because its hardware UUID did not change.

**Acceptance Scenarios**:

1. **Given** a machine with a valid system UUID, **When** the IRI derivation runs, **Then** it returns the UUID-tier IRI of the test vector.
2. **Given** a firmware placeholder UUID and a valid serial, **When** the derivation runs, **Then** it uses the serial tier.

### User Story 3 - A lifecycle that evidence drives (Priority: P1)

An operator sees which machines are Active, Draining or in Maintenance, and sees the evidence for each change. An illegal change is refused.

**Acceptance Scenarios**:

1. **Given** an Active machine with a cordoned node, **When** the lifecycle derives its event, **Then** the event is `Drain`.
2. **Given** an Active machine, **When** a `Retire` event arrives, **Then** the machine refuses with `TRANSITION_REFUSED`.

### User Story 4 - Generic hardware parts (Priority: P2)

An operator lists the processors, memory, disks, interfaces, bonds, accelerators and management controller of a machine from one vocabulary.

**Acceptance Scenarios**:

1. **Given** two interfaces in one bond, **When** the records load, **Then** the bond links both as members.
2. **Given** a memory module with no capacity, **When** the shape validates it, **Then** the record is refused.

### User Story 5 - A public hosts file (Priority: P2)

An operator writes a small YAML hosts file. The engine validates it, refuses a secret value and returns normalized records.

**Acceptance Scenarios**:

1. **Given** a hosts file with a `password` key, **When** it normalizes, **Then** it refuses with `HOSTS_FILE_INVALID` and reason `secret_field`.
2. **Given** two valid files that list the same hosts in different order, **When** they normalize, **Then** both return identical bytes.

### Edge Cases

- A system UUID of all zeros or all `F` digits is a placeholder and counts as absent.
- A hosts file with two entries of the same name refuses with reason `duplicate_name`.
- A host with no address refuses with reason `no_address`.
- A host that leaves the hosts file disappears at the next reconcile. A host marked `retired` stays and drains first.
- A machine whose best identity key changes tier gets a new IRI. The build reports the tier.

## Requirements

- **FR-001**: The core ontology holds one machine class, `core:Machine`, and no retired host class. Rows: `MEDULLA-MACHINE-R001`, `MEDULLA-MACHINE-R001.1`, `MEDULLA-MACHINE-R001.2`, `MEDULLA-MACHINE-R001.3`, `MEDULLA-MACHINE-R001.4`.
- **FR-002**: A pure function derives a stable IRI from ordered identity keys. Rows: `MEDULLA-MACHINE-R002`, `MEDULLA-MACHINE-R002.1`, `MEDULLA-MACHINE-R002.2`, `MEDULLA-MACHINE-R002.3`.
- **FR-003**: Seven part classes and `hasComponent` model the hardware. Rows: `MEDULLA-MACHINE-R003`, `MEDULLA-MACHINE-R003.1`, `MEDULLA-MACHINE-R003.2`, `MEDULLA-MACHINE-R003.3`.
- **FR-004**: Any connector record links to its machine with `observes`. Rows: `MEDULLA-MACHINE-R004`, `MEDULLA-MACHINE-R004.1`, `MEDULLA-MACHINE-R004.2`.
- **FR-005**: The host lifecycle is an explicit state machine with a refusal and an ontology mirror. Rows: `MEDULLA-MACHINE-R005`, `MEDULLA-MACHINE-R005.1`, `MEDULLA-MACHINE-R005.2`, `MEDULLA-MACHINE-R005.3`, `MEDULLA-MACHINE-R005.4`, `MEDULLA-MACHINE-R005.5`.
- **FR-006**: A public hosts-file schema normalizes to deterministic records that enter the tenant graph through source ingestion. Rows: `MEDULLA-MACHINE-R006`, `MEDULLA-MACHINE-R006.1`, `MEDULLA-MACHINE-R006.2`, `MEDULLA-MACHINE-R006.3`, `MEDULLA-MACHINE-R006.4`.
- **FR-007**: The pure functions reach callers through the Python wheel. Rows: `MEDULLA-MACHINE-R007`.
- **FR-008**: The vocabulary answers one competency question across three connectors, and a served proof covers the ingest path. Rows: `MEDULLA-MACHINE-R008`, `MEDULLA-MACHINE-R009`.

## Success Criteria

- **SC-001**: Three connector records resolve to one machine in one query. Rows: `MEDULLA-MACHINE-R009`.
- **SC-002**: Each identity test vector returns its recorded IRI. Rows: `MEDULLA-MACHINE-R002.2`.
- **SC-003**: Every illegal pair of lifecycle state and event returns `TRANSITION_REFUSED`. Rows: `MEDULLA-MACHINE-R005.3`.
- **SC-004**: The ontology edge set equals the Rust edge table. Rows: `MEDULLA-MACHINE-R005.4`.
- **SC-005**: Normalizing one hosts file twice returns identical bytes. Rows: `MEDULLA-MACHINE-R006.3`.

## Assumptions

- Facts about a machine come from connectors through arbora. The engine never probes a host.
- The hosts file holds credential references only. A real hosts file lives in a private repository.
- A tenant owns its machine graph. Sharing across tenants follows the existing visibility rules.

## Why a new spec

The search covered these medulla specs: `MEDULLA-SUPPLY-VOCAB`, `MEDULLA-DATAPLAT-VOCAB`, `MEDULLA-PLACEMENT`, `MEDULLA-ORCH-VOCAB`, `MEDULLA-INGEST-WORKER`, `MEDULLA-TYPED-PACKS` and `MEDULLA-CQ-XSRC`. The command was `git grep -n "Machine\|HardwareNode\|InfrastructureHost\|lifecycle" -- specs`. `MEDULLA-SUPPLY-VOCAB` uses the host class as an anchor for limits and owns no machine identity, parts or lifecycle. `MEDULLA-PLACEMENT` owns capacity leases and placement targets, not the machine itself. No spec owns a machine, its identity, its parts, its lifecycle or the hosts file. This spec owns them. `MEDULLA-SUPPLY-VOCAB` text moves to the new class in place.

## Scope and interfaces

Owned behaviour: the machine class, identity derivation, hardware parts and the observer link. It also owns the lifecycle machine, the hosts-file schema, the record set and the Python surface of the pure functions.

Public contracts: the consumers hold them. Arbora and each connector name their seam in their own `contracts/` directory. The wire shapes and vectors live in [data-model.md](data-model.md).

Non-goals: probing hosts (connectors), serving the build and the description (arbora), and the real hosts file (a private repository).

## Traceability

| Label | Rows | Design section | Test ID | Evidence |
|---|---|---|---|---|
| FR-001 | `MEDULLA-MACHINE-R001` | plan.md Phase 1 | T-001 |  |
| FR-002 | `MEDULLA-MACHINE-R002` | plan.md Phase 1 | T-002 |  |
| FR-003 | `MEDULLA-MACHINE-R003` | plan.md Phase 1 | T-003 |  |
| FR-004 | `MEDULLA-MACHINE-R004` | plan.md Phase 1 | T-004 |  |
| FR-005 | `MEDULLA-MACHINE-R005` | plan.md Phase 1 | T-005 |  |
| FR-006 | `MEDULLA-MACHINE-R006` | plan.md Phase 1 | T-006 |  |
| FR-007 | `MEDULLA-MACHINE-R007` | plan.md Phase 1 | T-007 |  |
| FR-008 | `MEDULLA-MACHINE-R008` | plan.md Phase 1 | T-008 |  |
