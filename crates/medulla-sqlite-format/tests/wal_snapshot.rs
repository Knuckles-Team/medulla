//! Lock-free WAL snapshot reader (MEDULLA-UNIFIED-DATA-PLANE-R015.2.2.1): the reader sees
//! the last committed WAL state, ignores a torn or uncommitted tail, never writes the
//! files, and refuses with the snapshot-unstable code when a checkpoint keeps racing it.

use std::path::Path;
use std::time::{Duration, Instant};

use medulla_sqlite_format::{ColumnDef, Error, Reader, SqliteReadRefusal, Value, Writer};

const PAGE: usize = 4096;

fn build_db(path: &Path, ids: &[i64]) {
    let mut w = Writer::create(path, PAGE as u32).unwrap();
    let cols = vec![
        ColumnDef {
            name: "id".into(),
            decl_type: "INTEGER".into(),
        },
        ColumnDef {
            name: "v".into(),
            decl_type: "TEXT".into(),
        },
    ];
    w.add_table("t", &cols).unwrap();
    let rows: Vec<_> = ids
        .iter()
        .map(|id| vec![Value::Integer(*id), Value::Text(format!("v{id}"))])
        .collect();
    w.insert_rows("t", &rows).unwrap();
    w.finish().unwrap();
}

fn checksum(big: bool, data: &[u8], seed: (u32, u32)) -> (u32, u32) {
    let word = |b: &[u8]| {
        let raw = [b[0], b[1], b[2], b[3]];
        if big {
            u32::from_be_bytes(raw)
        } else {
            u32::from_le_bytes(raw)
        }
    };
    let (mut s0, mut s1) = seed;
    for c in data.chunks_exact(8) {
        s0 = s0.wrapping_add(word(&c[0..4])).wrapping_add(s1);
        s1 = s1.wrapping_add(word(&c[4..8])).wrapping_add(s0);
    }
    (s0, s1)
}

/// One valid frame; `commit` is the database size in pages (0 for a non-commit frame).
fn frame(
    big: bool,
    salts: (u32, u32),
    run: &mut (u32, u32),
    no: u32,
    commit: u32,
    data: &[u8],
) -> Vec<u8> {
    let mut head = Vec::new();
    head.extend_from_slice(&no.to_be_bytes());
    head.extend_from_slice(&commit.to_be_bytes());
    head.extend_from_slice(&salts.0.to_be_bytes());
    head.extend_from_slice(&salts.1.to_be_bytes());
    let sum = checksum(big, &head[..8], *run);
    let sum = checksum(big, data, sum);
    *run = sum;
    head.extend_from_slice(&sum.0.to_be_bytes());
    head.extend_from_slice(&sum.1.to_be_bytes());
    head.extend_from_slice(data);
    head
}

/// A WAL holding every page of `image` as one committed transaction, then an uncommitted
/// frame for page 1 of `stale`, then a torn half frame.
fn write_wal(wal: &Path, big: bool, salts: (u32, u32), image: &[u8], stale: &[u8]) {
    let mut out = Vec::new();
    let magic: u32 = if big { 0x377f_0683 } else { 0x377f_0682 };
    out.extend_from_slice(&magic.to_be_bytes());
    out.extend_from_slice(&3_007_000u32.to_be_bytes());
    out.extend_from_slice(&(PAGE as u32).to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(&salts.0.to_be_bytes());
    out.extend_from_slice(&salts.1.to_be_bytes());
    let head_sum = checksum(big, &out, (0, 0));
    out.extend_from_slice(&head_sum.0.to_be_bytes());
    out.extend_from_slice(&head_sum.1.to_be_bytes());
    let mut run = head_sum;
    let pages = image.len() / PAGE;
    for (i, data) in image.chunks(PAGE).enumerate() {
        let commit = if i + 1 == pages { pages as u32 } else { 0 };
        out.extend(frame(big, salts, &mut run, i as u32 + 1, commit, data));
    }
    out.extend(frame(big, salts, &mut run, 1, 0, &stale[..PAGE]));
    let torn = frame(big, salts, &mut run, 1, pages as u32, &stale[..PAGE]);
    out.extend_from_slice(&torn[..torn.len() / 2]);
    std::fs::write(wal, out).unwrap();
}

fn rows(reader: &Reader) -> Vec<i64> {
    reader
        .scan_table("t")
        .unwrap()
        .iter()
        .map(|r| match r[0] {
            Value::Integer(i) => i,
            ref other => panic!("unexpected {other:?}"),
        })
        .collect()
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R015.2.2.1
#[test]
fn snapshot_sees_last_commit_and_survives_checkpoint_race() {
    let dir = tempfile::tempdir().unwrap();
    let (main, newer, wal) = (
        dir.path().join("a.db"),
        dir.path().join("b.db"),
        dir.path().join("a.db-wal"),
    );
    build_db(&main, &[1]);
    build_db(&newer, &[1, 2, 3]);
    let image = std::fs::read(&newer).unwrap();
    let stale = std::fs::read(&main).unwrap();
    let main_before = stale.clone();

    // Both checksum byte orders: committed rows are visible, the uncommitted and torn
    // tail frames are not, and the main file is never written.
    for big in [false, true] {
        write_wal(&wal, big, (7, 9), &image, &stale);
        let reader = Reader::open_with_wal(&main, &wal, Duration::from_millis(50)).unwrap();
        assert_eq!(rows(&reader), vec![1, 2, 3], "big_endian={big}");
        assert_eq!(std::fs::read(&main).unwrap(), main_before);
    }

    // A checkpoint (salt change) racing the first read: the retry settles on the new WAL.
    write_wal(&wal, false, (7, 9), &image, &stale);
    let mut raced = Vec::new();
    let reader = Reader::open_with_wal_observed(&main, &wal, Duration::from_secs(5), &mut |n| {
        raced.push(n);
        if n == 0 {
            write_wal(&wal, false, (11, 13), &image, &stale);
        }
    })
    .unwrap();
    assert_eq!(rows(&reader), vec![1, 2, 3]);
    assert_eq!(raced, vec![0, 1]);

    // A checkpoint that never stops: after the timeout the read refuses, typed.
    let timeout = Duration::from_millis(40);
    let started = Instant::now();
    let err = Reader::open_with_wal_observed(&main, &wal, timeout, &mut |n| {
        write_wal(&wal, false, (100 + n, 1), &image, &stale);
    })
    .map(|_| ())
    .unwrap_err();
    assert!(started.elapsed() >= timeout);
    match err {
        Error::Refused(refusal @ SqliteReadRefusal::SnapshotUnstable) => {
            assert_eq!(refusal.code(), Some("SQLITE_SNAPSHOT_UNSTABLE"));
        }
        other => panic!("expected the snapshot-unstable refusal, got {other:?}"),
    }
    assert_eq!(std::fs::read(&main).unwrap(), main_before);
}
