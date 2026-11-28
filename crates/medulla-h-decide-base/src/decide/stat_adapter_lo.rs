//! The adapter pointer keys of the served adapter, split out so lower crates reach them.

use medulla_decision_types::decision::statistical::retrieval_adapter::AdapterScope;

/// The pointer key of a graph's adapter.
pub fn adapter_pointer(graph: &str) -> String {
    format!("adapter:{graph}")
}

/// The pointer key of one scope of a graph's adapter. The graph scope is the
/// graph's own pointer.
pub fn scoped_adapter_pointer(graph: &str, scope: &AdapterScope) -> String {
    match scope {
        AdapterScope::Graph => adapter_pointer(graph),
        other => format!("adapter:{graph}:{}", other.key()),
    }
}
