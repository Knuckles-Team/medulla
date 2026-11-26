//! Evidence-stream references sealed into decision records
//! (MEDULLA-DECISION-ENGINE-R019.100.1): a served `Decide` seals the streams
//! it names into its inputs and premises, refuses a repeated stream or a
//! malformed digest by code, and the committed record reads back through the
//! decision log to an authorized reader only.

use super::*;
use medulla_decision_types::decision::evidence_stream::{
    EvidenceStreamKind, EvidenceStreamRef, EVIDENCE_STREAM_DIGEST_INVALID,
    EVIDENCE_STREAM_DUPLICATE,
};
use medulla_decision_types::decision::PremiseClass;

fn stream(kind: EvidenceStreamKind, id: &str, digest: String) -> EvidenceStreamRef {
    EvidenceStreamRef {
        kind,
        stream_id: id.to_string(),
        revision: 7,
        digest,
    }
}

fn digest(c: char) -> String {
    format!("sha256:{}", c.to_string().repeat(64))
}

// spec: MEDULLA-DECISION-ENGINE-R019.100.1
#[tokio::test]
async fn evidence_streams_are_sealed_and_read_back() {
    let h = Harness::new().await;
    let RouteFixture { schema_pin, .. } = route_fixture(&h).await;
    let ask = |streams: Vec<EvidenceStreamRef>| {
        let mut req = request(
            &schema_pin,
            None,
            DecisionPolicyRef::Default,
            QuestionSafety::Ordinary,
        );
        req.evidence_streams = BoundedVec::new(streams).unwrap();
        req
    };

    // A request without streams seals none: the inputs omit the field.
    let plain = decide(&h, ask(Vec::new()))
        .await
        .unwrap()
        .records
        .as_slice()[0]
        .clone();
    assert!(plain.inputs.evidence_streams.is_empty());
    assert!(!plain
        .premises
        .iter()
        .any(|p| p.fact.starts_with("evidence_stream:")));

    // Two streams are sealed into the inputs and each is one CLAIM premise.
    let streams = vec![
        stream(EvidenceStreamKind::Simulation, "sim-1", digest('a')),
        stream(EvidenceStreamKind::ProcessKnowledge, "proc-1", digest('b')),
    ];
    let record = decide(&h, ask(streams.clone()))
        .await
        .unwrap()
        .records
        .as_slice()[0]
        .clone();
    assert_eq!(
        record.inputs.evidence_streams.as_slice(),
        streams.as_slice()
    );
    assert_ne!(
        record.inputs_digest, plain.inputs_digest,
        "the inputs digest covers the streams"
    );
    for (fact, subject) in [
        ("evidence_stream:simulation", "sim-1"),
        ("evidence_stream:process_knowledge", "proc-1"),
    ] {
        let premise = record
            .premises
            .iter()
            .find(|p| p.fact == fact)
            .unwrap_or_else(|| panic!("premise {fact}"));
        assert_eq!(premise.subject, subject);
        assert_eq!(premise.class, PremiseClass::Claim);
        assert!(matches!(
            &premise.provenance,
            medulla_decision_types::decision::PremiseProvenance::Request { field }
                if field == "evidence_streams"
        ));
    }

    // Both refusal codes are served, by code.
    let twice = vec![
        stream(EvidenceStreamKind::Inspection, "ins-1", digest('c')),
        stream(EvidenceStreamKind::Inspection, "ins-1", digest('d')),
    ];
    let refused = decide(&h, ask(twice)).await.unwrap_err();
    assert!(
        refused.starts_with(EVIDENCE_STREAM_DUPLICATE.as_str()),
        "{refused}"
    );
    let bad = vec![stream(
        EvidenceStreamKind::Simulation,
        "sim-2",
        "sha256:ABC".to_string(),
    )];
    let refused = decide(&h, ask(bad.clone())).await.unwrap_err();
    assert!(
        refused.starts_with(EVIDENCE_STREAM_DIGEST_INVALID.as_str()),
        "{refused}"
    );

    // AgentAssemble refuses the same references by the same codes, before it
    // reads the library.
    let assemble_refusal = |streams: Vec<EvidenceStreamRef>| {
        let h = &h;
        async move {
            let mut req = medulla_decision_types::decision::AssemblyRequest {
                tenant_id: TENANT.to_string(),
                requirements: Default::default(),
                candidates: LibraryCandidateScope {
                    kinds: BoundedVec::new(vec![AgentComponentKind::Tool]).unwrap(),
                    classification_under: None,
                },
                templates: BoundedVec::default(),
                policy: DecisionPolicyRef::Default,
                solver: None,
                evidence_streams: Default::default(),
                primitive_edges: Default::default(),
                replacements: Default::default(),
                assistant_profile_ref: None,
                coding_agent: None,
                patterns: Default::default(),
                prompts: Default::default(),
                step_contracts: Default::default(),
            };
            req.evidence_streams = BoundedVec::new(streams).unwrap();
            let response = super::super::assemble::handle_agent_assemble(
                &h.state,
                3,
                ("default", &verified()),
                req,
            )
            .await;
            response.error.expect("a refusal")
        }
    };
    assert!(assemble_refusal(bad)
        .await
        .starts_with(EVIDENCE_STREAM_DIGEST_INVALID.as_str()));
    let dup = vec![
        stream(EvidenceStreamKind::Simulation, "s", digest('e')),
        stream(EvidenceStreamKind::Simulation, "s", digest('e')),
    ];
    assert!(assemble_refusal(dup)
        .await
        .starts_with(EVIDENCE_STREAM_DUPLICATE.as_str()));

    // The committed record reads back through the log with its streams, to
    // an authorized reader in the tenant; another tenant's reader is refused.
    let commit = DecisionLogOp::Commit {
        record: Box::new(record.clone()),
        evaluator: None,
        approval: None,
    };
    let committed: DecisionLogCommitted = decode(log_op(&h, "decider", commit).await).unwrap();
    let get = DecisionLogOp::Get {
        tenant_id: TENANT.to_string(),
        record_id: committed.record_id.clone(),
    };
    let entry: Option<DecisionLogEntry> = decode(log_op(&h, "decider", get.clone()).await).unwrap();
    let entry = entry.expect("the committing principal reads its record");
    assert_eq!(
        entry.record.inputs.evidence_streams.as_slice(),
        streams.as_slice()
    );
    let foreign = VerifiedRequestContext::verified_for_test_in_tenant("outsider", "tenant-other");
    let denied = decode::<Option<DecisionLogEntry>>(
        super::super::log::handle_decision_log(&h.state, 11, &foreign, get).await,
    )
    .unwrap_err();
    assert!(denied.starts_with("ACCESS_DENIED"), "{denied}");
}

