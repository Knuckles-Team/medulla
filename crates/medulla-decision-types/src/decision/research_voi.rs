//! Value-of-information research budget vocabulary
//! (MEDULLA-DECISION-ENGINE-R705.1).
//!
//! A research run decomposes a question into [`Subtask`]s and walks citation
//! links between sources ([`CitationWalk`]). The family spends a bounded
//! [`VoiBudget`]. Every amount is in micro-units held in `u64`. Language-model
//! calls stay outside medulla; these are the family's candidates and budget
//! parameters only.

use serde::{Deserialize, Serialize};

use medulla_types::contract::BoundedVec;

/// Most subtasks one research plan may carry.
pub const MAX_RESEARCH_SUBTASKS: usize = 32;
/// Most source ids one citation-walk frontier may carry.
pub const MAX_CITATION_FRONTIER: usize = 1024;

/// One research subtask and the budget it may spend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Subtask {
    pub subtask_id: String,
    /// Micro-units this subtask may spend.
    pub budget_units: u64,
}

/// A decomposed question and its total budget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchPlan {
    pub subtasks: BoundedVec<Subtask, MAX_RESEARCH_SUBTASKS>,
    pub budget: VoiBudget,
}

/// What a run has spent, what it may spend, and the value floor below which
/// it stops. All micro-units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiBudget {
    pub spent: u64,
    pub limit: u64,
    pub floor: u64,
}

/// The citation links still to read and how deep the walk may go.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CitationWalk {
    /// Source ids of the unread links.
    pub frontier: BoundedVec<String, MAX_CITATION_FRONTIER>,
    pub depth_limit: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire form is fixed: field names, order and number encoding.
    // spec: MEDULLA-DECISION-ENGINE-R705.1
    #[test]
    fn research_types_round_trip_with_golden_vector() {
        let plan = ResearchPlan {
            subtasks: BoundedVec::new(vec![
                Subtask {
                    subtask_id: "s1".into(),
                    budget_units: 2_000_000,
                },
                Subtask {
                    subtask_id: "s2".into(),
                    budget_units: 500_000,
                },
            ])
            .unwrap(),
            budget: VoiBudget {
                spent: 0,
                limit: 2_500_000,
                floor: 1_000,
            },
        };
        let golden = r#"{"subtasks":[{"subtask_id":"s1","budget_units":2000000},{"subtask_id":"s2","budget_units":500000}],"budget":{"spent":0,"limit":2500000,"floor":1000}}"#;
        assert_eq!(serde_json::to_string(&plan).unwrap(), golden);
        assert_eq!(serde_json::from_str::<ResearchPlan>(golden).unwrap(), plan);
        let bytes = rmp_serde::to_vec_named(&plan).unwrap();
        assert_eq!(rmp_serde::from_slice::<ResearchPlan>(&bytes).unwrap(), plan);

        let walk = CitationWalk {
            frontier: BoundedVec::new(vec!["src:a".into(), "src:b".into()]).unwrap(),
            depth_limit: 3,
        };
        let golden = r#"{"frontier":["src:a","src:b"],"depth_limit":3}"#;
        assert_eq!(serde_json::to_string(&walk).unwrap(), golden);
        assert_eq!(serde_json::from_str::<CitationWalk>(golden).unwrap(), walk);
    }

    #[test]
    fn unknown_fields_and_oversized_lists_are_refused() {
        let unknown = r#"{"spent":0,"limit":1,"floor":0,"extra":1}"#;
        assert!(serde_json::from_str::<VoiBudget>(unknown).is_err());
        let many: Vec<Subtask> = (0..=MAX_RESEARCH_SUBTASKS)
            .map(|i| Subtask {
                subtask_id: format!("s{i}"),
                budget_units: 1,
            })
            .collect();
        assert!(BoundedVec::<Subtask, MAX_RESEARCH_SUBTASKS>::new(many.clone()).is_err());
        let json = serde_json::json!({
            "subtasks": many,
            "budget": {"spent": 0, "limit": 1, "floor": 0},
        });
        assert!(serde_json::from_value::<ResearchPlan>(json).is_err());
    }
}
