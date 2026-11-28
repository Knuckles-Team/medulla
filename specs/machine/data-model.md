# MEDULLA-MACHINE — Data model

Governing spec: [spec.md](spec.md).

All terms live in the core namespace `https://knuckles-team.github.io/ontology/core#`, module `machine`. The prefix `:` below means that namespace. Files: `crates/medulla-core/ontology/machine-v1.ttl` and `crates/medulla-core/ontology/machine-v1.shapes.ttl`. The lifecycle machine lives in `crates/medulla-core/ontology/state-machines-v1.ttl`.

## Classes

| Class | Parent | Meaning |
|---|---|---|
| `:Machine` | `bfo:0000004` | A physical or virtual computing machine with one stable identity |
| `:MachineComponent` | `bfo:0000030` | A part of a machine. Abstract |
| `:CentralProcessor` | `:MachineComponent` | One processor package |
| `:MemoryModule` | `:MachineComponent` | One memory module |
| `:StorageDevice` | `:MachineComponent` | One disk, solid-state drive or NVMe device |
| `:NetworkInterface` | `:MachineComponent` | One network interface |
| `:NetworkBond` | `:MachineComponent` | One aggregation of interfaces |
| `:Accelerator` | `:MachineComponent` | One GPU, FPGA, TPU, DPU or similar device |
| `:ManagementController` | `:MachineComponent` | One out-of-band management controller |
| `:ProbeSnapshot` | `:Observation` | One recorded probe result with its content digest |
| `:MachineLifecycleTransitionRecord` | `:Observation` | One applied lifecycle transition |

## Object properties

| Property | Domain | Range | Meaning |
|---|---|---|---|
| `:hasComponent` | `:Machine` | `:MachineComponent` | The machine owns the part |
| `:bondMember` | `:NetworkBond` | `:NetworkInterface` | The interface belongs to the bond |
| `:observes` | none | `:Machine` | Any connector record or probe snapshot points at the machine it describes |
| `:transitionOf` | `:MachineLifecycleTransitionRecord` | `:Machine` | The record belongs to the machine |

`:observes` has no domain on purpose. Every connector host class, orchestrator member class and probe record may use it.

## Datatype properties

| Property | Datatype | Used on |
|---|---|---|
| `:machineName` | `xsd:string` | Machine. The hosts-file name |
| `:identityTier` | `xsd:string` in `uuid`, `serial`, `bmc`, `host` | Machine |
| `:machineKey` | `xsd:string`, form `<kind>:<normalized value>` | Machine. One per known key |
| `:systemUuid` | `xsd:string` | Machine |
| `:serialNumber` | `xsd:string` | Machine, MemoryModule, StorageDevice, Accelerator |
| `:manufacturer` | `xsd:string` | Machine |
| `:model` | `xsd:string` | Machine, CentralProcessor, MemoryModule, StorageDevice, Accelerator, ManagementController |
| `:architecture` | `xsd:string` | Machine |
| `:operatingSystem` | `xsd:string` | Machine |
| `:kernelVersion` | `xsd:string` | Machine |
| `:processorCount` | `xsd:nonNegativeInteger` | Machine |
| `:coreCount` | `xsd:nonNegativeInteger` | Machine, CentralProcessor |
| `:threadCount` | `xsd:nonNegativeInteger` | Machine, CentralProcessor |
| `:memoryTotalBytes` | `xsd:nonNegativeInteger` | Machine |
| `:machineAddress` | `xsd:string` | Machine. One per address |
| `:managementAddress` | `xsd:string` | Machine, ManagementController |
| `:machineRole` | `xsd:string` | Machine. One per role |
| `:machineGroup` | `xsd:string` | Machine. One per group |
| `:sshCredentialRef` | `xsd:string` | Machine. A reference, never a secret |
| `:managementCredentialRef` | `xsd:string` | Machine. A reference, never a secret |
| `:retireRequested` | `xsd:boolean` | Machine |
| `:machineLifecycleState` | `xsd:string` in `commissioned`, `active`, `draining`, `maintenance`, `retired` | Machine |
| `:socketIndex` | `xsd:nonNegativeInteger` | CentralProcessor |
| `:componentSlot` | `xsd:string` | MemoryModule |
| `:capacityBytes` | `xsd:nonNegativeInteger` | MemoryModule, StorageDevice, Accelerator |
| `:deviceName` | `xsd:string` | StorageDevice |
| `:storageMedium` | `xsd:string` in `hdd`, `ssd`, `nvme`, `other` | StorageDevice |
| `:interfaceName` | `xsd:string` | NetworkInterface |
| `:macAddress` | `xsd:string`, lower-case colon form | NetworkInterface |
| `:speedMegabits` | `xsd:nonNegativeInteger` | NetworkInterface |
| `:linkState` | `xsd:string` in `up`, `down`, `unknown` | NetworkInterface |
| `:bondName` | `xsd:string` | NetworkBond |
| `:bondMode` | `xsd:string` | NetworkBond |
| `:acceleratorKind` | `xsd:string` in `gpu`, `fpga`, `tpu`, `dpu`, `other` | Accelerator |
| `:pciAddress` | `xsd:string` | Accelerator |
| `:controllerId` | `xsd:string`, lower-case colon MAC | ManagementController |
| `:firmwareVersion` | `xsd:string` | ManagementController |
| `:probeKind` | `xsd:string` in `system`, `management`, `orchestrator` | ProbeSnapshot |
| `:connector` | `xsd:string` | ProbeSnapshot |
| `:tool` | `xsd:string` | ProbeSnapshot |
| `:probeDigest` | `xsd:string`, form `sha256:<64 hex>` | ProbeSnapshot |
| `:observedAt` | `xsd:dateTime` | ProbeSnapshot, MachineLifecycleTransitionRecord |
| `:previousState` | `xsd:string` | MachineLifecycleTransitionRecord |
| `:nextState` | `xsd:string` | MachineLifecycleTransitionRecord |
| `:appliedEdgeId` | `xsd:string`, `H01` to `H07` | MachineLifecycleTransitionRecord |
| `:firedByEvent` | `xsd:string` | MachineLifecycleTransitionRecord |
| `:evidenceDigest` | `xsd:string`, form `sha256:<64 hex>` | MachineLifecycleTransitionRecord |

