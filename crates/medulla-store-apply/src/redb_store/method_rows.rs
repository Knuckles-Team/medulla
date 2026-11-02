use crate::redb_store::store_cleanup::clear_graph_rows;
use medulla_store::redb_store::store_prelude::*;
use medulla_store::redb_store::*;

use medulla_work_store::redb_store::graph_row_tables::*;
use medulla_work_store::redb_store::work_item;
use medulla_work_store::redb_store::work_item_capability;

pub fn apply_method_rows(
    graph: &str,
    method: &Method,
    tables: &mut GraphRowTables<'_>,
    crypto: DurableCrypto<'_>,
) -> Result<(), String> {
    let mut tables = GraphRowTablesRef {
        nodes: &mut tables.nodes,
        edges: &mut tables.edges,
        ledger: &mut tables.ledger,
        semantic: &mut tables.semantic,
        native_work_items: &mut tables.native_work_items,
    };
    apply_method_rows_ref(graph, method, &mut tables, crypto)
}

pub fn apply_method_rows_ref(
    graph: &str,
    method: &Method,
    tables: &mut GraphRowTablesRef<'_, '_, '_, '_, '_, '_>,
    crypto: DurableCrypto<'_>,
) -> Result<(), String> {
    let GraphRowTablesRef {
        nodes,
        edges,
        ledger,
        semantic,
        native_work_items,
        ..
    } = tables;
    work_item_capability::validate_generic_method(graph, method, nodes, native_work_items, crypto)?;
    work_item::refuse_generic_native_row_write(graph, method, nodes, crypto)?;
    sealed_rows::refuse_generic_sealed_row_write(graph, method, nodes, crypto)?;
    match method {
        Method::AddNode {
            node_id,
            properties_msgpack,
        } => {
            let blob = crypto.seal(properties_msgpack);
            nodes
                .insert((graph, node_id.as_str()), blob.as_ref())
                .map_err(|e| e.to_string())?;
        }
        Method::RemoveNode { node_id } => {
            remove_durable_node(graph, node_id, nodes, edges, semantic, crypto)?;
        }
        Method::CompareAndSetNodeFields {
            node_id,
            conditions_msgpack,
            updates_msgpack,
        } => {
            apply_cas_node_fields_row(
                graph,
                node_id,
                conditions_msgpack,
                updates_msgpack,
                nodes,
                crypto,
            )?;
        }
        Method::AddEdge {
            source_id,
            target_id,
            properties_msgpack,
        } => {
            apply_add_edge_row(
                graph,
                source_id,
                target_id,
                properties_msgpack,
                nodes,
                edges,
                crypto,
            )?;
        }
        Method::RemoveEdge {
            source_id,
            target_id,
        } => {
            remove_durable_edge_pair(graph, source_id, target_id, edges)?;
        }
        Method::BatchUpdate { operations_msgpack } => {
            apply_batch_rows(graph, operations_msgpack, nodes, edges, semantic, crypto)?;
        }
        // ClearGraph and DeleteGraph share one arm because their row effect is
        // byte-identical.  For DeleteGraph the lifecycle caller performs the
        // native/resource drain guard before entering this row applier; keeping
        // its ordinary graph effect here as well means no low-level path
        // (including cross-modal and checkpoint pending methods) can silently
        // commit a no-op graph delete.
        Method::ClearGraph | Method::DeleteGraph { .. } => {
            clear_graph_rows(graph, nodes, edges, ledger)?;
            semantic.remove(graph).map_err(|error| error.to_string())?;
        }
        Method::AddEmbedding { node_id, embedding } => {
            upsert_durable_embedding(semantic, graph, node_id, embedding, crypto)?;
        }
        Method::MintWorkItemClaimCapability { .. }
        | Method::VerifyWorkItemClaimCapability { .. } => {
            return Err(
                "WorkItem claim capabilities require their native authority operation".to_string(),
            );
        }
        _ => {}
    }
    Ok(())
}
