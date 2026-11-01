//! Encryption-at-rest + tamper-evident audit proofs over the durable store
//! (CONCEPT:MEDULLA-KG.sharding.row-level-security), exercised through the SAME `commit_ops`/read/`verify_audit`
//! the server + embedded engine use.
use super::*;
use medulla_kernel::crypto::ValueCipher;
use medulla_kernel::test_rendezvous::join_bounded;

fn open_db(dir: &std::path::Path) -> Shard {
    let path = dir.join("graph-0.redb");
    Shard::open(&path).unwrap()
}

fn add_node_method(node_id: &str, props: serde_json::Value) -> Method {
    Method::AddNode {
        node_id: node_id.to_string(),
        properties_msgpack: rmp_serde::to_vec_named(&props).unwrap(),
    }
}

fn add_edge_method(src: &str, tgt: &str) -> Method {
    Method::AddEdge {
        source_id: src.to_string(),
        target_id: tgt.to_string(),
        properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({})).unwrap(),
    }
}

/// Read back the stored ordinals for one (graph,src,tgt) in ascending order.
fn edge_ords(shard: &Shard, graph: &str, src: &str, tgt: &str) -> Vec<u32> {
    let handle = shard.graph(graph).unwrap();
    let read = shard.read(&handle).unwrap();
    let edges = read.scoped_owner_table(EDGES).unwrap();
    edges
        .scope_rows()
        .unwrap()
        .map(|row| row.expect("read durable edge row"))
        .filter(|(k, _)| {
            let (g, s, t, _) = k.value();
            g == graph && s == src && t == tgt
        })
        .map(|(k, _)| k.value().3)
        .collect()
}

fn next_drain_id(tag: &str) -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "{tag}-{}",
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

fn tamper_audit_row(shard: &Shard, graph: &str, sequence: u64) {
    let members = shard.graph_members(&[graph]).unwrap();
    let op_id = next_drain_id("tamper-audit");
    let (group, batches) = shard.admit_maintenance(&members, &op_id).unwrap();
    let write = ShardWrite::open(shard, &group, &members, &batches).unwrap();
    {
        let mut audit = write
            .graph(graph)
            .unwrap()
            .open_scoped_table(AUDIT)
            .unwrap();
        let original = audit
            .get((graph, sequence))
            .unwrap()
            .unwrap()
            .value()
            .to_vec();
        let mut mutated = original;
        let last = mutated.len() - 1;
        mutated[last] ^= 0xFF;
        audit.insert((graph, sequence), mutated.as_slice()).unwrap();
    }
    write.finish().unwrap();
    shard.commit_drain(group, &batches, 0).unwrap();
}

#[test]
fn hierarchical_edge_ordinal_cache_invalidates_only_the_requested_scope() {
    EDGE_ORD_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        cache.clear();
        cache
            .entry("g1".into())
            .or_default()
            .entry("a".into())
            .or_default()
            .extend([("b".into(), 2), ("c".into(), 3)]);
        cache
            .entry("g1".into())
            .or_default()
            .entry("x".into())
            .or_default()
            .insert("y".into(), 4);
        cache
            .entry("g2".into())
            .or_default()
            .entry("a".into())
            .or_default()
            .insert("b".into(), 5);
    });

    invalidate_edge_ord("g1", "a", "b");
    EDGE_ORD_CACHE.with(|cache| {
        let cache = cache.borrow();
        assert!(!cache["g1"]["a"].contains_key("b"));
        assert_eq!(cache["g1"]["a"]["c"], 3);
        assert_eq!(cache["g2"]["a"]["b"], 5);
    });

    invalidate_node_edge_ords("g1", "a");
    EDGE_ORD_CACHE.with(|cache| {
        let cache = cache.borrow();
        assert!(!cache["g1"].contains_key("a"));
        assert_eq!(cache["g1"]["x"]["y"], 4);
    });

    invalidate_graph_edge_ords("g1");
    EDGE_ORD_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        assert!(!cache.contains_key("g1"));
        assert_eq!(cache["g2"]["a"]["b"], 5);
        cache.clear();
    });
}

