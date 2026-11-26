// MEDULLA-ORCH-VOCAB-R810: a run-bound visual observation is stored through the served
// `PutObservation`, read back through the served graph read, and refused for another tenant, an
// unknown run, a malformed envelope and an id stored with another content digest.

#[cfg(all(unix, feature = "redb", feature = "blob", feature = "shacl", feature = "security"))]
mod served {
    use medulla_protocol_types::protocol::Method;
    use medulla_work_types::orchestration::visual_observation::{
        VisualObservation, VisualObservationReceipt, VISUAL_OBSERVATION_NODE_TYPE,
    };
    use serde_json::json;

    use super::super::connector_pack_harness::{next_id, ok, Served, OTHER_TENANT, TENANT};

    const GRAPH: &str = "visual-observation";
    const RUN: &str = "run-visual-1";

    fn observation(tenant: &str, run: &str, id: &str, digest: &str) -> VisualObservation {
        VisualObservation {
            observation_id: id.into(),
            modality: "screenshot".into(),
            source_operation_id: "browser.capture".into(),
            frame_id: Some("frame-1".into()),
            sequence: None,
            captured_at: "2026-11-26T10:00:00Z".into(),
            content_digest: digest.repeat(32),
            content_ref: format!("blob://observations/{id}"),
            tenant_id: tenant.into(),
            run_id: run.into(),
            request_id: "req-1".into(),
            schema_revision: 1,
            retention: None,
            invalidation: None,
        }
    }

    async fn put(served: &Served, tenant: &str, record: VisualObservation) -> medulla_protocol_types::protocol::Response {
        let method = Method::PutObservation { observation: record };
        if tenant == TENANT {
            served.call_on(GRAPH, &format!("observation:{}", next_id()), method).await
        } else {
            served.verified(tenant, &["*"], method).await
        }
    }

    fn refusal(response: &medulla_protocol_types::protocol::Response) -> String {
        response.error.clone().unwrap_or_default()
    }

    // spec: MEDULLA-ORCH-VOCAB-R810
    #[tokio::test]
    async fn observation_round_trip_and_scope() {
        let served = Served::provisioned(GRAPH, "observation-source", &[("run", "RunTrace")]).await;
        let run = served
            .call_on(
                GRAPH,
                &format!("observation:{}", next_id()),
                Method::AddNode {
                    node_id: RUN.into(),
                    properties_msgpack: rmp_serde::to_vec_named(
                        &json!({"type": "RunTrace", "name": RUN}),
                    )
                    .unwrap(),
                },
            )
            .await;
        assert!(run.error.is_none(), "AddNode refused: {run:?}");

        // Store, then read the node back through the served graph read.
        let record = observation(TENANT, RUN, "obs-1", "ab");
        let receipt: VisualObservationReceipt =
            ok("PutObservation", put(&served, TENANT, record.clone()).await);
        assert!(!receipt.replayed);
        assert_eq!(receipt.node_id, record.node_id());
        let read = served
            .call_on(
                GRAPH,
                &format!("observation:{}", next_id()),
                Method::GetNodeProperties {
                    node_id: receipt.node_id.clone(),
                },
            )
            .await;
        let node: serde_json::Value = ok("GetNodeProperties", read);
        assert_eq!(node["type"], VISUAL_OBSERVATION_NODE_TYPE);
        assert_eq!(node["run_id"], RUN);
        assert_eq!(node["content_digest"], record.content_digest.as_str());

        // The same record again is a replay.
        let again: VisualObservationReceipt =
            ok("PutObservation", put(&served, TENANT, record.clone()).await);
        assert!(again.replayed);

        // An id stored with another content digest refuses.
        let other = observation(TENANT, RUN, "obs-1", "cd");
        assert!(refusal(&put(&served, TENANT, other).await).starts_with("OBSERVATION_CONFLICT"));

        // An unknown run and another tenant's record read the same.
        let unknown = refusal(&put(&served, TENANT, observation(TENANT, "no-run", "obs-2", "ab")).await);
        assert!(unknown.starts_with("OBSERVATION_SCOPE_UNKNOWN"), "{unknown}");
        let foreign = refusal(
            &put(&served, TENANT, observation(OTHER_TENANT, RUN, "obs-3", "ab")).await,
        );
        assert_eq!(foreign, unknown, "no existence oracle");
        let by_other = refusal(
            &put(&served, OTHER_TENANT, observation(OTHER_TENANT, RUN, "obs-4", "ab")).await,
        );
        assert!(by_other.starts_with("OBSERVATION_SCOPE_UNKNOWN"), "{by_other}");

        // A malformed envelope refuses before any scope check.
        let mut bad = observation(TENANT, RUN, "obs-5", "ab");
        bad.content_digest = "short".into();
        assert!(refusal(&put(&served, TENANT, bad).await).starts_with("OBSERVATION_INVALID"));
    }
}
