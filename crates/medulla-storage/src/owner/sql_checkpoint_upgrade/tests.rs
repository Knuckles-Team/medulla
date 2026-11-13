//! Focused compatibility checks for the two SQL layouts actually declared by
//! the Train 4 owner lineage.

use super::*;
use crate::owner::persisted_layout::create_predecessor_owner_file;
use crate::recovery::evidence::strict_recovery_evidence;
use medulla_direct_state::direct_state::private_local_tempdir;
use redb::{ReadableDatabase, TableDefinition};
use sha2::{Digest, Sha256};

/// Serializes the tests here that open redb files. `upgrade_is_atomic_across_each_crash_stage`
/// spawns child processes, and a child holds duplicates of every parent file
/// descriptor until it execs. redb's `flock` lives on the shared open file, so a
/// database another test thread has just dropped can still read as locked
/// ("Database already open") while a child is starting.
static REDB_FILES: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn redb_files() -> std::sync::MutexGuard<'static, ()> {
    REDB_FILES
        .lock()
        .expect("a sibling redb-file test panicked while holding the lock")
}

const USER_ROWS: TableDefinition<(&str, u64), &[u8]> = TableDefinition::new("__sql_rows__");
const STRAY: TableDefinition<&str, &[u8]> = TableDefinition::new("stray_sql_table");

fn identity() -> PhysicalStoreIdentity {
    PhysicalStoreIdentity::new("eg-query:sql-user-tables").unwrap()
}

fn options(path: &Path) -> SqlSourceCheckpointInspectionOptions {
    SqlSourceCheckpointInspectionOptions::new(
        path.parent().unwrap().join("sql-layout-inspection"),
        256 * 1024 * 1024,
    )
    .unwrap()
}

fn predecessor(path: &Path, old: &crate::owner::persisted_layout::LayoutPredecessor) {
    create_predecessor_owner_file(path, identity(), old).unwrap();
    let db = Database::open(path).unwrap();
    let write = db.begin_write().unwrap();
    write
        .open_table(USER_ROWS)
        .unwrap()
        .insert(("tenant-a:source-table", 7), b"preserved row".as_slice())
        .unwrap();
    write.commit().unwrap();
}

#[test]
fn exact_predecessor_contracts_match_the_frozen_historical_digests() {
    for (old, pinned) in [
        (
            SQL_BEFORE_SOURCE_CHECKPOINTS,
            contract::PRE_SOURCE_CHECKPOINTS,
        ),
        (SQL_BEFORE_DURABLE_ANN, contract::PRE_DURABLE_ANN),
        (
            SQL_BEFORE_TRIGGER_REACTIONS,
            contract::PRE_TRIGGER_REACTIONS,
        ),
    ] {
        let contracts: Vec<_> = crate::owner::contract::expected_table_contracts(OwnerLayout::Sql)
            .into_iter()
            .filter(|contract| {
                contract.ownership == crate::physical::manifest::TableOwnership::Ledger
                    || old.owner_tables.contains(&contract.table_id.as_str())
            })
            .collect();
        assert_eq!(
            crate::owner::layout::layout_digest_over(OwnerLayout::Sql, &contracts),
            pinned,
            "{} no longer matches its frozen layout contract",
            old.label
        );
    }
}

fn row(path: &Path) -> Vec<u8> {
    let db = Database::open(path).unwrap();
    let read = db.begin_read().unwrap();
    let table = read.open_table(USER_ROWS).unwrap();
    table
        .get(("tenant-a:source-table", 7))
        .unwrap()
        .unwrap()
        .value()
        .to_vec()
}

/// Only a subprocess receives the crash stage. Exiting without unwinding
/// exercises redb's committed versus uncommitted upgrade boundary.
pub(super) fn crash_at(stage: &str) {
    if std::env::var("MEDULLA_SQL_LAYOUT_UPGRADE_CRASH")
        .ok()
        .as_deref()
        == Some(stage)
    {
        std::process::exit(73);
    }
}