Only `:observedAt` holds a timestamp. A machine node carries none.

## Retired terms

| Retired term | Replacement |
|---|---|
| `infrastructure#Host` | `:Machine` |
| `:InfrastructureHost` | `:Machine` |
| `kg:HardwareNode` | `:Machine` |
| `infrastructure#NetworkInterface` | `:NetworkInterface` |
| `infrastructure#GPUAccelerator` | `:Accelerator` |
| `infrastructure#hasInterface` | `:hasComponent` |

`infrastructure#Node` stays as the orchestration member. `infrastructure#isHost` keeps its name and now ranges over `:Machine`. `infrastructure#Device` stays as a device that groups entity states.

## Inventory record set

The normalized records are entities and relationships in the shape of the connector ingest contract. The binding is connector `inventory-records`, stream `machines`.

| Entity type | Id | Properties |
|---|---|---|
| `Machine` | machine IRI | The Machine properties above |
| `CentralProcessor` | `urn:machine-part:` + first 32 hex of sha256(`<machine IRI> LF processor LF <natural key>`) | `socketIndex`, `model`, `coreCount`, `threadCount` |
| `MemoryModule` | same form, kind `memory` | `componentSlot`, `capacityBytes`, `serialNumber`, `model` |
| `StorageDevice` | same form, kind `storage` | `deviceName`, `model`, `serialNumber`, `capacityBytes`, `storageMedium` |
| `NetworkInterface` | same form, kind `nic` | `interfaceName`, `macAddress`, `speedMegabits`, `linkState` |
| `NetworkBond` | same form, kind `bond` | `bondName`, `bondMode` |
| `Accelerator` | same form, kind `accelerator` | `acceleratorKind`, `model`, `capacityBytes`, `serialNumber`, `pciAddress` |
| `ManagementController` | same form, kind `management` | `controllerId`, `managementAddress`, `firmwareVersion`, `model` |
| `ProbeSnapshot` | `urn:machine-probe:` + first 32 hex of sha256(`<machine IRI> LF <probeKind> LF <probeDigest>`) | `probeKind`, `connector`, `tool`, `probeDigest`, `observedAt` |
| `MachineLifecycleTransitionRecord` | `urn:machine-transition:` + first 32 hex of sha256(`<machine IRI> LF <edge id> LF <evidenceDigest>`) | `previousState`, `nextState`, `appliedEdgeId`, `firedByEvent`, `evidenceDigest`, `observedAt` |

| Relationship | From | To |
|---|---|---|
| `hasComponent` | Machine | any component |
| `bondMember` | NetworkBond | NetworkInterface |
| `observes` | ProbeSnapshot | Machine |
| `transitionOf` | MachineLifecycleTransitionRecord | Machine |

Natural key per component kind, first non-empty value in this order:

| Kind | Natural key order |
|---|---|
| `processor` | `socketIndex`, then `ordinal:<n>` |
| `memory` | `componentSlot`, `serialNumber`, then `ordinal:<n>` |
| `storage` | `serialNumber`, `deviceName`, then `ordinal:<n>` |
| `nic` | `macAddress`, `interfaceName`, then `ordinal:<n>` |
| `bond` | `bondName` |
| `accelerator` | `pciAddress`, `serialNumber`, then `ordinal:<n>` |
| `management` | `controllerId` |

`ordinal:<n>` is the zero-based position after the part records sort by their canonical JSON text.

## Canonical form

- Canonical JSON sorts object keys by code point. It has no extra whitespace and writes UTF-8. It holds integer numbers only.
- A list property holds its values sorted ascending by code point, with duplicates removed.
- Entities sort by `(node_type, id)`. Relationships sort by `(source, relationship, target)`.
- A digest is `sha256:` followed by 64 lower-case hex digits of the canonical bytes.

## Hosts file (schema `hosts/v1`)

