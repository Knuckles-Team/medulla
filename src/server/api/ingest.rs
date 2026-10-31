//! Ingest, graph, jobs and compute handler seams (`IngestApi`, `GraphApi`, `JobsApi`, `ComputeApi`).

/// Ingest handler behaviour reachable from crates below `medulla-h-ingest`.
pub trait IngestApi: Send + Sync {}

/// Graph handler behaviour reachable from crates below `medulla-h-graph`.
pub trait GraphApi: Send + Sync {}

/// Jobs handler behaviour reachable from crates below `medulla-h-jobs`.
pub trait JobsApi: Send + Sync {}

/// Compute handler behaviour reachable from crates below `medulla-h-compute`.
pub trait ComputeApi: Send + Sync {}
