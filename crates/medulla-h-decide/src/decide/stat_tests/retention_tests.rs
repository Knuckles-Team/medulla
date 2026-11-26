//! R066 acceptance uses served decisions and the real persistent retention path.
use super::*;

/// One day in milliseconds; the default graph-summary expiry is 90 of them.
#[cfg(feature = "blob")]
const DAY_MS: u64 = 86_400_000;

async fn library_record(h: &Harness) -> StatisticalDecisionRecord {
    h.publish_test_component(
        "retained-tool",
        AgentComponentKind::Tool,
        "web search",
        None::<&()>,
        None,
    )
    .unwrap();
    let (schema, _) = publish_route_schema(h);
    decide_and_assert_abstains(h, &schema, None)
        .await
        .records
        .as_slice()[0]
        .clone()
}

#[cfg(feature = "blob")]
async fn commit_record(h: &Harness, record: StatisticalDecisionRecord) -> DecisionLogEntry {
    let id = record.record_id.clone();
    let _: DecisionLogCommitted = decode(
        log_op(
            h,
            "decider",
            DecisionLogOp::Commit {
                record: Box::new(record),
                evaluator: None,
                approval: None,
            },
        )
        .await,
    )
    .unwrap();
    log_tests::log_entry(h, &id)
}

#[cfg(feature = "blob")]
async fn compact_at(
    h: &Harness,
    policy: &ComponentDependency,
    now_ms: u64,
) -> medulla_decision_types::decision::statistical::log::DecisionLogCompacted {
    let state = h.state.read().await;
    let retention = super::super::stat_retention::Retention::of(&state, &verified());
    let ctx = super::super::stat_executor::ExecutionContext {
        store: &h.store,
        tenant_id: TENANT,
        now_ms,
        server_secret: state.auth_secret.as_bytes(),
        matched: &[],
        designation: None,
        reader: None,
        assistant_profile: None,
        observation_windows: None,
        type_digest: None,
        recommend_catalog: None,
    };
    super::super::stat_retention::compact(
        &ctx,
        &retention,
        &DecisionPolicyRef::Pinned {
            component: policy.clone(),
        },
        16,
    )
    .unwrap()
}

// spec: MEDULLA-DECISION-ENGINE-R066.1
#[cfg(feature = "blob")]
#[tokio::test]
async fn library_records_survive_every_retention_window() {
    let h = Harness::new().await;
    let record = library_record(&h).await;
    assert!(matches!(
        record.candidate_source,
        medulla_decision_types::decision::CandidateSourceRecord::AgentLibrary { .. }
    ));
    let entry = commit_record(&h, record).await;
    let before = serde_json::to_vec(&entry).unwrap();
    let policy = h.publish_policy("retain-library", &log_tests::retention_policy(0, Some(0)));
    for now in [entry.committed_at_ms, u64::MAX, u64::MAX] {
        let changed = compact_at(&h, &policy, now).await;
        assert_eq!((changed.compacted, changed.retired), (0, 0));
        assert_eq!(
            serde_json::to_vec(&log_tests::log_entry(&h, &entry.record.record_id)).unwrap(),
            before
        );
    }
    assert!(matches!(
        log_tests::verify_log(&h, &entry.record.record_id)
            .await
            .unwrap(),
        medulla_decision_types::decision::statistical::log::DecisionLogVerification::Verified { .. }
    ));
}

// spec: MEDULLA-DECISION-ENGINE-R066.2
#[cfg(all(feature = "blob", feature = "query"))]
#[tokio::test]
async fn graph_inputs_expire_at_the_configured_boundaries() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let record = graph_log_record(&h).await;
    let entry = commit_record(&h, record).await;
    let policy = h.publish_policy(
        "expire-graph-inputs",
        &log_tests::retention_policy(100, Some(200)),
    );
    let start = entry.committed_at_ms;
    for (now, expected) in [
        (start + 99, (0, 0)),
        (start + 100, (1, 0)),
        (start + 299, (0, 0)),
        (start + 300, (0, 1)),
        (start + 300 + 90 * DAY_MS - 1, (0, 0)),
    ] {
        let changed = compact_at(&h, &policy, now).await;
        assert_eq!((changed.compacted, changed.retired), expected);
        assert_eq!(
            log_tests::log_entry(&h, &entry.record.record_id)
                .record
                .record_digest,
            entry.record.record_digest
        );
    }
    assert!(matches!(
        log_tests::verify_log(&h, &entry.record.record_id)
            .await
            .unwrap(),
        medulla_decision_types::decision::statistical::log::DecisionLogVerification::InputsRetired { .. }
    ));
}

