//! The graph snapshot set a replay re-reads, split out so the support module reaches it.

/// The graph rows a record names, each graph as it stood at the version the
/// rows were read at, keyed by `(graph, graph_version)`. Only the rows the
/// record names are guaranteed to be present, and only when the caller can see
/// them in the image they had at that version.
pub type GraphSnapshots =
    std::collections::BTreeMap<(String, u64), medulla_graph_core::graph::GraphView>;