#[test]
fn edge_ordinal_cache_assigns_u32_max_once_then_fails_closed() {
    let dir = tempdir();
    let writer = std::thread::Builder::new()
        .name("medulla-redb-writer-exhaustion-test".to_string())
        .spawn(move || {
            let db = open_db(&dir);
            let members = db.graph_members(&["g"]).unwrap();
            let op_id = next_drain_id("edge-space");
            let (group, batches) = db.admit_maintenance(&members, &op_id).unwrap();
            let write = ShardWrite::open(&db, &group, &members, &batches).unwrap();
            let edges = write.graph("g").unwrap().open_scoped_table(EDGES).unwrap();
            EDGE_ORD_CACHE.with(|cache| {
                cache
                    .borrow_mut()
                    .entry("g".into())
                    .or_default()
                    .entry("a".into())
                    .or_default()
                    .insert("b".into(), u64::from(u32::MAX));
            });

            assert_eq!(next_edge_ordinal(&edges, "g", "a", "b").unwrap(), u32::MAX);
            assert_eq!(
                next_edge_ordinal(&edges, "g", "a", "b").unwrap_err(),
                "edge ordinal space exhausted"
            );
            drop(edges);
            write.finish().unwrap();
            db.commit_drain(group, &batches, 0).unwrap();
            EDGE_ORD_CACHE.with(|cache| cache.borrow_mut().clear());
        })
        .unwrap();
    join_bounded(writer, "the edge-ordinal writer thread");
}

/// CONCEPT:MEDULLA-KG.storage.redb-store #3 — the O(1) edge-ordinal counter assigns CORRECT, strictly
/// monotonic ordinals across many `AddEdge` to one node (per-op across SEPARATE commit
/// batches on the dedicated writer thread — the hot path that used to range-scan every
/// time), and a FRESH writer thread (the restart case) RE-SEEDS each (src,tgt) from one
/// bounded tail seek and continues with no gap, reset, or collision. `RemoveEdge` invalidates the
/// counter so a re-add resets to 0, matching the old scan behavior exactly.
#[test]
fn edge_ordinals_monotonic_o1_counter_and_reseed_after_restart() {
    let dir = tempdir();

    // PHASE 1 — on a dedicated `medulla-redb-writer*` thread so the MEDULLA-029 counter is active.
    let d1 = dir.clone();
    let writer = std::thread::Builder::new()
        .name("medulla-redb-writer-egtest".to_string())
        .spawn(move || {
            let crypto = DurableCrypto::none();
            let db = open_db(&d1);
            let mut tail = AuditTailCache::new();
            let mut commit = |m: Method| {
                let mut ops = vec![("g".to_string(), m)];
                let mut log = Vec::new();
                commit_ops(
                    &db,
                    &mut ops,
                    &mut log,
                    &next_drain_id("security-edge"),
                    0,
                    crypto,
                    &mut tail,
                )
                .unwrap();
            };
            // AddEdge now requires both endpoints to already be durable nodes
            // (redb_store.rs's "AddEdge requires durable endpoints" guard) --
            // seed the three nodes this test's edges reference before adding
            // any edge between them.
            commit(add_node_method("a", serde_json::json!({})));
            commit(add_node_method("b", serde_json::json!({})));
            commit(add_node_method("c", serde_json::json!({})));

            // 6 multi-edges a->b across SEPARATE batches (cross-batch in-RAM counter),
            // interleaved with 2 a->c.
            for _ in 0..6 {
                commit(add_edge_method("a", "b"));
            }
            commit(add_edge_method("a", "c"));
            commit(add_edge_method("a", "c"));
            assert_eq!(edge_ords(&db, "g", "a", "b"), vec![0, 1, 2, 3, 4, 5]);
            assert_eq!(edge_ords(&db, "g", "a", "c"), vec![0, 1]);

            // RemoveEdge invalidates the counter → re-add resets to 0 (old behavior).
            commit(Method::RemoveEdge {
                source_id: "a".into(),
                target_id: "c".into(),
            });
            assert_eq!(edge_ords(&db, "g", "a", "c"), Vec::<u32>::new());
            commit(add_edge_method("a", "c"));
            assert_eq!(edge_ords(&db, "g", "a", "c"), vec![0]);
        })
        .unwrap();
    join_bounded(writer, "the edge-ordinal writer thread");

    // PHASE 2 — RESTART: reopen the SAME file on a NEW writer thread (fresh thread-local
    // counter). Adding 3 more a->b must RE-SEED from one scan (max was 5) and continue
    // 6,7,8 — monotonic, no reset, no collision.
    let d2 = dir.clone();
    let writer = std::thread::Builder::new()
        .name("medulla-redb-writer-egtest".to_string())
        .spawn(move || {
            let crypto = DurableCrypto::none();
            let db = open_db(&d2);
            let mut tail = AuditTailCache::new();
            for _ in 0..3 {
                let mut ops = vec![("g".to_string(), add_edge_method("a", "b"))];
                let mut log = Vec::new();
                commit_ops(
                    &db,
                    &mut ops,
                    &mut log,
                    &next_drain_id("security-edge-restart"),
                    0,
                    crypto,
                    &mut tail,
                )
                .unwrap();
            }
            assert_eq!(
                edge_ords(&db, "g", "a", "b"),
                vec![0, 1, 2, 3, 4, 5, 6, 7, 8],
                "re-seeded counter must continue monotonically after restart"
            );
        })
        .unwrap();
    join_bounded(writer, "the edge-ordinal writer thread");
}

