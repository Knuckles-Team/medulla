//! Paging resumes by seek, not by skip (MEDULLA-DURABLE-KERNEL-R873.2).

use std::time::{Duration, Instant};

use medulla_protocol_types::protocol::GraphType;
use medulla_store::redb_store::shard::Shard;
use medulla_store::redb_store::DurableCrypto;
use medulla_store_apply::redb_store::checkpoint::apply_checkpoint;
use medulla_store_apply::redb_store::dump::{read_graph_dump_page, PageCursorRef};
use medulla_store_apply::redb_store::store_control::{GraphDump, InPlaceCoreCheckpoint};

const NODES: usize = 6_000;
const PAGE: usize = 50;

fn page_time(shard: &Shard, node_after: Option<&str>) -> Duration {
    let mut best = Duration::MAX;
    for _ in 0..9 {
        let started = Instant::now();
        let page = read_graph_dump_page(
            shard,
            "graph",
            DurableCrypto::none(),
            PageCursorRef {
                node_offset: 0,
                edge_offset: 0,
                node_after,
                edge_after: None,
                page_size: PAGE,
            },
        )
        .unwrap()
        .unwrap();
        best = best.min(started.elapsed());
        assert_eq!(page.nodes.len(), PAGE);
    }
    best
}

// spec: MEDULLA-DURABLE-KERNEL-R873.2
#[test]
fn page_reads_are_independent_of_prefix_length() {
    let path = medulla_store::redb_store::temp_path("medulla-dump-page-seek", "");
    let shard = Shard::open(&path).unwrap();
    let nodes: Vec<_> = (0..NODES)
        .map(|n| (format!("n{n:06}"), vec![0u8; 8]))
        .collect();
    apply_checkpoint(
        &shard,
        &mut Vec::new(),
        vec![GraphDump::in_place_core_checkpoint(InPlaceCoreCheckpoint {
            graph: "graph".to_string(),
            name: "graph".to_string(),
            graph_type: GraphType::Global,
            incarnation_id: "incarnation:test:seek".to_string(),
            source_snapshot_version: 1,
            schema_sources: std::sync::Arc::new(
                medulla_graph_core::graph::GraphSchemaSources::default(),
            ),
            nodes: nodes.clone(),
            edges: Vec::new(),
            ledger: Vec::new(),
            semantic: Vec::new(),
        })],
        DurableCrypto::none(),
    )
    .unwrap();

    let early = page_time(&shard, Some(nodes[PAGE].0.as_str()));
    let deep = page_time(&shard, Some(nodes[NODES - PAGE - 2].0.as_str()));
    // A page after any prefix reads the same number of rows, so a deep page costs the
    // same as an early one within 1.5 times (plus a fixed allowance for timer noise).
    assert!(
        deep <= early.mul_f64(1.5) + Duration::from_micros(500),
        "deep page {deep:?} versus early page {early:?}"
    );

    let after = nodes[100].0.as_str();
    let page = read_graph_dump_page(
        &shard,
        "graph",
        DurableCrypto::none(),
        PageCursorRef {
            node_offset: 0,
            edge_offset: 0,
            node_after: Some(after),
            edge_after: None,
            page_size: PAGE,
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        page.nodes.first().map(|(id, _)| id.as_str()),
        Some("n000101")
    );
    drop(shard);
    let _ = std::fs::remove_file(path);
}
