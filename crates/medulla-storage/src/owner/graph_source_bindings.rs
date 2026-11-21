//! The graph-sourced semantic binding registry owner table (MEDULLA-MEDIA-VOCAB-R001.102.2).
//!
//! One row per graph-sourced binding, keyed `(tenant, binding_id)`, valued with the JSON of the
//! binding's `GraphTextPropertyRef` selector. The bound-source registry the graph commit
//! chokepoint reads is rebuilt from this table at startup. `medulla-storage` declares the table;
//! the store in `medulla-persistence` only names it.

use redb::TableDefinition;

/// `(tenant, binding_id) -> json(GraphTextPropertyRef)`.
pub const GRAPH_SOURCE_BINDINGS: TableDefinition<'static, (&str, &str), &[u8]> =
    TableDefinition::new("graph_source_bindings");