/// CONCEPT:MEDULLA-KG.storage.redb-store #4 — with encryption OFF, `seal` returns `Cow::Borrowed` and the
/// stored value blob is BYTE-FOR-BYTE the caller's plaintext (zero clone, no format
/// change). Proven by reading the stored bytes back and comparing to the input.
#[test]
fn seal_off_stores_plaintext_bytes_byte_identical() {
    let dir = tempdir();
    let crypto = DurableCrypto::none();
    let db = open_db(&dir);
    let pbytes = rmp_serde::to_vec_named(&serde_json::json!({"k": "v-plain-123"})).unwrap();
    let mut ops = vec![(
        "g".to_string(),
        Method::AddNode {
            node_id: "n".to_string(),
            properties_msgpack: pbytes.clone(),
        },
    )];
    let mut log = Vec::new();
    let mut tail = AuditTailCache::new();
    commit_ops(
        &db,
        &mut ops,
        &mut log,
        &next_drain_id("security-plaintext"),
        0,
        crypto,
        &mut tail,
    )
    .unwrap();

    let handle = db.graph("g").unwrap();
    let read = db.read(&handle).unwrap();
    let nodes = read.scoped_owner_table(NODES).unwrap();
    let stored = nodes.get(("g", "n")).unwrap().unwrap().value().to_vec();
    assert_eq!(
        stored, pbytes,
        "encryption-off stored bytes must equal the input plaintext (seal = identity)"
    );
}

#[test]
fn encryption_no_plaintext_on_disk_round_trips_and_wrong_key_fails() {
    let dir = tempdir();
    let db_path = dir.join("graph-0.redb");
    let cipher = ValueCipher::from_key_material(b"correct-horse-battery-staple");
    let crypto = DurableCrypto::new(Some(&cipher));

    // Write a node carrying a recognizable SECRET via the durable write path.
    {
        let db = open_db(&dir);
        let mut ops = vec![(
            "g".to_string(),
            add_node_method("n1", serde_json::json!({"ssn": "SECRET-123-45-6789"})),
        )];
        let mut log = Vec::new();
        let mut audit_tail = AuditTailCache::new();
        commit_ops(
            &db,
            &mut ops,
            &mut log,
            &next_drain_id("security-encrypted"),
            0,
            crypto,
            &mut audit_tail,
        )
        .unwrap();
    }

    // The raw on-disk redb bytes must NOT contain the plaintext secret.
    let raw = std::fs::read(&db_path).unwrap();
    let needle = b"SECRET-123-45-6789";
    assert!(
        !raw.windows(needle.len()).any(|w| w == needle),
        "plaintext node property leaked into raw redb file"
    );

    // It round-trips with the right key.
    {
        let db = open_db(&dir);
        let dumps = read_all_dumps(&db, crypto).unwrap();
        let g = dumps.iter().find(|d| d.graph == "g").expect("graph g");
        let (_, props) = &g.nodes[0];
        let m: serde_json::Value = rmp_serde::from_slice(props).unwrap();
        assert_eq!(m["ssn"], "SECRET-123-45-6789");
    }

    // A WRONG key fails to decrypt (never silent plaintext).
    {
        let db = open_db(&dir);
        let wrong = ValueCipher::from_key_material(b"totally-different-key");
        let res = read_all_dumps(&db, DurableCrypto::new(Some(&wrong)));
        assert!(res.is_err(), "wrong key must not decrypt");
    }
}

