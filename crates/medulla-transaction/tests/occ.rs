use medulla_transaction::occ::{disjoint, CommittedWrites, KeyRange, TxnConflict, TxnFootprint};

fn key(table: &str, k: &str) -> KeyRange {
    KeyRange::point(table, k.as_bytes())
}

// spec: MEDULLA-ENGINE-PARITY-R002.1
#[test]
fn write_skew_is_detected() {
    // Invariant: at least one of x and y stays on call. Each transaction
    // reads both and turns one off.
    let t1 = TxnFootprint {
        snapshot_seq: 10,
        reads: vec![key("oncall", "x"), key("oncall", "y")],
        writes: vec![key("oncall", "x")],
    };
    let t2 = TxnFootprint {
        snapshot_seq: 10,
        reads: vec![key("oncall", "x"), key("oncall", "y")],
        writes: vec![key("oncall", "y")],
    };
    assert!(!disjoint(&t1, &t2));
    let mut log = Vec::new();
    assert_eq!(t1.validate(&log), Ok(()));
    log.push(CommittedWrites {
        commit_seq: 11,
        writes: t1.writes.clone(),
    });
    let conflict = t2.validate(&log).unwrap_err();
    assert_eq!(conflict.keys, vec![key("oncall", "x")]);
    assert_eq!(TxnConflict::CODE, "TXN_CONFLICT");

    // A commit at or before the snapshot is already visible: no conflict.
    let late = TxnFootprint {
        snapshot_seq: 11,
        ..t2.clone()
    };
    assert_eq!(late.validate(&log), Ok(()));

    // Ranges conflict with the point keys inside them.
    let scan = TxnFootprint {
        snapshot_seq: 10,
        reads: vec![KeyRange {
            table: "oncall".to_string(),
            start: b"a".to_vec(),
            end: b"z".to_vec(),
        }],
        writes: vec![],
    };
    assert!(scan.validate(&log).is_err());
}

// spec: MEDULLA-ENGINE-PARITY-R004.1
#[test]
fn disjoint_footprints_never_wait() {
    let writers: Vec<TxnFootprint> = (0..32)
        .map(|i| TxnFootprint {
            snapshot_seq: 0,
            reads: vec![key("acct", &format!("r{i}"))],
            writes: vec![key("acct", &format!("w{i}"))],
        })
        .collect();
    for (i, a) in writers.iter().enumerate() {
        for b in writers.iter().skip(i + 1) {
            assert!(disjoint(a, b));
        }
    }
    // Every writer commits after every other: validation against all the
    // other commits still passes, with no shared lock between the threads.
    let log: Vec<CommittedWrites> = writers
        .iter()
        .enumerate()
        .map(|(i, w)| CommittedWrites {
            commit_seq: i as u64 + 1,
            writes: w.writes.clone(),
        })
        .collect();
    std::thread::scope(|scope| {
        let handles: Vec<_> = writers
            .iter()
            .enumerate()
            .map(|(i, w)| {
                let others: Vec<CommittedWrites> = log
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| *j != i)
                    .map(|(_, c)| c.clone())
                    .collect();
                scope.spawn(move || w.validate(&others))
            })
            .collect();
        for handle in handles {
            assert_eq!(handle.join().unwrap(), Ok(()));
        }
    });
}

// spec: MEDULLA-ENGINE-PARITY-R002.2
#[test]
fn conflict_refuses_and_writes_nothing() {
    // A pinned transaction whose read key was rewritten after its snapshot
    // refuses with every conflicting key, and validation itself changes no
    // history: the same log validates identically a second time.
    let fp = TxnFootprint {
        snapshot_seq: 3,
        reads: vec![key("acct", "a")],
        writes: vec![key("acct", "a"), key("acct", "b")],
    };
    let log = vec![CommittedWrites {
        commit_seq: 4,
        writes: vec![key("acct", "b")],
    }];
    let before = log.clone();
    let first = fp.validate(&log).unwrap_err();
    assert_eq!(first.keys, vec![key("acct", "b")]);
    assert_eq!(fp.validate(&log).unwrap_err(), first);
    assert_eq!(log, before, "validation never writes history");
    // A whole-table DDL range conflicts with every point write in that table.
    let ddl = TxnFootprint {
        snapshot_seq: 3,
        reads: vec![],
        writes: vec![KeyRange::whole_table("acct")],
    };
    assert!(ddl.validate(&log).is_err());
}
