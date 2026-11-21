//! Inventory of the raft and xshard lifecycle machines
//! (MEDULLA-PLACEMENT-R783.3).
//!
//! Every lifecycle state enum in `medulla-raft` and `medulla-raft-xshard` has a
//! row here naming its machine, its closed state set and its terminal states.
//! The test reads the enum declarations from the crate sources, so a new or
//! renamed variant, or a new lifecycle enum with no row, fails it.

use medulla_raft::raft::membership_shrink::MEMBERSHIP_SHRINK_MACHINE;

use crate::drain::SHARD_DRAIN_MACHINE;
use medulla_types::transition_refused::MachineState;

/// One lifecycle state enum and its machine.
struct MachineRow {
    /// The enum name as declared.
    enum_name: &'static str,
    /// The machine instance name.
    machine: &'static str,
    /// The source file that declares the enum, from the workspace crates dir.
    source: &'static str,
    /// The states in declaration order.
    states: &'static [&'static str],
    /// The terminal states.
    terminals: &'static [&'static str],
}

const INVENTORY: &[MachineRow] = &[
    MachineRow {
        enum_name: "MembershipShrinkPhase",
        machine: "MembershipShrinkMachine",
        source: "medulla-raft/src/raft/membership_shrink.rs",
        states: &[
            "Proposed",
            "DrainRequested",
            "Drained",
            "LearnerCaughtUp",
            "LeadershipTransferred",
            "SafetyChecked",
            "RemovalCommitted",
            "Completed",
            "Aborted",
        ],
        terminals: &["Completed", "Aborted"],
    },
    MachineRow {
        enum_name: "MoveStage",
        machine: "PartitionMoveMachine",
        source: "medulla-raft/src/raft/placement/move_journal.rs",
        states: &[
            "Planned",
            "Moving",
            "Transferring",
            "ReadyForCutover",
            "CutoverCommitted",
            "Aborting",
            "Completed",
            "Aborted",
        ],
        terminals: &["Completed", "Aborted"],
    },
    MachineRow {
        enum_name: "PartitionState",
        machine: "PartitionMoveMachine",
        source: "medulla-raft/src/raft/placement.rs",
        states: &["Active", "Moving"],
        terminals: &[],
    },
    MachineRow {
        enum_name: "DrainPhase",
        machine: "ShardDrainMachine",
        source: "medulla-raft-xshard/src/drain.rs",
        states: &[
            "Proposed",
            "AdmissionStopRequested",
            "AdmissionStopped",
            "DrainObserved",
            "ShrinkRequested",
            "ShrinkCommitted",
            "Completed",
            "RollbackRequired",
            "RolledBack",
            "RecoveryRequired",
        ],
        terminals: &["Completed", "RolledBack"],
    },
    MachineRow {
        enum_name: "PlacementState",
        machine: "ShardPlacementMachine",
        source: "medulla-raft-xshard/src/cross_node_elasticity/mod.rs",
        states: &[
            "Resident",
            "Follower",
            "Cold",
            "Hydrating",
            "Snapshotting",
            "DeltaCatchUp",
            "FencedCutover",
            "Draining",
            "Quarantined",
        ],
        terminals: &[],
    },
    MachineRow {
        enum_name: "MovePhase",
        machine: "MovePhaseMachine",
        source: "medulla-raft-xshard/src/cross_node_elasticity/mod.rs",
        states: &[
            "Snapshot",
            "DeltaCatchUp",
            "FencedCutover",
            "Hydrate",
            "Complete",
            "Aborted",
        ],
        terminals: &["Complete", "Aborted"],
    },
];

/// Enums whose name ends in a lifecycle word but that carry no lifecycle.
const NOT_A_LIFECYCLE: &[(&str, &str)] = &[
    ("LockMode", "a lock kind (shared or exclusive), not a phase"),
];

fn crates_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates dir")
        .to_path_buf()
}

