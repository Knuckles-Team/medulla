//! The object-store restore of one graph carries EVERY graph-scoped owner table
//! (MEDULLA-OBJECT-STORAGE-BACKEND-R002).

use std::collections::BTreeMap;

use medulla_protocol_types::protocol::{GraphType, Method};
use medulla_storage::{
    export_graph_table_rows, owner_row_key, owner_table_names, GraphTableVisitor, OwnerLayout,
    OwnerRowScope, OwnerRowScopeStart, RowKey, PAGED_GRAPH_TABLES,
};
use medulla_store::redb_store::shard::{DrainCommit, Shard, ShardWrite};
use medulla_store::redb_store::store_read::{read_mutation_graph_version, write_graph_meta};
use medulla_store::redb_store::DurableCrypto;
use medulla_store_apply::redb_store::checkpoint::restore_graph_rows;
use medulla_store_apply::redb_store::dump::{
    read_graph_dump_page, read_graph_table_rows, PageCursorRef,
};
use medulla_store_apply::redb_store::store_control::commit_ops;

const GRAPH: &str = "restore-graph";

/// Writes the least row the graph can own into every graph-scoped table but
/// `nodes` and `edges`, so each table of the registry holds a row to carry.
struct SeedFloorRows<'m, 'g> {
    member: &'m medulla_transaction::AdmittedOwnerWrite<'g, medulla_storage::GraphShardOwner>,
}

impl GraphTableVisitor for SeedFloorRows<'_, '_> {
    fn visit<K, V>(
        &mut self,
        definition: redb::TableDefinition<'static, K, V>,
    ) -> Result<(), String>
    where
        K: redb::Key + 'static,
        for<'k> K::SelfType<'k>: OwnerRowScope + OwnerRowScopeStart<'k>,
        V: redb::Value + 'static,
    {
        let name = redb::TableHandle::name(&definition).to_string();
        if PAGED_GRAPH_TABLES.contains(&name.as_str()) {
            return Ok(());
        }
        let mut table = self.member.open_scoped_table(definition)?;
        let key: K::SelfType<'_> = OwnerRowScopeStart::scope_start(GRAPH);
        let value = match V::fixed_width() {
            Some(width) => vec![1_u8; width],
            None => b"seeded".to_vec(),
        };
        table.insert(key, V::from_bytes(&value))
    }
}

/// Every row of the graph, per table, as the root would carry it: the node and
/// edge rows as the dump pages read them and every other table as exported.
fn graph_rows(shard: &Shard) -> BTreeMap<String, Vec<(Vec<u8>, Vec<u8>)>> {
    let mut rows = BTreeMap::new();
    for table in read_graph_table_rows(shard, GRAPH).unwrap() {
        rows.insert(table.table, table.rows);
    }
    let page = read_graph_dump_page(
        shard,
        GRAPH,
        DurableCrypto::none(),
        PageCursorRef {
            node_offset: 0,
            edge_offset: 0,
            node_after: None,
            edge_after: None,
            page_size: 10_000,
        },
    )
    .unwrap()
    .unwrap();
    assert!(page.nodes_exhausted && page.edges_exhausted);
    rows.insert(
        "nodes".to_string(),
        page.nodes
            .into_iter()
            .map(|(id, blob)| (id.into_bytes(), blob))
            .collect(),
    );
    rows.insert(
        "edges".to_string(),
        page.edges
            .into_iter()
            .map(|(source, target, blob)| (format!("{source}\u{0}{target}").into_bytes(), blob))
            .collect(),
    );
    rows
}

