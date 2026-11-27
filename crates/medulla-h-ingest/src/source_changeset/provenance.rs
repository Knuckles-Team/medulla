//! Agent principals and run provenance of `SubmitChangeset` (MEDULLA-SCM-R012).
//!
//! A request is an agent request when its verified agent differs from its
//! principal, that is, when it carries a delegation chain. An agent request
//! must name the run that produced the change, and a named run must be a
//! `run_trace` node the caller can read in the repository graph. Both checks
//! run in stage one, after `validate` and before `verify_submission`, and write
//! nothing. The canonical binding is the sealed `Changeset` record; the
//! `producedBy` edge is derived from it by MEDULLA-SDLC-R001.3.

use medulla_source_types::source_changeset::{ChangesetRefusal, SubmitRefusal};

use super::AcceptError;

/// The `kind` property value of a run-trace node.
pub const RUN_TRACE_KIND: &str = "run_trace";

/// The caller's view of the repository graph, as the run-trace check reads it.
pub trait RunTraceLookup {
    /// The `kind` property of the node `node_id`, or `None` when the node is
    /// absent or the caller may not see it (the two are indistinguishable by
    /// design).
    fn node_kind(&self, node_id: &str) -> Result<Option<String>, String>;
}

/// Check the provenance of one submission.
///
/// `agent_identity` is `Some` for an agent request. An agent request without
/// `run_trace_id` refuses `AGENT_RUN_TRACE_MISSING`. A present `run_trace_id`
/// that names no visible node of kind `run_trace` refuses `RUN_TRACE_UNKNOWN`:
/// an absent node, another tenant's node and a node of another kind refuse
/// alike. A human request may omit the run.
pub fn check(
    agent_identity: Option<&str>,
    run_trace_id: Option<&str>,
    lookup: &dyn RunTraceLookup,
) -> Result<(), AcceptError> {
    let Some(run) = run_trace_id else {
        return match agent_identity {
            Some(_) => Err(AcceptError::Refused(SubmitRefusal::new(
                ChangesetRefusal::AgentRunTraceMissing,
                "an agent request must name the run that produced the change",
            ))),
            None => Ok(()),
        };
    };
    match lookup.node_kind(run).map_err(AcceptError::Store)? {
        Some(kind) if kind == RUN_TRACE_KIND => Ok(()),
        _ => Err(AcceptError::Refused(SubmitRefusal::new(
            ChangesetRefusal::RunTraceUnknown,
            "run_trace_id names no run_trace node the caller can read",
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct Graph(HashMap<&'static str, &'static str>);

    impl RunTraceLookup for Graph {
        fn node_kind(&self, node_id: &str) -> Result<Option<String>, String> {
            Ok(self.0.get(node_id).map(|kind| kind.to_string()))
        }
    }

    fn refusal(outcome: Result<(), AcceptError>) -> ChangesetRefusal {
        match outcome {
            Err(AcceptError::Refused(refusal)) => refusal.refusal,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    // spec: MEDULLA-SCM-R012
    #[test]
    fn agent_needs_a_visible_run_trace() {
        let graph = Graph(HashMap::from([
            ("run-1", "run_trace"),
            ("doc-1", "document"),
        ]));
        // A human may omit the run; a named run must still resolve.
        assert!(check(None, None, &graph).is_ok());
        assert!(check(None, Some("run-1"), &graph).is_ok());
        assert_eq!(
            refusal(check(None, Some("absent"), &graph)),
            ChangesetRefusal::RunTraceUnknown
        );
        // An agent must name the run, and it must be a visible run_trace node.
        assert!(check(Some("agent"), Some("run-1"), &graph).is_ok());
        assert_eq!(
            refusal(check(Some("agent"), None, &graph)),
            ChangesetRefusal::AgentRunTraceMissing
        );
        assert_eq!(
            refusal(check(Some("agent"), Some("absent"), &graph)),
            ChangesetRefusal::RunTraceUnknown
        );
        assert_eq!(
            refusal(check(Some("agent"), Some("doc-1"), &graph)),
            ChangesetRefusal::RunTraceUnknown
        );
    }
}