| Field | Type | Rule |
|---|---|---|
| `schema` | string | Required. Exactly `hosts/v1` |
| `hosts` | list | Required. At least one entry |
| `hosts[].name` | string | Required. Matches `^[a-z0-9][a-z0-9-]{0,62}$`. Unique in the file |
| `hosts[].addresses` | list of string | Required. At least one DNS name or IP literal |
| `hosts[].management_address` | string | Optional. A DNS name or IP literal |
| `hosts[].roles` | list of string | Optional. Each matches `^[a-z][a-z0-9-]{0,31}$` |
| `hosts[].groups` | list of string | Optional. Same pattern as roles |
| `hosts[].credentials.ssh` | string | Optional. A reference `<scheme>://<path>` |
| `hosts[].credentials.management` | string | Optional. A reference `<scheme>://<path>` |
| `hosts[].identity.system_uuid` | string | Optional pin |
| `hosts[].identity.system_serial` | string | Optional pin |
| `hosts[].identity.management_mac` | string | Optional pin |
| `hosts[].retired` | boolean | Optional. Default `false` |

An unknown key refuses. A key named `password`, `passphrase`, `secret`, `token`, `private_key` or `api_key` at any depth refuses with reason `secret_field`. A reference matches `^[a-z][a-z0-9+.-]*://[^\s]+$`.

## Python function wire shapes

Module `medulla.engine` holds these functions. Each takes and returns text. Structured values are canonical JSON.

| Function | Input | Output |
|---|---|---|
| `normalize_hosts_file(yaml)` | the hosts file text | `{"schema":"hosts-normalized/v1","hosts":[{"name","addresses","management_address","roles","groups","credentials":{"ssh","management"},"identity":{"system_uuid","system_serial","management_mac"},"retired"}]}`. Absent scalars are `null`. |
| `machine_normalize_key(kind, raw)` | kind in `uuid`, `serial`, `bmc`, `host`; raw text | `{"kind","value"}` |
| `machine_iri(tenant, keys_json)` | `keys_json` is `[{"kind","value"}]` in any order | `{"iri","tier","key"}` for the best valid key |
| `machine_lifecycle_derive(state, facts_json)` | stored state text; facts `{"reachable","ready","health_failing","drain_requested","workloads_evacuated","retire_requested"}` | `{"event":"Ready"\|"Drain"\|"Drained"\|"Resume"\|"Retire"\|null}` |
| `machine_lifecycle_apply(state, event, facts_json)` | stored state text, event name, facts | `{"state","edge_id"}` |
| `canonical_digest(text)` | canonical JSON text | `sha256:<64 hex>` |

## Function refusals

- `HOSTS_FILE_INVALID`: raised as `ValueError` whose text is the JSON record `{"code","reason","pointer"}`. Reasons: `schema_version`, `unknown_field`, `duplicate_name`, `bad_name`, `no_address`, `bad_address`, `bad_reference`, `bad_role`, `secret_field`, `empty_hosts`, `malformed_yaml`.
- `MachineKeyRefused`: raised as `ValueError("<kind>:<reason>")`. Reasons: `malformed`, `placeholder`, `empty`.
- `no_valid_key`: `machine_iri` found no valid key.
- `TRANSITION_REFUSED`: `machine_lifecycle_apply` raises `ValueError` whose text is `TRANSITION_REFUSED: MachineLifecycleMachine refused '<event>' from '<state>' (<reason>)`. Reasons: `illegal_transition`, `guard_failed`, `terminal_state`, `unknown_event`.

## Identity test vectors

All keys here are documentation values. The tenant of the host vectors is `tenant-a` unless stated.

| Input | Normalized value | IRI |
|---|---|---|
| uuid `123E4567-E89B-12D3-A456-426614174000` | `123e4567-e89b-12d3-a456-426614174000` | `urn:machine:uuid:d1c2987bf12e8996acbc7828e5d417f9` |
| serial `  abc   123 ` | `ABC 123` | `urn:machine:serial:f3af1defd63343f5dd2ee44b56cb636d` |
| bmc `AA-BB-CC-DD-EE-01` | `aa:bb:cc:dd:ee:01` | `urn:machine:bmc:7ed5f3e345f5057249d850266ed1dc84` |
| host `Node-1.Example.` | `node-1.example` | `urn:machine:host:bee191710c8bd12296a5e402bdf9f43e` |
| host `Node-1.Example.` with tenant `tenant-b` | `node-1.example` | `urn:machine:host:b84ef4959aa8c354baf8b92bf786dbc3` |
| serial `SN-0001` | `SN-0001` | `urn:machine:serial:106926c48469ff7da8c08bc3b320aeb8` |
| host `alpha` | `alpha` | `urn:machine:host:c9faaf568305e0a90e726ca4b4807c73` |

Preference vectors: keys `[uuid 00000000-0000-0000-0000-000000000000, serial SN-0001, host alpha]` select the serial key. Keys `[host alpha, bmc AA-BB-CC-DD-EE-01]` select the bmc key. An empty list selects none.

The vectors live in `crates/medulla-source-types/tests/vectors/machine_iri.json`, and the Python test reads the same file.