// spec: MEDULLA-OBJECT-STORAGE-BACKEND-R002
#[test]
fn a_restore_round_trip_carries_every_graph_scoped_table() {
    let source_path = medulla_store::redb_store::temp_path("medulla-restore", "source");
    let source = Shard::open(&source_path).unwrap();
    write_graph_meta(&source, GRAPH, GRAPH, GraphType::Global).unwrap();

    // Real rows through the served drain path: two nodes and an edge.
    let node = |id: &str| Method::AddNode {
        node_id: id.to_string(),
        properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({"type": "Doc", "id": id}))
            .unwrap(),
    };
    let mut ops = vec![
        (GRAPH.to_string(), node("a")),
        (GRAPH.to_string(), node("b")),
        (
            GRAPH.to_string(),
            Method::AddEdge {
                source_id: "a".to_string(),
                target_id: "b".to_string(),
                properties_msgpack: rmp_serde::to_vec_named(
                    &serde_json::json!({"relationship": "links"}),
                )
                .unwrap(),
            },
        ),
    ];
    #[cfg(feature = "security")]
    let mut audit = medulla_store::redb_store::AuditTailCache::new();
    commit_ops(
        &source,
        &mut ops,
        &mut Vec::new(),
        &mut Vec::new(),
        "restore-seed-rows",
        1,
        DurableCrypto::none(),
        #[cfg(feature = "security")]
        &mut audit,
    )
    .unwrap();

    // One row in every other graph-scoped table, in one maintenance write.
    let members = source.graph_members(&[GRAPH]).unwrap();
    let (group, batches) = source
        .admit_maintenance(&members, "restore-seed-tables")
        .unwrap();
    let write = ShardWrite::open(&source, &group, &members, &batches).unwrap();
    let seeded = (|| {
        let member = write.graph(GRAPH)?;
        medulla_storage::visit_graph_scoped_tables(&mut SeedFloorRows { member })
    })();
    let finished = write.finish();
    seeded.unwrap();
    finished.unwrap();
    source
        .commit_drain_with(group, &batches, 2, DrainCommit::default())
        .unwrap();

    // The test is only as strong as its coverage: every graph-scoped table of the
    // registry holds a row of the graph before the round trip.
    let before = graph_rows(&source);
    let registry: Vec<&str> = owner_table_names(OwnerLayout::GraphShard)
        .iter()
        .copied()
        .filter(|name| owner_row_key(name, OwnerLayout::GraphShard) == RowKey::ScopePrefixed)
        .collect();
    assert!(registry.len() >= 50, "{registry:?}");
    for name in &registry {
        assert!(
            before.get(*name).is_some_and(|rows| !rows.is_empty()),
            "graph-scoped table {name} holds no row of the graph before the round trip"
        );
    }
    assert_eq!(
        before.len(),
        registry.len(),
        "{:?}",
        before.keys().collect::<Vec<_>>()
    );

    // Restore into an empty store from exactly what the snapshot carries.
    let version = read_mutation_graph_version(&source, GRAPH).unwrap();
    let tables =
        export_graph_table_rows(&source.read(&source.graph(GRAPH).unwrap()).unwrap()).unwrap();
    let page = read_graph_dump_page(
        &source,
        GRAPH,
        DurableCrypto::none(),
        PageCursorRef {
            node_offset: 0,
            edge_offset: 0,
            node_after: None,
            edge_after: None,
            page_size: 10_000,
        },
    )
    .unwrap()
    .unwrap();
    let target_path = medulla_store::redb_store::temp_path("medulla-restore", "target");
    let target = Shard::open(&target_path).unwrap();
    write_graph_meta(&target, GRAPH, GRAPH, GraphType::Global).unwrap();
    restore_graph_rows(
        &target,
        GRAPH,
        &page.nodes,
        &page.edges,
        &tables,
        version,
        DurableCrypto::none(),
    )
    .unwrap();

    assert_eq!(
        read_mutation_graph_version(&target, GRAPH).unwrap(),
        version
    );
    let after = graph_rows(&target);
    for name in &registry {
        assert_eq!(
            after.get(*name),
            before.get(*name),
            "graph-scoped table {name} differs after the restore"
        );
    }
    assert_eq!(after, before);
    drop((source, target));
    let _ = std::fs::remove_file(source_path);
    let _ = std::fs::remove_file(target_path);
}
