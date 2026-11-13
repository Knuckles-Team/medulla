//! Refusals of the served A2A routing family (MEDULLA-DECISION-ENGINE-R044.2.2).
//!
//! An inbound A2A task names the agent graph it wants. The family routes it to
//! that graph only when the tenant's visible published graphs hold it; the
//! typed refusal says why a request routes nowhere.

use super::record::outcome::{DecisionRefusal, RefusalCode};

/// Why an inbound task routes to no graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum A2aRouteRefusal {
    /// The request names no graph (`a2a.requested_graph_id` absent or empty).
    RequestMissing,
    /// No visible published graph of the tenant has the requested id.
    GraphUnknown { requested: String },
}

/// The `REFUSALS` table of the family, registered in `codes::FAMILY_TABLES`.
pub const REFUSALS: &[RefusalCode] = &[
    RefusalCode::new("A2A_GRAPH_UNKNOWN"),
    RefusalCode::new("A2A_REQUEST_MISSING"),
];

impl DecisionRefusal for A2aRouteRefusal {
    fn code(&self) -> RefusalCode {
        match self {
            Self::GraphUnknown { .. } => REFUSALS[0],
            Self::RequestMissing => REFUSALS[1],
        }
    }

    fn detail(&self) -> String {
        match self {
            Self::GraphUnknown { requested } => {
                format!("no visible published agent graph has the id `{requested}`")
            }
            Self::RequestMissing => "the task names no requested graph id".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DECISION-ENGINE-R044.2.2
    #[test]
    fn every_refusal_has_its_own_registered_code() {
        let unknown = A2aRouteRefusal::GraphUnknown {
            requested: "g".into(),
        };
        assert_eq!(unknown.code().as_str(), "A2A_GRAPH_UNKNOWN");
        assert_eq!(
            A2aRouteRefusal::RequestMissing.code().as_str(),
            "A2A_REQUEST_MISSING"
        );
    }
}
