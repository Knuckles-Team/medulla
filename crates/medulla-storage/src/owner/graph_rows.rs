//! Every graph-scoped owner table of the graph shard, visited with its types.
//!
//! The graph shard declares its tables once, in the registry
//! ([`crate::owner_table_names`] for [`crate::OwnerLayout::GraphShard`]). A table
//! is graph-scoped when the registry gives it `TableScope::Serving`: its key
//! leads with the graph name and a graph owns exactly the rows under that name.
//! A restore of one graph has to carry every one of those tables, and redb's
//! tables are typed, so the one typed enumeration lives here beside the
//! registry and is proved equal to it by `visits_exactly_the_registry_serving_tables`.
//! A graph-scoped table added to the registry without an entry here fails that
//! test.
//!
//! A consumer implements [`GraphTableVisitor`] once, generically over the key
//! and value types, and [`visit_graph_scoped_tables`] calls it for each table.
//! The export of a graph's rows ([`export_graph_table_rows`]) is the read-side
//! consumer; the object-store restore is the write-side one.

use redb::{TableDefinition, TableHandle};

use crate::capability::ScopedRead;
use crate::owner::domain::GraphShardOwner;
use crate::owner::graph_shard as shard;
use crate::owner::row_key::{OwnerRowScope, OwnerRowScopeStart};

/// One graph-scoped owner table's rows of one graph, as stored: the encoded key
/// and the encoded value of each row, in key order. A sealed value stays sealed,
/// so a root restores into a store that holds the key it was sealed with.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TableRows {
    /// The registry name of the table (`owner_table_names` of the graph shard).
    pub table: String,
    pub rows: Vec<(Vec<u8>, Vec<u8>)>,
}

/// The `nodes` and `edges` tables: the row pages carry them in their own
/// unsealed form, so the generic table rows skip them.
pub const PAGED_GRAPH_TABLES: [&str; 2] = ["nodes", "edges"];

