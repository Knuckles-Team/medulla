//! The `Discover` answer envelope (MEDULLA-FEDERATED-QUERY-R089.1). It lives here, not beside
//! `DiscoverHit`, because its verdict is a query wire type and medulla-types sits below this crate.

use medulla_types::ingestion_wire::{DiscoverHit, DiscoverPlan};
use serde::{Deserialize, Serialize};

use crate::wire::RetrievalVerdict;

/// The `Discover` answer envelope (R089.1): the hits, the quality verdict and the plan.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DiscoverResult {
    pub hits: Vec<DiscoverHit>,
    pub verdict: RetrievalVerdict,
    #[serde(default)]
    pub plan: DiscoverPlan,
}

#[cfg(test)]
mod discover_plan_tests {
    use super::*;
    use crate::wire::{DiscoverThresholds, RetrievalVerdict};
    use medulla_types::ingestion_wire::{
        ArmRank, DiscoverAnalysis, DiscoverArm, DiscoverDegraded, DiscoverHit, DiscoverPlan, DiscoverPlanRequest,
        ExpansionEvidence, QueryFilters, QUERY_TEXT_TOO_LONG,
    };

    fn hit() -> DiscoverHit {
        DiscoverHit {
            id: "n1".into(),
            name: "n".into(),
            description: "d".into(),
            node_type: "doc".into(),
            score: 0.5,
            hop_path: vec!["n0".into(), "n1".into()],
            arm_ranks: vec![
                ArmRank {
                    arm: DiscoverArm::Semantic,
                    rank: 1,
                },
                ArmRank {
                    arm: DiscoverArm::Expansion,
                    rank: 3,
                },
            ],
            recency_boost: 1.25,
            url: None,
            retrieved_at: None,
            backend: None,
            query_digest: None,
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R090.1
    #[test]
    fn discover_plan_round_trips() {
        let result = DiscoverResult {
            hits: vec![hit()],
            verdict: RetrievalVerdict {
                gate_passed: true,
                failure_modes: vec![],
                composite_quality_q16: 40_000,
                index_population_ratio_q16: 65_535,
                above_threshold: true,
                total_candidates: 1,
                policy_revision: "p".into(),
                index_generation: 3,
                thresholds: DiscoverThresholds::default(),
                adapter_digest: None,
                shard_generations: vec![],
            },
            plan: DiscoverPlan {
                expansions: vec![ExpansionEvidence {
                    digest: "0".repeat(64),
                    kind: "sub_query".into(),
                    provider_id: "m".into(),
                    provider_revision: "r".into(),
                }],
                degraded: Some(DiscoverDegraded::ProviderFailed {
                    message: "down".into(),
                }),
                analysis: Some(DiscoverAnalysis {
                    filters: QueryFilters {
                        types: vec!["Doc".into()],
                        ..QueryFilters::default()
                    },
                    intent: None,
                }),
            },
        };
        let bytes = rmp_serde::to_vec_named(&result).expect("encode");
        let back: DiscoverResult = rmp_serde::from_slice(&bytes).expect("decode");
        assert_eq!(back, result);
        let seed_only: DiscoverHit = serde_json::from_value(serde_json::json!({
            "id": "a", "name": "a", "description": "", "type": "doc", "score": 1.0
        }))
        .expect("every new field defaults");
        assert_eq!(seed_only.recency_boost, 1.0);
        assert!(seed_only.hop_path.is_empty() && seed_only.arm_ranks.is_empty());

        let ok = DiscoverPlanRequest {
            query_text: Some("x".repeat(2048)),
            hops: 2,
            ..Default::default()
        };
        assert_eq!(ok.validate(), Ok(()));
        let deep_hops = DiscoverPlanRequest {
            hops: 3,
            ..Default::default()
        };
        assert_eq!(deep_hops.validate().unwrap_err().code(), "INVALID_ARGUMENT");
        let long = DiscoverPlanRequest {
            query_text: Some("x".repeat(2049)),
            ..Default::default()
        };
        assert_eq!(long.validate().unwrap_err().code(), QUERY_TEXT_TOO_LONG);
        let before_epoch = DiscoverPlanRequest {
            as_of: Some(-1),
            ..Default::default()
        };
        assert_eq!(
            before_epoch.validate().unwrap_err().code(),
            "INVALID_ARGUMENT"
        );
    }
}