/// The variant names of `pub enum <name>` in `source`, in order. Tuple and
/// struct variants keep their name only.
fn declared_variants(source: &str, name: &str) -> Vec<String> {
    let header = format!("pub enum {name} ");
    let start = source
        .find(&header)
        .unwrap_or_else(|| panic!("enum {name} is not declared in its inventory source"));
    let mut depth = 0i32;
    let mut opened = false;
    let mut variants = Vec::new();
    for line in source[start..].lines() {
        let trimmed = line.trim();
        if opened && depth == 1 && !trimmed.starts_with("//") && !trimmed.starts_with('#') {
            let ident: String = trimmed
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !ident.is_empty() {
                variants.push(ident);
            }
        }
        for ch in line.chars() {
            match ch {
                '{' | '(' => {
                    depth += 1;
                    opened = true;
                }
                '}' | ')' => depth -= 1,
                _ => {}
            }
        }
        if opened && depth == 0 {
            break;
        }
    }
    variants
}

/// Names of every `pub enum <X>(State|Phase|Stage|Status|Mode)` in a source tree.
fn lifecycle_named_enums(dir: &std::path::Path, out: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).expect("read source dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            lifecycle_named_enums(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let text = std::fs::read_to_string(&path).expect("read source");
            for line in text.lines() {
                let Some(rest) = line.trim_start().strip_prefix("pub enum ") else {
                    continue;
                };
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if ["State", "Phase", "Stage", "Status", "Mode"]
                    .iter()
                    .any(|suffix| name.ends_with(suffix))
                {
                    out.push(name);
                }
            }
        }
    }
}

// spec: MEDULLA-PLACEMENT-R783.3
#[test]
fn every_raft_state_enum_has_a_machine_row() {
    let crates = crates_dir();

    // Every inventory row matches the declared enum exactly.
    for row in INVENTORY {
        let source = std::fs::read_to_string(crates.join(row.source)).expect("read source");
        let declared = declared_variants(&source, row.enum_name);
        assert_eq!(
            declared, row.states,
            "{} declares states that differ from its inventory row",
            row.enum_name
        );
        for terminal in row.terminals {
            assert!(
                row.states.contains(terminal),
                "{}: terminal {terminal} is not a state",
                row.enum_name
            );
        }
        assert!(row.machine.ends_with("Machine"), "{}", row.machine);
    }

    // Every lifecycle-named enum in the two crates has a row or a stated exemption.
    let mut found = Vec::new();
    lifecycle_named_enums(&crates.join("medulla-raft/src"), &mut found);
    lifecycle_named_enums(&crates.join("medulla-raft-xshard/src"), &mut found);
    for name in found {
        let covered = INVENTORY.iter().any(|row| row.enum_name == name)
            || NOT_A_LIFECYCLE.iter().any(|(exempt, _)| *exempt == name);
        assert!(covered, "enum {name} has no machine row in the inventory");
    }

    // Where the machine definition exists, its state set equals the row.
    let shrink = INVENTORY
        .iter()
        .find(|r| r.enum_name == "MembershipShrinkPhase")
        .unwrap();
    assert_eq!(MEMBERSHIP_SHRINK_MACHINE.name, shrink.machine);
    let stored: Vec<_> = MEMBERSHIP_SHRINK_MACHINE
        .states
        .iter()
        .map(|s| s.stored().replace('_', "").to_ascii_lowercase())
        .collect();
    let want: Vec<_> = shrink
        .states
        .iter()
        .map(|s| s.to_ascii_lowercase())
        .collect();
    assert_eq!(stored, want);

    let drain = INVENTORY
        .iter()
        .find(|r| r.enum_name == "DrainPhase")
        .unwrap();
    assert_eq!(SHARD_DRAIN_MACHINE.name, drain.machine);
    let stored: Vec<_> = SHARD_DRAIN_MACHINE
        .states
        .iter()
        .map(|s| s.stored().replace('_', ""))
        .collect();
    let want: Vec<_> = drain
        .states
        .iter()
        .map(|s| s.to_ascii_lowercase())
        .collect();
    assert_eq!(stored, want);
    assert_eq!(SHARD_DRAIN_MACHINE.terminals.len(), drain.terminals.len());
}