#[test]
fn layout_upgrade_crash_child() {
    let Some(path) = std::env::var_os("MEDULLA_SQL_LAYOUT_UPGRADE_CHILD_PATH") else {
        return;
    };
    let path = PathBuf::from(path);
    let token =
        inspect_sql_source_checkpoint_upgrade(&path, identity(), None, options(&path)).unwrap();
    upgrade_sql_source_checkpoints(token).unwrap();
    panic!("the child must terminate at the selected durable stage");
}

#[test]
fn upgrade_is_atomic_across_each_crash_stage() {
    let _serial = redb_files();
    for stage in [
        "before_table",
        "after_table",
        "after_manifest",
        "after_commit",
    ] {
        let directory = private_local_tempdir();
        let path = directory.path().join("sql.redb");
        predecessor(&path, &SQL_BEFORE_DURABLE_ANN);
        let before = row(&path);
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "owner::sql_checkpoint_upgrade::tests::layout_upgrade_crash_child",
                "--nocapture",
            ])
            .env("MEDULLA_SQL_LAYOUT_UPGRADE_CHILD_PATH", &path)
            .env("MEDULLA_SQL_LAYOUT_UPGRADE_CRASH", stage)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(73), "{stage}");
        if stage == "after_commit" {
            StorageKernel::open_owner::<SqlOwner>(&path, identity(), None).unwrap();
            assert!(
                inspect_sql_source_checkpoint_upgrade(&path, identity(), None, options(&path))
                    .is_err()
            );
        } else {
            assert!(StorageKernel::open_owner::<SqlOwner>(&path, identity(), None).is_err());
            let token =
                inspect_sql_source_checkpoint_upgrade(&path, identity(), None, options(&path))
                    .unwrap();
            upgrade_sql_source_checkpoints(token).unwrap();
        }
        assert_eq!(row(&path), before, "{stage}");
    }
}

#[test]
fn both_declared_sql_predecessors_upgrade_once_and_preserve_rows() {
    let _serial = redb_files();
    for old in [
        SQL_BEFORE_SOURCE_CHECKPOINTS,
        SQL_BEFORE_DURABLE_ANN,
        SQL_BEFORE_TRIGGER_REACTIONS,
    ] {
        let directory = private_local_tempdir();
        let path = directory.path().join("sql.redb");
        predecessor(&path, &old);
        assert!(StorageKernel::open_owner::<SqlOwner>(&path, identity(), None).is_err());
        let before = row(&path);
        let token =
            inspect_sql_source_checkpoint_upgrade(&path, identity(), None, options(&path)).unwrap();
        let (kernel, report) = upgrade_sql_source_checkpoints(token).unwrap();
        assert_ne!(report.previous_layout_digest, report.current_layout_digest);
        assert_eq!(
            report.previous_authority_epoch + 1,
            report.current_authority_epoch
        );
        let evidence = strict_recovery_evidence(&kernel).unwrap();
        for table_name in [
            SQL_SOURCE_CHECKPOINTS.name(),
            SQL_ANN_DIRTY.name(),
            SQL_ANN_GENERATIONS.name(),
            SQL_EDGE_INDEXES.name(),
        ] {
            assert!(evidence
                .tables
                .iter()
                .any(|table| table.table_id == table_name));
        }
        drop(kernel);
        assert_eq!(row(&path), before);
        assert!(
            inspect_sql_source_checkpoint_upgrade(&path, identity(), None, options(&path)).is_err()
        );
    }
}

