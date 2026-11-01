//! Wire-decode tests for the analytics job governor. They drive the native transport
//! decoder, which lives in a sibling handler crate, so they run from the facade.

use medulla_agent_types::jobs::{JobKind, JobOp};
use medulla_h_jobs::jobs::govern_job_kind;
use medulla_protocol_types::protocol::Response;
use medulla_server_api::access::CarrierAuthority;

mod mine_associate_wire_tests {
    use super::*;
    use medulla_server_api::auth::VerifiedRequestContext;

    fn authority() -> CarrierAuthority {
        CarrierAuthority::from_verified(&VerifiedRequestContext::from_verified_claims(
            medulla_types::acl::RequestContextClaims {
                principal: "analytics-test".into(),
                agent_id: "analytics-test".into(),
                tenant: "analytics-test-tenant".into(),
                audience: "medulla".into(),
                policy_version: "test".into(),
                ..Default::default()
            },
            "analytics-test-key".into(),
        ))
        .unwrap()
    }

    fn govern(kind: JobKind) -> Result<(JobKind, String, String, serde_json::Value), Response> {
        govern_job_kind(&authority(), 82, "policy:test", "", kind, &mut Vec::new())
    }

    fn mining_kind(support: f64, confidence: f64, algorithm: &str) -> JobKind {
        JobKind::MineAssociate {
            transactions: vec![vec!["eg:item:a".into(), "eg:item:b".into()]],
            min_support: support,
            min_confidence: confidence,
            algorithm: algorithm.into(),
        }
    }


    // spec: MEDULLA-DURABLE-KERNEL-R082.2
    #[test]
    fn mine_associate_defaults_aliases_and_threshold_boundaries_are_accepted() {
        let payload = rmp_serde::to_vec_named(&serde_json::json!({
            "id": 82, "graph": "analytics-test", "auth_token": "", "agent_id": null,
            "method": "AnalyticsJob", "params": {"op": {"Submit": {
                "graph": "analytics-test", "kind": {
                    "MineAssociate": {"transactions": [["eg:item:a", "eg:item:b"]]}
                }
            }}}
        }))
        .unwrap();
        let request = medulla_h_admin::transport::decode_request_payload(&payload, 128)
            .expect("the same envelope must accept valid transactions with defaults");
        let medulla_protocol_types::protocol::Method::AnalyticsJob {
            op: JobOp::Submit(spec),
        } = request.method
        else {
            panic!("expected a decoded Submit operation");
        };
        let kind = spec.kind;
        let (kind, family, algorithm, params) = govern(kind).unwrap();
        assert_eq!(family, "mining.association");
        assert_eq!(algorithm, "fpgrowth");
        assert_eq!(params["min_support"], 0.1);
        assert_eq!(params["min_confidence"], 0.5);
        assert!(matches!(kind, JobKind::MineAssociate { .. }));
        for algorithm in ["apriori", "fpgrowth", "fp-growth", "fp_growth", "eclat"] {
            for (support, confidence) in [(0.0, 0.0), (1.0, 1.0)] {
                govern(mining_kind(support, confidence, algorithm)).unwrap();
            }
        }
    }

    // spec: MEDULLA-DURABLE-KERNEL-R082.2
    #[test]
    fn mine_associate_malformed_transaction_shapes_have_typed_wire_refusals() {
        // Use the actual native transport decoder: malformed shapes must retain
        // their correlation id and return a bounded typed refusal before Submit.
        for transactions in [
            serde_json::json!(null),
            serde_json::json!("not-a-list"),
            serde_json::json!({}),
            serde_json::json!(7),
            serde_json::json!(true),
            serde_json::json!(["not-a-nested-list"]),
            serde_json::json!([null]),
            serde_json::json!([7]),
            serde_json::json!([true]),
            serde_json::json!([{}]),
            serde_json::json!([[7]]),
            serde_json::json!([[null]]),
            serde_json::json!([[true]]),
            serde_json::json!([[{}]]),
            serde_json::json!([[[]]]),
        ] {
            let payload = rmp_serde::to_vec_named(&serde_json::json!({
                "id": 82, "graph": "analytics-test", "auth_token": "", "agent_id": null,
                "method": "AnalyticsJob", "params": {"op": {"Submit": {
                    "graph": "analytics-test", "kind": {
                        "MineAssociate": {"transactions": transactions}
                    }
                }}}
            }))
            .unwrap();
            let response = medulla_h_admin::transport::decode_request_payload(&payload, 128)
                .expect_err("malformed transactions must be refused before submission");
            assert_eq!(response.id, 82);
            assert_eq!(response.error.as_deref(), Some("INVALID_ARGUMENT"));
            assert_eq!(
                response.error_detail.as_deref(),
                Some("invalid request encoding")
            );
            assert!(response.result.is_none());
        }    }
}