/// One graph-scoped table, with the key and value types the registry declares.
pub trait GraphTableVisitor {
    /// Called once per graph-scoped table, in registry order.
    fn visit<K, V>(&mut self, definition: TableDefinition<'static, K, V>) -> Result<(), String>
    where
        K: redb::Key + 'static,
        for<'k> K::SelfType<'k>: OwnerRowScope + OwnerRowScopeStart<'k>,
        V: redb::Value + 'static;
}

/// Visit every graph-scoped table of the graph shard exactly once.
pub fn visit_graph_scoped_tables<V: GraphTableVisitor>(visitor: &mut V) -> Result<(), String> {
    visitor.visit(shard::NODES)?;
    visitor.visit(shard::EDGES)?;
    visitor.visit(shard::LEDGER)?;
    visitor.visit(shard::SEMANTIC_STORE)?;
    visitor.visit(shard::AUDIT_CHAIN)?;
    visitor.visit(shard::PROVENANCE_ANCHOR_MEMBERS)?;
    visitor.visit(shard::WORK_ITEM_COMMAND_SEQUENCE)?;
    visitor.visit(shard::RESOURCE_RESERVATIONS)?;
    visitor.visit(shard::RESOURCE_RESERVATION_TENANT_INDEX)?;
    visitor.visit(shard::RESOURCE_RESERVATION_ATTEMPTS)?;
    visitor.visit(shard::RESOURCE_HOSTS)?;
    visitor.visit(shard::RESOURCE_EXCLUSIVITY)?;
    visitor.visit(shard::RESOURCE_FAIRNESS)?;
    visitor.visit(shard::RESOURCE_CONCURRENCY)?;
    visitor.visit(shard::RESOURCE_ANTI_AFFINITY)?;
    visitor.visit(shard::RESOURCE_DISK_POLICIES)?;
    visitor.visit(shard::CHANGE_ENVELOPES)?;
    visitor.visit(shard::CONTENT_VERSIONS)?;
    visitor.visit(shard::CHANGE_CURSORS)?;
    visitor.visit(shard::CHANGE_BLOBS)?;
    visitor.visit(shard::CHANGE_FEATURES)?;
    visitor.visit(shard::CHANGE_EVIDENCE)?;
    visitor.visit(shard::CHANGE_POLICIES)?;
    visitor.visit(shard::CHANGE_LINEAGE)?;
    visitor.visit(shard::CAPACITY_CELLS)?;
    visitor.visit(shard::CAPACITY_LEASES)?;
    visitor.visit(shard::CAPACITY_USAGE)?;
    visitor.visit(shard::CAPACITY_IDEMPOTENCY)?;
    visitor.visit(shard::REPOSITORY_ENRICHMENT_BUDGETS)?;
    visitor.visit(shard::REPOSITORY_ENRICHMENT_POLICY_REVISIONS)?;
    visitor.visit(shard::REPOSITORY_ENRICHMENT_SUPERSESSIONS)?;
    visitor.visit(shard::REPOSITORY_ENRICHMENT_PARKS)?;
    visitor.visit(shard::AUDIT_REQUESTS)?;
    visitor.visit(shard::CURRENT_PLANS)?;
    visitor.visit(shard::RUN_CHECKPOINTS)?;
    visitor.visit(shard::EFFECT_RECEIPTS)?;
    visitor.visit(shard::ADMITTED_REQUESTS)?;
    visitor.visit(shard::WRITE_DAYS)?;
    visitor.visit(shard::GRAPH_CREATOR)?;
    visitor.visit(shard::WORK_ITEM_TRANSITIONS)?;
    visitor.visit(shard::WORK_ITEM_CLAIM_CAPABILITIES)?;
    visitor.visit(shard::WORK_ITEM_CLAIM_CAPABILITY_INVOCATIONS)?;
    visitor.visit(shard::NATIVE_WORK_ITEM_AUTHORITY)?;
    visitor.visit(shard::DEVELOPMENT_LANE_HOLDS)?;
    visitor.visit(shard::DEVELOPMENT_LANE_TENANT_INDEX)?;
    visitor.visit(shard::DEVELOPMENT_LANE_LANE_INDEX)?;
    visitor.visit(shard::DEVELOPMENT_LANE_REPOSITORY_BRANCH_INDEX)?;
    visitor.visit(shard::DEVELOPMENT_LANE_WORKTREE_INDEX)?;
    visitor.visit(shard::DEVELOPMENT_LANE_WORK_ITEM_INDEX)?;
    visitor.visit(shard::DEVELOPMENT_LANE_COUNTERS)?;
    visitor.visit(shard::DEVELOPMENT_LANE_PRESSURE_INDEX)?;
    visitor.visit(shard::DEVELOPMENT_LANE_POLICIES)?;
    visitor.visit(shard::DEVELOPMENT_LANE_INVOCATIONS)?;
    Ok(())
}

struct Export<'r, 'a> {
    read: &'r ScopedRead<'a, GraphShardOwner>,
    tables: Vec<TableRows>,
}

impl GraphTableVisitor for Export<'_, '_> {
    fn visit<K, V>(&mut self, definition: TableDefinition<'static, K, V>) -> Result<(), String>
    where
        K: redb::Key + 'static,
        for<'k> K::SelfType<'k>: OwnerRowScope + OwnerRowScopeStart<'k>,
        V: redb::Value + 'static,
    {
        let name = definition.name().to_string();
        if PAGED_GRAPH_TABLES.contains(&name.as_str()) {
            return Ok(());
        }
        let table = self.read.scoped_owner_table(definition)?;
        let mut rows = Vec::new();
        for row in table.scope_rows()? {
            let (key, value) = row?;
            rows.push((
                K::as_bytes(&key.value()).as_ref().to_vec(),
                V::as_bytes(&value.value()).as_ref().to_vec(),
            ));
        }
        if !rows.is_empty() {
            self.tables.push(TableRows { table: name, rows });
        }
        Ok(())
    }
}

/// Every row of this read's graph in every graph-scoped table except `nodes`
/// and `edges`, as stored (a sealed value stays sealed), in registry order. A
/// table with no row of the graph is absent.
pub fn export_graph_table_rows(
    read: &ScopedRead<'_, GraphShardOwner>,
) -> Result<Vec<TableRows>, String> {
    let mut export = Export {
        read,
        tables: Vec::new(),
    };
    visit_graph_scoped_tables(&mut export)?;
    Ok(export.tables)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physical::manifest::TableScope;

    struct Names(Vec<String>);

    impl GraphTableVisitor for Names {
        fn visit<K, V>(&mut self, definition: TableDefinition<'static, K, V>) -> Result<(), String>
        where
            K: redb::Key + 'static,
            for<'k> K::SelfType<'k>: OwnerRowScope + OwnerRowScopeStart<'k>,
            V: redb::Value + 'static,
        {
            self.0.push(definition.name().to_string());
            Ok(())
        }
    }

    #[test]
    fn visits_exactly_the_registry_serving_tables() {
        let mut visited = Names(Vec::new());
        visit_graph_scoped_tables(&mut visited).unwrap();
        let mut expected: Vec<String> =
            crate::owner::registry::owner_table_names(crate::OwnerLayout::GraphShard)
                .iter()
                .filter(|name| shard::scope(name) == Some(TableScope::Serving))
                .map(|name| name.to_string())
                .collect();
        visited.0.sort();
        expected.sort();
        assert_eq!(visited.0, expected);
    }
}