#[test]
fn audit_chain_verifies_clean_and_detects_tampering() {
    let dir = tempdir();
    let crypto = DurableCrypto::none();
    let db = open_db(&dir);

    // Three durable mutations → three chained audit entries.
    let mut audit_tail = AuditTailCache::new();
    for (i, m) in [
        add_node_method("a", serde_json::json!({"v": 1})),
        add_node_method("b", serde_json::json!({"v": 2})),
        Method::AddEdge {
            source_id: "a".into(),
            target_id: "b".into(),
            properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({})).unwrap(),
        },
    ]
    .into_iter()
    .enumerate()
    {
        let _ = i;
        let mut ops = vec![("g".to_string(), m)];
        let mut log = Vec::new();
        commit_ops(
            &db,
            &mut ops,
            &mut log,
            &next_drain_id("security-audit"),
            0,
            crypto,
            &mut audit_tail,
        )
        .unwrap();
    }

    // A clean chain verifies.
    let report = verify_audit(&db, "g").unwrap();
    assert!(report.ok, "{report:?}");
    assert_eq!(report.entries, 3);

    // Tamper entry seq=1: flip its stored line/hash bytes directly in the table.
    tamper_audit_row(&db, "g", 1);

    let broken = verify_audit(&db, "g").unwrap();
    assert!(!broken.ok, "tamper undetected");
    assert_eq!(broken.first_broken_seq, Some(1), "wrong break position");
}

#[cfg(feature = "security")]
#[test]
fn operation_audit_reservation_outcome_replay_and_restart() {
    let dir = tempdir();
    let db = open_db(&dir);
    let mut tail = AuditTailCache::new();
    let mut ops = vec![(
        "g".to_string(),
        add_node_method("seed", serde_json::json!({})),
    )];
    commit_ops(
        &db,
        &mut ops,
        &mut Vec::new(),
        &next_drain_id("audit-seed"),
        0,
        DurableCrypto::none(),
        &mut tail,
    )
    .unwrap();
    let mut event = OperationAuditEvent {
        tenant: "tenant-a".into(),
        principal: "svc:graph-os".into(),
        op: "graph.nodes.write".into(),
        surface: "http".into(),
        params_sha256: "a".repeat(64),
        status: "reserved".into(),
        request_id: "request-a".into(),
        audit_class: "event".into(),
    };
    let reserved = operation_audit_append(&db, &mut tail, "g", &event).unwrap();
    assert!(!reserved.replayed);
    assert_eq!(
        operation_audit_append(&db, &mut tail, "g", &event)
            .unwrap()
            .seq,
        reserved.seq
    );
    event.status = "ok".into();
    let outcome = operation_audit_append(&db, &mut tail, "g", &event).unwrap();
    assert!(!outcome.replayed);
    assert_eq!(
        operation_audit_append(&db, &mut tail, "g", &event)
            .unwrap()
            .seq,
        outcome.seq
    );
    event.status = "error".into();
    assert!(operation_audit_append(&db, &mut tail, "g", &event)
        .unwrap_err()
        .contains("AUDIT_IDEMPOTENCY_CONFLICT"));
    assert!(
        operation_audit_read(&db, "g", "tenant-a", outcome.seq)
            .unwrap()
            .chain_verified
    );
    assert!(operation_audit_read(&db, "g", "tenant-b", outcome.seq).is_err());
    drop(db);
    let reopened = open_db(&dir);
    let mut cold_tail = AuditTailCache::new();
    event.status = "ok".into();
    assert!(
        operation_audit_append(&reopened, &mut cold_tail, "g", &event)
            .unwrap()
            .replayed
    );
    let proof = operation_audit_read(&reopened, "g", "tenant-a", reserved.seq).unwrap();
    assert!(proof.chain_verified);
    assert!(proof.event_line.contains("status=reserved"));
    event.request_id = "never-reserved".into();
    assert!(
        operation_audit_append(&reopened, &mut cold_tail, "g", &event)
            .unwrap_err()
            .contains("AUDIT_RESERVATION_REQUIRED")
    );
}