/// An SQL-layout file whose owner-manifest generation is NOT one of the
/// predecessors the shared owner-manifest lineage declares (a stale/foreign
/// binding) is refused both by the one shared classification that governs
/// every owner's open-refusal and upgrade-routing decision, AND by the SQL
/// offline upgrade's own inspection -- proving the SQL store has no separate
/// admission path beyond what that one shared lineage check declares.
// spec: MEDULLA-DURABLE-KERNEL-R045
#[test]
fn an_undeclared_sql_predecessor_is_refused_by_the_shared_lineage_check() {
    let _serial = redb_files();
    const SQL_UNDECLARED_GENERATION: crate::owner::persisted_layout::LayoutPredecessor =
        crate::owner::persisted_layout::LayoutPredecessor {
            layout: OwnerLayout::Sql,
            label: "SQL generation that no lineage declares",
            owner_tables: &["__nonexistent_sql_owner_table__"],
            data_lost: "nothing is known about it",
            file_name: "sql.redb",
        };
    let directory = private_local_tempdir();
    let path = directory.path().join("sql.redb");
    create_predecessor_owner_file(&path, identity(), &SQL_UNDECLARED_GENERATION).unwrap();

    assert!(
        matches!(
            crate::owner::offline_upgrade::classify_owner_store_format(&path),
            Ok(crate::owner::offline_upgrade::OwnerStoreFormat::Unknown(
                OwnerLayout::Sql
            ))
        ),
        "the shared classification must not treat an undeclared generation as upgradable"
    );
    assert!(
        inspect_sql_source_checkpoint_upgrade(&path, identity(), None, options(&path)).is_err(),
        "the SQL offline upgrade must not admit a predecessor the shared lineage never declared"
    );
}

#[test]
fn mismatched_census_is_rejected_without_changing_the_file() {
    let _serial = redb_files();
    let directory = private_local_tempdir();
    let path = directory.path().join("sql.redb");
    predecessor(&path, &SQL_BEFORE_DURABLE_ANN);
    let db = Database::open(&path).unwrap();
    let write = db.begin_write().unwrap();
    write.open_table(STRAY).unwrap();
    write.commit().unwrap();
    drop(db);
    let before: [u8; 32] = Sha256::digest(std::fs::read(&path).unwrap()).into();
    assert!(inspect_sql_ann_generation_upgrade(&path, identity(), None, options(&path)).is_err());
    let after: [u8; 32] = Sha256::digest(std::fs::read(&path).unwrap()).into();
    assert_eq!(before, after);
}

/// The old per-table rowid counter and a named sequence a predecessor kept
/// beside it, written the way the predecessor's engine wrote them.
const OLD_COUNTER: u64 = 7;
const OLD_NAMED_SEQUENCE_HIGH_MARK: u64 = 11;

#[derive(serde::Serialize)]
struct OldColumn {
    name: &'static str,
    serial: bool,
}

#[derive(serde::Serialize)]
struct OldTable {
    name: &'static str,
    columns: Vec<OldColumn>,
}

/// A predecessor file that has served tenant `tenant`: a bound SQL bootstrap
/// scope, one table `orders` with a serial column `id` whose rowid counter is
/// [`OLD_COUNTER`], one table without a serial column, and one named sequence.
fn serial_predecessor(
    path: &Path,
    old: &crate::owner::persisted_layout::LayoutPredecessor,
    tenant: &str,
) {
    use crate::owner::registry::{SQL_CATALOG, SQL_SEQ};
    predecessor(path, old);
    let (incarnation, _) = StoreIncarnation::derive(path).unwrap();
    let scope = medulla_mutation_types::mutation_scope::MutationScopeIdentity::fixed_native(
        tenant,
        medulla_mutation_types::mutation_scope::DurabilityDomain::SqlCatalog,
        crate::owner::registry::SQL_BOOTSTRAP_RESOURCE,
        "sql-upgrade-test",
    )
    .unwrap();
    let db = Database::open(path).unwrap();
    let write = db.begin_write().unwrap();
    crate::physical::binding::bind_scope_in(&store_handle(incarnation), &write, &scope, 0).unwrap();
    {
        let mut catalog = write.open_table(SQL_CATALOG).unwrap();
        for table in [
            OldTable {
                name: "orders",
                columns: vec![
                    OldColumn {
                        name: "id",
                        serial: true,
                    },
                    OldColumn {
                        name: "note",
                        serial: false,
                    },
                ],
            },
            OldTable {
                name: "plain",
                columns: vec![OldColumn {
                    name: "k",
                    serial: false,
                }],
            },
        ] {
            let bytes = rmp_serde::to_vec_named(&table).unwrap();
            catalog.insert(table.name, bytes.as_slice()).unwrap();
        }
        let mut counters = write.open_table(SQL_SEQ).unwrap();
        counters.insert("orders", OLD_COUNTER).unwrap();
        counters.insert("plain", 3).unwrap();
        counters
            .insert(
                crate::owner::registry::sequence_row_key(tenant, "invoice_no", "high_mark")
                    .as_str(),
                OLD_NAMED_SEQUENCE_HIGH_MARK,
            )
            .unwrap();
    }
    write.commit().unwrap();
}

