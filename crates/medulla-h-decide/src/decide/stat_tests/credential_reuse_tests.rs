//! MEDULLA-DECISION-ENGINE-R832: a served `CredentialReuse` question admits a use that
//! sits inside the owner's binding, denies one outside it with the reason as the option, and
//! abstains when no binding accompanies it.

use super::*;
use medulla_decision_statistical_types::decision::statistical::declared::DeclaredOption;
use {
    medulla_decision_types::decision::record::AbstainReason,
    medulla_decision_types::decision::record::ResolutionKind,
};

const FAR_FUTURE_MS: u64 = u64::MAX / 2;

fn binding(expires_at_ms: u64, revoked: bool) -> serde_json::Value {
    serde_json::json!({
        "owner": "usr:alice",
        "agents": ["agent-worker"],
        "connectors": ["github-mcp"],
        "providers": ["github"],
        "scopes": ["repo:read", "repo:write"],
        "purposes": ["triage"],
        "expires_at_ms": expires_at_ms,
        "revoked": revoked,
        "revision": 4,
    })
}

struct Use {
    owner: &'static str,
    agent_id: &'static str,
    connector: &'static str,
    scopes: Vec<&'static str>,
    purpose: &'static str,
    binding: Option<serde_json::Value>,
}

impl Use {
    fn inside() -> Self {
        Self {
            owner: "usr:alice",
            agent_id: "agent-worker",
            connector: "github-mcp",
            scopes: vec!["repo:read"],
            purpose: "triage",
            binding: Some(binding(FAR_FUTURE_MS, false)),
        }
    }

    fn request(
        &self,
        schema: &ComponentDependency,
        tenant: &str,
        policy: DecisionPolicyRef,
    ) -> DecideRequest {
        let text = |name: &str, value: &str| TypedParam {
            name: name.to_string(),
            value: TypedValue::Text(value.to_string()),
        };
        let mut params = vec![
            text("owner", self.owner),
            text("agent_id", self.agent_id),
            text("connector", self.connector),
            text("provider", "github"),
            text("purpose", self.purpose),
            text("scopes", &serde_json::json!(self.scopes).to_string()),
        ];
        if let Some(binding) = &self.binding {
            params.push(text("binding", &binding.to_string()));
        }
        DecideRequest {
            tenant_id: tenant.to_string(),
            question: StatisticalQuestion {
                question_id: "credential.reuse".to_string(),
                kind: QuestionKind::CredentialReuse,
                safety: QuestionSafety::Ordinary,
            },
            candidates: CandidateSource::Declared {
                options: BoundedVec::new(vec![DeclaredOption {
                    option_id: "admit".to_string(),
                    classification: BoundedVec::default(),
                    numbers: BoundedVec::default(),
                    texts: BoundedVec::default(),
                }])
                .unwrap(),
            },
            feature_schema: schema.clone(),
            head: None,
            policy,
            params: BoundedVec::new(params).unwrap(),
            max_records: None,
            belief_as_of: BoundedVec::default(),
            evidence_streams: Default::default(),
        }
    }
}

/// The option the one advisory record chose.
fn chosen(batch: &DecisionBatch) -> String {
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.question.kind, QuestionKind::CredentialReuse);
    let StatisticalOutcome::Advisory { scores, .. } = &record.outcome else {
        panic!("expected an advisory answer, got {:?}", record.outcome)
    };
    assert_eq!(record.resolution_kind, ResolutionKind::Constraint);
    assert_eq!(record.evidence_class, EvidenceClass::Claim);
    scores
        .iter()
        .map(|s| s.option_id.clone())
        .collect::<Vec<_>>()
        .join(",")
}

// spec: MEDULLA-DECISION-ENGINE-R832
#[tokio::test]
async fn credential_reuse_admits_denies_and_abstains() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    let run = |usage: &Use| {
        decide(
            &h,
            usage.request(&schema, TENANT, DecisionPolicyRef::Default),
        )
    };

    // Inside the binding: admitted.
    assert_eq!(chosen(&run(&Use::inside()).await.unwrap()), "admit");

    // Each way outside the binding denies with its registered code as the reason.
    let mut other_owner = Use::inside();
    other_owner.owner = "usr:mallory";
    assert_eq!(
        chosen(&run(&other_owner).await.unwrap()),
        "deny:CREDENTIAL_OWNER_MISMATCH"
    );
    let mut other_agent = Use::inside();
    other_agent.agent_id = "agent-intruder";
    assert_eq!(
        chosen(&run(&other_agent).await.unwrap()),
        "deny:CREDENTIAL_AGENT_NOT_BOUND"
    );
    let mut revoked = Use::inside();
    revoked.binding = Some(binding(FAR_FUTURE_MS, true));
    assert_eq!(
        chosen(&run(&revoked).await.unwrap()),
        "deny:CREDENTIAL_REVOKED"
    );
    // The engine clock, not the caller, decides expiry: a binding that ended at 0 is expired.
    let mut expired = Use::inside();
    expired.binding = Some(binding(0, false));
    assert_eq!(
        chosen(&run(&expired).await.unwrap()),
        "deny:CREDENTIAL_EXPIRED"
    );
    let mut other_connector = Use::inside();
    other_connector.connector = "gitlab-mcp";
    assert_eq!(
        chosen(&run(&other_connector).await.unwrap()),
        "deny:CREDENTIAL_CONNECTOR_NOT_BOUND"
    );
    let mut wide = Use::inside();
    wide.scopes = vec!["repo:read", "repo:admin"];
    assert_eq!(
        chosen(&run(&wide).await.unwrap()),
        "deny:CREDENTIAL_SCOPE_EXCEEDED"
    );
    let mut other_purpose = Use::inside();
    other_purpose.purpose = "deploy";
    assert_eq!(
        chosen(&run(&other_purpose).await.unwrap()),
        "deny:CREDENTIAL_PURPOSE_NOT_ALLOWED"
    );

    // No binding: nothing to decide on, an abstention naming the registered code.
    let mut unbound = Use::inside();
    unbound.binding = None;
    let batch = run(&unbound).await.unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.resolution_kind, ResolutionKind::Abstention);
    let StatisticalOutcome::Abstained { reasons } = &record.outcome else {
        panic!("expected an abstention, got {:?}", record.outcome)
    };
    match &reasons.as_slice()[0] {
        AbstainReason::Family { family, code, .. } => {
            assert_eq!(*family, QuestionKind::CredentialReuse);
            assert_eq!(code.as_str(), "CREDENTIAL_BINDING_MISSING");
        }
        other => panic!("expected a family reason, got {other:?}"),
    }
}
