//! Attached SQLite file source (MEDULLA-UNIFIED-DATA-PLANE-R015.2.2): watermark capture
//! emits each change once, and a live file reads and captures cleanly under concurrent
//! writers. The fixtures are authored by the real `sqlite3` CLI (`$MEDULLA_SQLITE3`, else
//! `sqlite3` on `$PATH`); the tests fail, never skip, when it is not runnable.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use medulla_durable::idempotent_consumer::ConsumerCheckpoint;
use medulla_federation::sqlite_foreign;
use medulla_plan::sqlite_capture::{poll_watermark, CaptureRefusal};

const BUSY: Duration = Duration::from_secs(5);

fn sqlite3_bin() -> OsString {
    let bin = std::env::var_os("MEDULLA_SQLITE3").unwrap_or_else(|| OsString::from("sqlite3"));
    match Command::new(&bin).arg("--version").output() {
        Ok(out) if out.status.success() => bin,
        other => {
            panic!("these tests need the real sqlite3 CLI, but {bin:?} is not runnable ({other:?})")
        }
    }
}

fn sqlite(db: &Path, script: &str) -> String {
    let mut child = Command::new(sqlite3_bin())
        .arg(db)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn sqlite3");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "sqlite3 failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn checkpoint(position: u64) -> ConsumerCheckpoint {
    ConsumerCheckpoint {
        consumer_id: "attached-sqlite".to_string(),
        position,
    }
}

fn rowids(envelopes: &[medulla_durable::change_envelope::ChangeEnvelope]) -> Vec<String> {
    envelopes
        .iter()
        .map(|e| e.key.as_ref().unwrap()["rowid"].clone())
        .collect()
}

#[test]
fn watermark_poll_emits_each_change_once() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("w.db");
    sqlite(
        &db,
        "PRAGMA journal_mode=WAL;
         CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT, updated_at INTEGER);
         CREATE INDEX t_updated ON t(updated_at);
         CREATE TABLE u (id INTEGER PRIMARY KEY, updated_at INTEGER);
         INSERT INTO t VALUES (1,'a',10),(2,'b',11),(3,'c',12);",
    );

    let mut cp = checkpoint(0);
    let first = poll_watermark(&db, "t", "updated_at", &mut cp, BUSY).unwrap();
    assert_eq!(rowids(&first), ["1", "2", "3"]);
    assert!(first.iter().all(|e| e.source_position == "12"));
    assert_eq!(cp.position, 12);
    assert!(poll_watermark(&db, "t", "updated_at", &mut cp, BUSY)
        .unwrap()
        .is_empty());

    // An insert and an update: one envelope each, in watermark order.
    sqlite(
        &db,
        "INSERT INTO t VALUES (4,'d',13); UPDATE t SET v='B', updated_at=14 WHERE id=2;",
    );
    let next = poll_watermark(&db, "t", "updated_at", &mut cp, BUSY).unwrap();
    assert_eq!(rowids(&next), ["4", "2"]);
    assert!(next.iter().all(|e| e.source_position == "14"));

    // A restart resumes from the stored position: nothing is emitted twice.
    let mut restarted = checkpoint(cp.position);
    assert!(poll_watermark(&db, "t", "updated_at", &mut restarted, BUSY)
        .unwrap()
        .is_empty());

    // The rowid watermark sees only new rows.
    let mut by_rowid = checkpoint(2);
    let tail = poll_watermark(&db, "t", "rowid", &mut by_rowid, BUSY).unwrap();
    assert_eq!(rowids(&tail), ["3", "4"]);

    // A missing or unindexed watermark column is refused with the registered code.
    for (table, column) in [("t", "no_such_column"), ("u", "updated_at")] {
        let refusal = poll_watermark(&db, table, column, &mut checkpoint(0), BUSY).unwrap_err();
        assert!(matches!(refusal, CaptureRefusal::WatermarkInvalid { .. }));
        assert_eq!(refusal.code(), Some("SQLITE_WATERMARK_INVALID"));
    }
}

#[test]
fn live_file_reads_and_capture_under_concurrent_writer() {
    const WRITERS: usize = 4;
    const PER_WRITER: usize = 500;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("live.db");
    sqlite(
        &db,
        "PRAGMA journal_mode=WAL;
         CREATE TABLE t (id INTEGER PRIMARY KEY, w INTEGER, v TEXT, updated_at INTEGER);
         CREATE INDEX t_updated ON t(updated_at);",
    );

    let writers: Vec<_> = (0..WRITERS)
        .map(|w| {
            let db = db.clone();
            std::thread::spawn(move || {
                let mut script = String::from("PRAGMA busy_timeout=30000;\n");
                for n in 0..PER_WRITER {
                    script.push_str(&format!(
                        "INSERT INTO t(w,v,updated_at) VALUES ({w},'w{w}-{n}',(SELECT COALESCE(MAX(id),0)+1 FROM t));\n"
                    ));
                }
                sqlite(&db, &script);
            })
        })
        .collect();

    // The engine reads and polls while the writers run: zero errors, each change once.
    let mut cp = checkpoint(0);
    let mut seen = BTreeSet::new();
    let mut poll = |cp: &mut ConsumerCheckpoint| {
        for id in rowids(&poll_watermark(&db, "t", "updated_at", cp, BUSY).expect("poll")) {
            assert!(seen.insert(id.clone()), "rowid {id} captured twice");
        }
    };
    while !writers.iter().all(|w| w.is_finished()) {
        sqlite_foreign::fetch(db.to_str().unwrap(), "t", &[]).expect("a live read never errors");
        poll(&mut cp);
    }
    for writer in writers {
        writer.join().unwrap();
    }
    poll(&mut cp);
    assert_eq!(seen.len(), WRITERS * PER_WRITER);

    // The rows equal the `sqlite3` result.
    let rows = sqlite_foreign::fetch(db.to_str().unwrap(), "t", &[]).unwrap();
    let mut ours: Vec<(i64, String)> = rows
        .rows()
        .iter()
        .map(|r| {
            (
                r.id.parse().unwrap(),
                format!(
                    "{}|{}|{}",
                    r.id,
                    r.columns["w"],
                    r.columns["v"].as_str().unwrap()
                ),
            )
        })
        .collect();
    ours.sort();
    let theirs = sqlite(&db, "SELECT id||'|'||w||'|'||v FROM t ORDER BY id;");
    let ours: Vec<String> = ours.into_iter().map(|(_, line)| line).collect();
    assert_eq!(ours.join("\n"), theirs.trim_end());
}