fn sequence_value(path: &Path, tenant: &str, name: &str, field: &str) -> Option<u64> {
    use crate::owner::registry::{sequence_row_key, SQL_SEQUENCES};
    let db = Database::open(path).unwrap();
    let read = db.begin_read().unwrap();
    let table = read.open_table(SQL_SEQUENCES).unwrap();
    let value = table
        .get(sequence_row_key(tenant, name, field).as_str())
        .unwrap()
        .map(|value| value.value());
    value
}

// spec: MEDULLA-ENGINE-PARITY-R013.2
#[test]
fn named_sequence_upgrade_seeds_serial_sequences_above_the_old_high_mark() {
    let _serial = redb_files();
    for old in [
        SQL_BEFORE_SOURCE_CHECKPOINTS,
        SQL_BEFORE_DURABLE_ANN,
        SQL_BEFORE_NAMED_SEQUENCES,
    ] {
        let directory = private_local_tempdir();
        let path = directory.path().join("sql.redb");
        serial_predecessor(&path, &old, "tenant-a");
        // A normal open of the old layout refuses with the upgrade-required code.
        let refusal = StorageKernel::open_owner::<SqlOwner>(&path, identity(), None)
            .err()
            .expect("the old layout must refuse a normal open");
        assert!(
            refusal.contains("FORMAT_UPGRADE_REQUIRED"),
            "{}: {refusal}",
            old.label
        );
        let token =
            inspect_sql_named_sequence_upgrade(&path, identity(), None, options(&path)).unwrap();
        drop(upgrade_sql_named_sequences(token).unwrap());

        // The first value never issued is the old counter + 1: the old engine
        // issued rowid + 1 and the counter was the next rowid.
        assert_eq!(
            sequence_value(&path, "tenant-a", "orders_id_seq", "high_mark"),
            Some(OLD_COUNTER + 1)
        );
        assert_eq!(
            sequence_value(&path, "tenant-a", "orders_id_seq", "start"),
            Some(1)
        );
        assert_eq!(
            sequence_value(&path, "tenant-a", "orders_id_seq", "increment"),
            Some(1)
        );
        assert!(sequence_value(&path, "tenant-a", "orders_id_seq", "cache").is_some());
        // A table without a serial column gets no sequence.
        assert_eq!(
            sequence_value(&path, "tenant-a", "plain_k_seq", "high_mark"),
            None
        );
        // The named sequence moved out of the rowid-counter table, intact.
        assert_eq!(
            sequence_value(&path, "tenant-a", "invoice_no", "high_mark"),
            Some(OLD_NAMED_SEQUENCE_HIGH_MARK)
        );
        let db = Database::open(&path).unwrap();
        let read = db.begin_read().unwrap();
        let counters = read.open_table(crate::owner::registry::SQL_SEQ).unwrap();
        assert_eq!(
            counters.get("orders").unwrap().unwrap().value(),
            OLD_COUNTER
        );
        assert!(counters
            .iter()
            .unwrap()
            .all(|entry| !entry.unwrap().0.value().starts_with('\0')));
        drop(counters);
        drop(read);
        drop(db);
        // The upgraded file opens normally and is never upgraded twice.
        drop(StorageKernel::open_owner::<SqlOwner>(&path, identity(), None).unwrap());
        assert!(
            inspect_sql_named_sequence_upgrade(&path, identity(), None, options(&path)).is_err()
        );
    }
}

