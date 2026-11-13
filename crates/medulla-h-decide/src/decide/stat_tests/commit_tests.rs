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