/// CONCEPT:MEDULLA-KG.storage.embedded-store — the O(1) tail-cache append produces an IDENTICAL, verifiable
/// chain to the old per-op scan across: (1) many ops in ONE commit batch
/// (intra-batch chaining off RAM), (2) several commit batches reusing the cache
/// (inter-batch), and (3) a fresh cache that must RE-SEED the tail from one scan
/// (the restart case) and continue the chain without a gap. Two interleaved graphs
/// prove per-graph isolation of the cache.
#[test]
fn audit_tail_cache_o1_append_builds_verifiable_chain_across_batches_and_restart() {
    let dir = tempdir();
    let crypto = DurableCrypto::none();
    let db = open_db(&dir);

    // Helper: commit a batch of (graph, node) AddNode ops through commit_ops with a
    // caller-owned cache (mirrors the writer thread's persistent cache).
    let commit_batch = |shard: &Shard, cache: &mut AuditTailCache, batch: &[(&str, &str)]| {
        let mut ops: Vec<(String, Method)> = batch
            .iter()
            .map(|(g, n)| {
                (
                    g.to_string(),
                    add_node_method(n, serde_json::json!({"n": n})),
                )
            })
            .collect();
        let mut log = Vec::new();
        commit_ops(
            shard,
            &mut ops,
            &mut log,
            &next_drain_id("security-cache"),
            0,
            crypto,
            cache,
        )
        .unwrap();
    };

    // Batch 1: 5 ops for "g1" + 3 ops for "g2" in ONE commit (intra-batch chaining,
    // interleaved graphs). The cache seeds each graph once (genesis) then chains in RAM.
    let mut cache = AuditTailCache::new();
    commit_batch(
        &db,
        &mut cache,
        &[
            ("g1", "a"),
            ("g2", "x"),
            ("g1", "b"),
            ("g1", "c"),
            ("g2", "y"),
            ("g1", "d"),
            ("g2", "z"),
            ("g1", "e"),
        ],
    );
    // Cache must reflect the in-RAM tails: g1 saw 5 ops (seq 0..4), g2 saw 3 (seq 0..2).
    assert_eq!(cache.get("g1").unwrap().0, 4, "g1 tail seq");
    assert_eq!(cache.get("g2").unwrap().0, 2, "g2 tail seq");

    // Batch 2: REUSE the same cache (inter-batch). No scan should be needed; the
    // chain must continue seamlessly.
    commit_batch(&db, &mut cache, &[("g1", "f"), ("g2", "w"), ("g1", "g")]);

    // Batch 3: simulate a WRITER RESTART — a brand-new empty cache. The first touch
    // of each graph must RE-SEED the tail from one range-scan and continue with NO gap.
    let mut cache_after_restart = AuditTailCache::new();
    commit_batch(&db, &mut cache_after_restart, &[("g1", "h"), ("g2", "v")]);
    assert_eq!(
        cache_after_restart.get("g1").unwrap().0,
        7,
        "g1 re-seeded tail continues (5+2 prior ⇒ next seq 7)"
    );
    assert_eq!(
        cache_after_restart.get("g2").unwrap().0,
        4,
        "g2 re-seeded tail continues (3+1+1 prior ⇒ seq 4)"
    );

    // The FULL chains must verify clean (tamper-evidence intact, no gaps/breaks).
    let r1 = verify_audit(&db, "g1").unwrap();
    assert!(r1.ok, "g1 chain broken: {r1:?}");
    assert_eq!(r1.entries, 8, "g1 entry count (5+2+1)");
    let r2 = verify_audit(&db, "g2").unwrap();
    assert!(r2.ok, "g2 chain broken: {r2:?}");
    assert_eq!(r2.entries, 5, "g2 entry count (3+1+1)");

    // And tamper-evidence still fires on the cache-built chain.
    tamper_audit_row(&db, "g1", 3);
    let broken = verify_audit(&db, "g1").unwrap();
    assert!(!broken.ok, "tamper on cache-built chain undetected");
    assert_eq!(broken.first_broken_seq, Some(3));
}

