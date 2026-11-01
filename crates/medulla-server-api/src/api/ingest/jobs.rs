//! Jobs-handler helpers shared by crates below `medulla-h-jobs`.

use medulla_agent_types::delegation::KgDelegateRequest;

/// The provenance references a delegation work item retains.
pub fn provenance_refs(request: &KgDelegateRequest) -> Vec<String> {
    vec![
        format!("delegation:{}", request.delegation_id),
        format!("run:{}", request.run_id),
        format!("trace:{}", request.trace_id),
        request.target.provenance_ref(),
    ]
}
