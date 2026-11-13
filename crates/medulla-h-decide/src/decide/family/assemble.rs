//! The primitive hooks of `AgentAssemble` (MEDULLA-DECISION-ENGINE-R036.100.3).
//!
//! `AgentAssemble` is a decision surface, not a `QuestionKind`, so its hooks are
//! plain functions this module owns beside the question families rather than a
//! `for_kind` arm. Two checks run, in two places:
//!
//! - the graph-size admission here, before anything is read: a request naming
//!   more edges than the resolved `primitive.max_edges` knob is refused with
//!   `PRIMITIVE_GRAPH_TOO_LARGE`. The knob is read here because the compute
//!   crate reads no configuration;
//! - the edge and replacement checks run inside the pure decision
//!   (`medulla_compute::assemble`), so the same record is re-derived on commit.
//!   A failed check abstains with the matching `PRIMITIVE_*` code and one
//!   eliminated-constraint premise for the option it removed.

use medulla_compute::assemble::primitive::check_graph_size;
use medulla_decision_types::decision::AssemblyRequest;

/// Refuse a request whose primitive graph is over the knob; the wire text is
/// `CODE: detail`, like every assembly refusal.
pub(crate) fn admit_graph_size(request: &AssemblyRequest, max_edges: u64) -> Result<(), String> {
    check_graph_size(request.primitive_edges.len(), max_edges).map_err(|refusal| {
        format!(
            "{}: the request names {} primitive edges, over primitive.max_edges ({max_edges})",
            refusal.code(),
            request.primitive_edges.len()
        )
    })
}