/// CONCEPT:MEDULLA-KG.storage.embedded-store — the cold-seed tail lookup is a BOUNDED reverse seek
/// (`(graph, 0)..=(graph, u64::MAX)` + `next_back`), not a forward walk to the end
/// of the chain. Proven by comparing the wall-clock cost of re-seeding (fresh
/// cache, i.e. after a simulated writer restart) a chain of 200,000 prior entries
/// against re-seeding a chain of 5: the chains differ 40,000x in length, but a
/// single bounded reverse seek's cost is independent of that (only the B-tree's
/// O(log n) depth differs — a small constant next to the `*50 + 20ms` slack
/// below). The old `.range((graph, 0u64)..)` + `.last()` forward walk (O(chain
/// length)) would blow well past this bound on the long chain.
#[test]
fn audit_tail_cold_seed_is_a_bounded_seek_not_a_forward_scan() {
    let crypto = DurableCrypto::none();

    // Build a chain of `len` AddNode entries for `graph` in ONE commit batch (the
    // cache stays warm for the whole build, so construction cost is irrelevant —
    // only the POST-RESTART re-seed below is timed), then re-seed from a FRESH
    // cache (simulating a writer restart) and time just that one call.
    let reseed_cost = |graph: &str, len: usize| -> u64 {
        let dir = tempdir();
        let db = open_db(&dir);
        let mut ops: Vec<(String, Method)> = (0..len)
            .map(|i| {
                (
                    graph.to_string(),
                    add_node_method(&format!("n{i}"), serde_json::json!({"i": i})),
                )
            })
            .collect();
        let mut log = Vec::new();
        let mut warm_cache = AuditTailCache::new();
        commit_ops(
            &db,
            &mut ops,
            &mut log,
            &next_drain_id("security-cold-build"),
            0,
            crypto,
            &mut warm_cache,
        )
        .unwrap();

        // Simulated restart: a brand-new empty cache forces the NEXT append to
        // cold-seed the tail from durable state instead of chaining off RAM.
        let mut cold_cache = AuditTailCache::new();
        let mut restart_ops = vec![(
            graph.to_string(),
            add_node_method(&format!("n{len}"), serde_json::json!({"i": len})),
        )];
        let mut restart_log = Vec::new();
        let _ = medulla_store::redb_store::audit::cold_seed_rows_touched_take(); // discard anything the build left
        commit_ops(
            &db,
            &mut restart_ops,
            &mut restart_log,
            &next_drain_id("security-cold-restart"),
            0,
            crypto,
            &mut cold_cache,
        )
        .unwrap();
        let rows_touched = medulla_store::redb_store::audit::cold_seed_rows_touched_take();

        // Correctness: the re-seeded tail must continue the chain with no gap, and
        // the full (len + 1)-entry chain must still verify clean.
        assert_eq!(
            cold_cache.get(graph).unwrap().0,
            len as u64,
            "re-seeded tail must continue at seq {len}"
        );
        let report = verify_audit(&db, graph).unwrap();
        assert!(report.ok, "{report:?}");
        assert_eq!(report.entries, (len + 1) as u64);

        rows_touched
    };

    let short = reseed_cost("g-short", 5);
    let long = reseed_cost("g-long", 200_000);

    // The property under test is STRUCTURAL — "a bounded seek, not a forward
    // scan" — so assert it structurally: the number of audit rows the cold seed
    // pulls must not depend on how long the chain is. A bounded reverse seek
    // pulls exactly one row from a 5-entry chain and exactly one from a
    // 200,000-entry chain; a forward walk pulls 5 and 200,000.
    //
    // This replaces a wall-clock ratio (`long <= short*50 + 20ms`). That budget
    // was BOTH flaky and weak: it failed reproducibly on a shared build host
    // purely from a concurrent job's disk contention (measured 5/5 at 1.6-2.0s
    // against a 0.5-1.1s budget with the mechanism provably unchanged), and it
    // would equally have PASSED a genuine forward-scan regression on a fast
    // enough machine. Counting the rows is deterministic, machine-independent,
    // and strictly stronger.
    assert_eq!(
        short, 1,
        "cold seed of a 5-entry chain should pull exactly one audit row"
    );
    assert_eq!(
        long, short,
        "cold-seed cost must be INDEPENDENT of chain length: a 200,000-entry \
         chain pulled {long} audit row(s) vs {short} for a 5-entry chain — the \
         O(1) bounded reverse seek has regressed to an O(chain length) forward scan"
    );
}

/// A throwaway temp dir under the scratch space.
fn tempdir() -> std::path::PathBuf {
    let base = medulla_kernel::kernel::persistence::temp_dir("medulla-sec", "test");
    std::fs::create_dir_all(&base).unwrap();
    base
}