// spec: MEDULLA-DECISION-ENGINE-R066.3
#[tokio::test]
async fn evaluate_only_leaves_no_committed_records() {
    let h = Harness::new().await;
    let record = library_record(&h).await;
    let schema = record.inputs.feature_schema;
    for _ in 0..16 {
        let batch = decide_and_assert_abstains(&h, &schema, None).await;
        for record in batch.records.as_slice() {
            assert!(h
                .store
                .decision_record_body(TENANT, &record.record_id)
                .unwrap()
                .is_none());
            assert!(h
                .store
                .current_component(TENANT, &record.record_id)
                .unwrap()
                .is_none());
        }
    }
    assert!(h
        .store
        .decision_artifacts_with_prefix(TENANT, "record:", 100)
        .unwrap()
        .is_empty());
}

#[cfg(all(feature = "blob", feature = "query"))]
fn joined_evaluation(h: &Harness, record: &StatisticalDecisionRecord, tenant: &str) -> String {
    let evaluation = DecisionOutcomeEvaluation {
        record_id: record.record_id.clone(),
        evaluation_id: "evaluation-expiry".to_string(),
        class: EvidenceClass::Observation,
        selected_agent: "agent-a".to_string(),
        lease_holder: "worker-a".to_string(),
        fidelity: OutcomeFidelity::ToolCalls,
        success: Some(true),
        backbone: None,
    };
    let key = medulla_persistence_records::decision_jobs::evaluation_key(
        &record.record_id,
        &evaluation.evaluation_id,
    );
    let stored = StoredEvaluation {
        evaluation,
        producer: "evaluator".to_string(),
        recorded_at_ms: 1,
    };
    h.store
        .put_decision_artifacts(
            tenant,
            &[(
                key.clone(),
                medulla_persistence_records::decision_jobs::encode_artifact(&stored).unwrap(),
            )],
        )
        .unwrap();
    key
}

// spec: MEDULLA-DECISION-ENGINE-R066.4
#[cfg(all(feature = "blob", feature = "query"))]
#[tokio::test]
async fn graph_summaries_expire_on_policy_window() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let graph = graph_log_record(&h).await;
    let entry = commit_record(&h, graph.clone()).await;
    let graph_id = entry.record.record_id.clone();
    let evaluation_key = joined_evaluation(&h, &graph, TENANT);
    let library = library_record(&h).await;
    let library_entry = commit_record(&h, library).await;
    let library_before = serde_json::to_vec(&library_entry).unwrap();

    // A graph summary of another tenant, already retired long ago, under the
    // same policy window: one tenant's compaction never touches it.
    let foreign_key = medulla_persistence_records::decision_jobs::record_key(&graph_id);
    let mut foreign_entry = entry.clone();
    foreign_entry.inputs = EntryInputs::Retired {
        blob: medulla_decision_types::decision::statistical::log::InputsBlob {
            sha256: "sha256:00".to_string(),
            manifest_digest: "manifest".to_string(),
            holder_id: "holder".to_string(),
            length: 0,
        },
        compacted_at_ms: 0,
        retired_at_ms: 0,
    };
    h.store
        .put_decision_artifacts(
            "tenant-foreign",
            &[(
                foreign_key.clone(),
                medulla_persistence_records::decision_jobs::encode_artifact(&foreign_entry)
                    .unwrap(),
            )],
        )
        .unwrap();

    let policy = h.publish_policy(
        "expire-graph-summaries",
        &log_tests::retention_policy(100, Some(200)),
    );
    let start = entry.committed_at_ms;
    let retired_at = start + 300;
    for (now, expected) in [
        (start + 100, (1, 0, 0)),
        (retired_at, (0, 1, 0)),
        (retired_at + 90 * DAY_MS - 1, (0, 0, 0)),
    ] {
        let changed = compact_at(&h, &policy, now).await;
        assert_eq!(
            (changed.compacted, changed.retired, changed.expired),
            expected
        );
        assert!(h
            .store
            .decision_artifact(
                TENANT,
                &medulla_persistence_records::decision_jobs::record_key(&graph_id)
            )
            .unwrap()
            .is_some());
    }

    // At the exact boundary the graph summary and its joined evaluation go;
    // the library record and the other tenant's entry stay.
    let changed = compact_at(&h, &policy, retired_at + 90 * DAY_MS).await;
    assert_eq!(
        (changed.compacted, changed.retired, changed.expired),
        (0, 0, 1)
    );
    let record_key = medulla_persistence_records::decision_jobs::record_key(&graph_id);
    assert!(h
        .store
        .decision_artifact(TENANT, &record_key)
        .unwrap()
        .is_none());
    assert!(h
        .store
        .decision_artifact(TENANT, &evaluation_key)
        .unwrap()
        .is_none());
    assert_eq!(
        serde_json::to_vec(&log_tests::log_entry(&h, &library_entry.record.record_id)).unwrap(),
        library_before,
        "a library record never expires"
    );
    assert!(
        h.store
            .decision_artifact("tenant-foreign", &foreign_key)
            .unwrap()
            .is_some(),
        "another tenant's entry is untouched"
    );
    let again = compact_at(&h, &policy, u64::MAX).await;
    assert_eq!(
        again.expired, 0,
        "the library record and the foreign tenant remain"
    );
}