// spec: MEDULLA-ENGINE-PARITY-R013.2
#[test]
fn named_sequence_upgrade_seeds_every_served_tenant_scope() {
    let _serial = redb_files();
    let directory = private_local_tempdir();
    let path = directory.path().join("sql.redb");
    serial_predecessor(&path, &SQL_BEFORE_NAMED_SEQUENCES, "tenant-a");
    let scope = medulla_mutation_types::mutation_scope::MutationScopeIdentity::fixed_native(
        "tenant-b",
        medulla_mutation_types::mutation_scope::DurabilityDomain::SqlCatalog,
        crate::owner::registry::SQL_BOOTSTRAP_RESOURCE,
        "sql-upgrade-test",
    )
    .unwrap();
    let (incarnation, _) = StoreIncarnation::derive(&path).unwrap();
    let db = Database::open(&path).unwrap();
    let write = db.begin_write().unwrap();
    crate::physical::binding::bind_scope_in(&store_handle(incarnation), &write, &scope, 0).unwrap();
    write.commit().unwrap();
    drop(db);
    let token =
        inspect_sql_named_sequence_upgrade(&path, identity(), None, options(&path)).unwrap();
    drop(upgrade_sql_named_sequences(token).unwrap());
    for tenant in ["tenant-a", "tenant-b"] {
        assert_eq!(
            sequence_value(&path, tenant, "orders_id_seq", "high_mark"),
            Some(OLD_COUNTER + 1),
            "{tenant}"
        );
    }
}

// spec: MEDULLA-ENGINE-PARITY-R013.2
#[test]
fn named_sequence_upgrade_refuses_serial_columns_without_a_served_scope() {
    let _serial = redb_files();
    let directory = private_local_tempdir();
    let path = directory.path().join("sql.redb");
    predecessor(&path, &SQL_BEFORE_NAMED_SEQUENCES);
    let db = Database::open(&path).unwrap();
    let write = db.begin_write().unwrap();
    {
        let table = OldTable {
            name: "orders",
            columns: vec![OldColumn {
                name: "id",
                serial: true,
            }],
        };
        let bytes = rmp_serde::to_vec_named(&table).unwrap();
        write
            .open_table(crate::owner::registry::SQL_CATALOG)
            .unwrap()
            .insert("orders", bytes.as_slice())
            .unwrap();
    }
    write.commit().unwrap();
    drop(db);
    let token =
        inspect_sql_named_sequence_upgrade(&path, identity(), None, options(&path)).unwrap();
    let refusal = upgrade_sql_named_sequences(token).err().unwrap();
    assert!(refusal.contains("no tenant scope"), "{refusal}");
    // The refused upgrade left the file in its predecessor layout.
    assert!(inspect_sql_named_sequence_upgrade(&path, identity(), None, options(&path)).is_ok());
}

// spec: MEDULLA-ENGINE-PARITY-R012.2
#[test]
fn trigger_reaction_upgrade_creates_the_empty_ledger_and_keeps_rows() {
    use crate::owner::registry::SQL_TRIGGER_REACTIONS;
    let _serial = redb_files();
    let directory = private_local_tempdir();
    let path = directory.path().join("sql.redb");
    predecessor(&path, &SQL_BEFORE_TRIGGER_REACTIONS);
    let refusal = StorageKernel::open_owner::<SqlOwner>(&path, identity(), None)
        .err()
        .expect("the old layout must refuse a normal open");
    assert!(refusal.contains("FORMAT_UPGRADE_REQUIRED"), "{refusal}");
    let before = row(&path);
    let token =
        inspect_sql_trigger_reaction_upgrade(&path, identity(), None, options(&path)).unwrap();
    drop(upgrade_sql_trigger_reactions(token).unwrap());
    assert_eq!(row(&path), before);
    let db = Database::open(&path).unwrap();
    let read = db.begin_read().unwrap();
    let ledger = read.open_table(SQL_TRIGGER_REACTIONS).unwrap();
    assert!(ledger.iter().unwrap().next().is_none());
}