// spec: MEDULLA-DECISION-ENGINE-R019.100.2
#[tokio::test]
async fn missing_stale_and_mismatched_approval_refuse_without_write() {
    use medulla_decision_types::decision::approval::RecommendationApproval;
    use medulla_decision_types::decision::DecisionPolicy;

    let now = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
    let approval = |record_id: &str, record_digest: &str, at_ms: u64| RecommendationApproval {
        record_id: record_id.to_string(),
        record_digest: record_digest.to_string(),
        reviewer_principal: "reviewer".to_string(),
        approved_at_ms: at_ms,
        approval_revision: 1,
    };
    let max_age_ms = 86_400_000_u64;

    // DecisionLog Commit: the submitted record's digest is the one the receipt names.
    let h = Harness::new().await;
    let RouteFixture { schema_pin, .. } = route_fixture(&h).await;
    let mut policy = DecisionPolicy::engine_default();
    policy.statistical = Some(super::super::stat_support::default_statistical_policy());
    policy.approval_required = true;
    let pin = h.publish_policy("policy-approval-required", &policy);
    let record = decide(
        &h,
        request(
            &schema_pin,
            None,
            DecisionPolicyRef::Pinned { component: pin },
            QuestionSafety::Ordinary,
        ),
    )
    .await
    .unwrap()
    .records
    .as_slice()[0]
        .clone();
    let logged = |h: &Harness| {
        h.store
            .decision_artifacts_with_prefix(TENANT, "", 100_000)
            .unwrap()
            .len()
    };
    let before = logged(&h);
    let commit = |approval: Option<RecommendationApproval>| DecisionLogOp::Commit {
        record: Box::new(record.clone()),
        evaluator: None,
        approval,
    };
    let refused = |outcome: Result<DecisionLogCommitted, String>, code: &str| {
        let error = outcome.unwrap_err();
        assert!(error.starts_with(code), "{error}");
    };
    refused(
        decode(log_op(&h, "decider", commit(None)).await),
        "APPROVAL_MISSING",
    );
    refused(
        decode(
            log_op(
                &h,
                "decider",
                commit(Some(approval("other", &record.record_digest, now))),
            )
            .await,
        ),
        "APPROVAL_DIGEST_MISMATCH",
    );
    refused(
        decode(
            log_op(
                &h,
                "decider",
                commit(Some(approval(
                    &record.record_id,
                    &record.record_digest,
                    now - max_age_ms - 60_000,
                ))),
            )
            .await,
        ),
        "APPROVAL_STALE",
    );
    assert_eq!(logged(&h), before, "every refusal writes nothing");
    let good = approval(&record.record_id, &record.record_digest, now);
    let committed: DecisionLogCommitted =
        decode(log_op(&h, "decider", commit(Some(good.clone()))).await).unwrap();
    assert!(!committed.replayed);
    let entry: Option<DecisionLogEntry> = decode(
        log_op(
            &h,
            "decider",
            DecisionLogOp::Get {
                tenant_id: TENANT.to_string(),
                record_id: committed.record_id.clone(),
            },
        )
        .await,
    )
    .unwrap();
    assert_eq!(entry.expect("the entry").approval, Some(good.clone()));
    let retried: DecisionLogCommitted =
        decode(log_op(&h, "decider", commit(Some(good))).await).unwrap();
    assert!(retried.replayed, "an approved retry answers replayed");

    // DecisionCommit: the receipt names the DecisionRecord's id and digest.
    use medulla_decision_types::decision::{DecisionCommitRequest, DecisionCommitResult};
    use medulla_persistence_records::agent_library::AgentLibraryStateExt;
    use medulla_persistence_records::decision_record::tests::{
        commit_context, decided_under, seed_library, TENANT,
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_str().unwrap().to_string();
    let backend = Arc::new(
        medulla_persistence::redb_backend::RedbBackend::open_with_shards(path.clone(), 16, 1)
            .unwrap(),
    );
    let mut server = ServerState::new_for_test(
        "approval-commit",
        medulla_isolation::isolation::IsolationLayer::new(),
    );
    server.persist_dir = Some(path);
    server.persistence = Some(backend);
    let state = Arc::new(tokio::sync::RwLock::new(server));
    let store = state.write().await.ensure_agent_library().unwrap();
    seed_library(&store, TENANT);
    let mut required = DecisionPolicy::engine_default();
    required.approval_required = true;
    let assembled = decided_under(&store, TENANT, 20, required).record;
    let verified = |key: &str, nonce: u8| {
        let claims = VerifiedRequestContext::verified_for_test_in_tenant("caller-a", TENANT);
        VerifiedRequestContext::from_verified_claims_with_nonce(
            claims.claims().clone(),
            key.into(),
            Some(medulla_types::contract::Nonce::from_bytes([nonce; 32])),
        )
    };
    let commit_request = |key: &str, nonce: u8, approval: Option<RecommendationApproval>| {
        DecisionCommitRequest {
            context: commit_context(&store, TENANT, key, nonce),
            expected_catalog_digest: assembled.inputs.catalog_digest.clone(),
            record: assembled.clone(),
            topology: None,
            retry_of: None,
            approval,
        }
    };
    let try_commit = |key: &'static str, nonce: u8, approval: Option<RecommendationApproval>| {
        let (state, request, verified) =
            (state.clone(), commit_request(key, nonce, approval), verified(key, nonce));
        async move {
            super::super::commit::handle_decision_commit(&state, 1, &verified, request).await
        }
    };
    for (key, nonce, receipt, code) in [
        ("missing", 1_u8, None, "APPROVAL_MISSING"),
        (
            "mismatch",
            2,
            Some(approval(&assembled.record_id, "sha256:other", now)),
            "APPROVAL_DIGEST_MISMATCH",
        ),
        (
            "stale",
            3,
            Some(approval(
                &assembled.record_id,
                &assembled.record_digest,
                now - max_age_ms - 60_000,
            )),
            "APPROVAL_STALE",
        ),
    ] {
        let response = try_commit(key, nonce, receipt).await;
        assert_eq!(response.error.as_deref(), Some(code));
        assert!(response.result.is_none());
        assert!(store
            .current_component(TENANT, &assembled.record_id)
            .unwrap()
            .is_none());
    }
    let good = approval(&assembled.record_id, &assembled.record_digest, now);
    let response = try_commit("approved", 4, Some(good.clone())).await;
    assert!(response.error.is_none(), "{:?}", response.error);
    let Some(medulla_protocol_types::protocol::ResultPayload::Raw(bytes)) = response.result else {
        panic!("commit result")
    };
    let committed: DecisionCommitResult = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(committed.record_id, assembled.record_id);
    let stored = store
        .current_component(TENANT, &assembled.record_id)
        .unwrap()
        .expect("the approved record is committed");
    assert_eq!(
        stored
            .attributes
            .get(medulla_persistence_records::decision_record::APPROVAL_ATTRIBUTE)
            .map(String::as_str),
        Some(serde_json::to_string(&good).unwrap().as_str())
    );
    // An approved retry (same approval, fresh request) answers replayed.
    let again = try_commit("approved", 4, Some(good)).await;
    assert!(again.error.is_none(), "{:?}", again.error);
    let Some(medulla_protocol_types::protocol::ResultPayload::Raw(bytes)) = again.result else {
        panic!("retry result")
    };
    let replayed: DecisionCommitResult = rmp_serde::from_slice(&bytes).unwrap();
    assert!(replayed.replayed, "an approved retry answers replayed");
}
