//! Named sequences (MEDULLA-ENGINE-PARITY-R013.1), driven through the
//! production `TableStore` allocator that `nextval`, `currval` and `setval`
//! call.
#![cfg(feature = "sql")]

use std::path::{Path, PathBuf};

use medulla_table_store::tables::TableStore;
use medulla_tables::sequence::{Sequence, SequenceSource};

fn open(path: &Path) -> TableStore {
    TableStore::open_scoped(
        path,
        "tenant-a",
        medulla_table_store::tables::store::dev_scope_grant::dev_verifier(),
        medulla_table_store::tables::store::dev_scope_grant::DEV_PRINCIPAL,
        medulla_table_store::tables::store::dev_scope_grant::DEV_PROOF,
    )
    .unwrap()
}

fn fresh_path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "medulla_sequence_{tag}_{}_{}.redb",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn small(name: &str) -> Sequence {
    Sequence {
        cache: 4,
        ..Sequence::new(name)
    }
}

// spec: MEDULLA-ENGINE-PARITY-R013.1
#[test]
fn values_never_repeat_after_crash() {
    let path = fresh_path("crash");
    let store = open(&path);
    assert!(store
        .create_sequence(&small("orders_id_seq"), false)
        .unwrap());
    assert!(!store
        .create_sequence(&small("orders_id_seq"), true)
        .unwrap());
    let first: Vec<i64> = (0..6)
        .map(|_| store.sequence_next("orders_id_seq").unwrap())
        .collect();
    assert_eq!(
        first,
        vec![1, 2, 3, 4, 5, 6],
        "default start 1, increment 1"
    );
    // Six values were issued from two reserved blocks of four: 1..=8 reserved.
    assert_eq!(store.sequence_high_mark("orders_id_seq").unwrap(), 9);
    // Crash: the in-memory block (7, 8) is lost.
    drop(store);
    let store = open(&path);
    let after = store.sequence_next("orders_id_seq").unwrap();
    assert_eq!(after, 9, "issuing resumes from the persisted high mark");
    assert!(
        after > *first.last().unwrap(),
        "no value repeats; 7 and 8 are a gap"
    );
    let _ = std::fs::remove_file(&path);
}

// spec: MEDULLA-ENGINE-PARITY-R013.1
#[test]
fn definition_options_and_defaults_are_enforced() {
    let store = open(&fresh_path("defs"));
    let stepped = Sequence {
        name: "by_ten".into(),
        start: 100,
        increment: 10,
        cache: 2,
    };
    store.create_sequence(&stepped, false).unwrap();
    let values: Vec<i64> = (0..3)
        .map(|_| store.sequence_next("by_ten").unwrap())
        .collect();
    assert_eq!(values, vec![100, 110, 120]);
    for bad in [
        Sequence {
            cache: 0,
            ..Sequence::new("a")
        },
        Sequence {
            cache: 65_537,
            ..Sequence::new("a")
        },
        Sequence {
            increment: 0,
            ..Sequence::new("a")
        },
        Sequence {
            start: -1,
            ..Sequence::new("a")
        },
        Sequence::new(""),
    ] {
        let error = store.create_sequence(&bad, false).unwrap_err();
        assert!(error.starts_with("SEQUENCE_INVALID"), "{error}");
    }
    assert_eq!(Sequence::new("d").cache, 32);
}

// spec: MEDULLA-ENGINE-PARITY-R013.1
#[test]
fn unknown_sequence_and_setval_rewind_are_refused() {
    let store = open(&fresh_path("refuse"));
    let error = store.sequence_next("nope").unwrap_err();
    assert!(error.starts_with("SEQUENCE_UNKNOWN"), "{error}");
    store.create_sequence(&small("s"), false).unwrap();
    assert_eq!(store.sequence_next("s").unwrap(), 1); // reserves 1..=4, high mark 5
    let error = store.sequence_set("s", 4).unwrap_err();
    assert!(error.starts_with("SEQUENCE_REWIND"), "{error}");
    assert_eq!(store.sequence_set("s", 50).unwrap(), 50);
    assert_eq!(store.sequence_next("s").unwrap(), 51, "next follows setval");
    assert!(store.drop_sequence("s", false).unwrap());
    assert!(!store.drop_sequence("s", true).unwrap());
    let error = store.sequence_next("s").unwrap_err();
    assert!(error.starts_with("SEQUENCE_UNKNOWN"), "{error}");
}
