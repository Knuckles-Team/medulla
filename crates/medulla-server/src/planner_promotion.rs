//! Promotion of the planner's challenger cost head on shadow evidence
//! (MEDULLA-FEDERATED-QUERY-R732.5).
//!
//! A downtime job runs as the engine's own service principal ([`PLANNER_ACTOR`]) holding
//! exactly the scopes the policy-evolution path checks ([`PLANNER_SCOPES`]). It judges the
//! challenger on the shadow comparisons the served path recorded, publishes both cost-head
//! revisions as `PlannerCostHead` component revisions, writes one `PolicyEvaluation` of the
//! `planner` knob set into the commons graph, and only on a passing verdict asks `Decide`
//! for a `Promotion` and moves the champion pointer through `DecisionLog`. The planner reads
//! the champion from [`PlanRecordStore`]; the store is rebuilt from the pointer at startup by
//! [`restore_champion`], so only a promoted revision ever answers a query.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use medulla_agent_types::agent_component::{
    AgentComponentDraft, AgentComponentKind, AgentComponentOp, AgentComponentPublishRequest,
    ComponentDependency, DraftPublication, DraftSubject,
};
use medulla_agent_types::agent_library::AgentLibraryMutationContext;
use medulla_decision_outcome_types::policy_evolution::{
    EvaluationMetrics, EvaluationVerdict, PolicyEvaluation, PolicyEvolutionOp, PolicyRecordReceipt,
    SafetyOutcome, MAX_HELD_OUT_TRAJECTORIES,
};
use medulla_decision_types::decision::held_out::{GraderKind, RevisionSubject};
use medulla_decision_types::decision::planner_cost_head::{
    COMPONENT_DIGEST_PREFIX, PLANNER_COST_HEAD_ATTRIBUTE, PLANNER_POLICY_ID,
};
use medulla_decision_types::decision::statistical::body::encode_body;
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::features::{
    FeatureKind, FeatureSchemaBody, FeatureSpec, MissingValue, FEATURE_SCHEMA_VERSION,
};
use medulla_decision_types::decision::statistical::log::DecisionLogOp;
use medulla_decision_types::decision::statistical::retrieval::{
    LearningWrite, PointerMovement, PointerRef,
};
use medulla_decision_types::decision::statistical::retrieval_pointer::PointerState;
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, DecisionBatch, QuestionKind, QuestionSafety,
    StatisticalQuestion, TypedParam, TypedValue,
};
use medulla_decision_types::decision::DecisionPolicyRef;
use medulla_exec::learned_cost::{CostHeadRevision, PlanRecordStore, ShadowComparison};
use medulla_persistence_records::agent_library::AgentLibraryStateExt;
use medulla_persistence_records::decision_jobs::decode_artifact;
use medulla_protocol_types::protocol::{Method, Request, Response, ResultPayload};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_types::contract::{BoundedVec, Digest256, Nonce, OpaqueId};
use serde::Serialize;
use tokio::sync::RwLock;

/// The service principal a planner-evolution job runs as.
pub const PLANNER_ACTOR: &str = "engine:planner-evolution";

/// The evaluator identity every planner evaluation names.
pub const PLANNER_EVALUATOR: &str = "engine:planner-evolution";

/// Exactly the scopes the promotion path checks, one per step: `Decide` (`query:decide`),
/// committing the `Promotion` record (`agent:decision-write`), moving the champion pointer
/// (`admin:decision-head`), writing the `PolicyEvaluation`
/// (`admin:policy-evaluation-write`), publishing the `PlannerCostHead` revisions
/// (`admin:decision-policy`) and publishing the one `FeatureSchema` pin `Decide` requires
/// (`admin:decision-catalog`).
pub const PLANNER_SCOPES: [&str; 6] = [
    "query:decide",
    "agent:decision-write",
    "admin:decision-head",
    "admin:policy-evaluation-write",
    "admin:decision-policy",
    "admin:decision-catalog",
];

/// The graph the evaluation, the decision and the champion pointer are engine-wide in.
pub const PLANNER_GRAPH: &str = "__commons__";

/// The component id of the schema pin the `Promotion` question names.
const SCHEMA_COMPONENT_ID: &str = "planner.promotion.feature_schema";

/// The measurement a challenger is judged on, derived from its shadow comparisons.
#[derive(Clone, Debug, PartialEq)]
pub struct ShadowAssessment {
    /// How many comparisons the judgment covers.
    pub comparisons: usize,
    /// Comparisons whose challenger rows differ from the champion's.
    pub mismatches: usize,
    /// Comparisons whose challenger run errored or panicked.
    pub failures: usize,
    /// p95 of the champion's relative estimate error.
    pub champion_p95_error: f64,
    /// p95 of the challenger's relative estimate error.
    pub challenger_p95_error: f64,
    /// p95 of the champion's measured cost in microseconds.
    pub champion_p95_cost_us: u64,
    /// p95 of the challenger's measured cost in microseconds.
    pub challenger_p95_cost_us: u64,
    /// The metrics the `PolicyEvaluation` records.
    pub metrics: EvaluationMetrics,
    /// Whether the challenger passed: both p95s improve, no result differs, none failed.
    pub accepted: bool,
    /// The digest of the ordered comparison list.
    pub evaluation_set_digest: Digest256,
    /// One held-out id per comparison, in order.
    pub held_out_ids: Vec<String>,
}

/// The nearest-rank p95 of `values`, which is not empty.
fn p95<T: Copy + PartialOrd>(mut values: Vec<T>) -> T {
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let rank = (values.len() * 95).div_ceil(100).max(1);
    values[rank - 1]
}

/// The median of `values`, which is not empty.
fn p50<T: Copy + PartialOrd>(mut values: Vec<T>) -> T {
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    values[(values.len() - 1) / 2]
}

fn ppm(fraction: f64) -> u32 {
    (fraction.clamp(0.0, 1.0) * 1_000_000.0).round() as u32
}

/// Judge the challenger on `shadows` (MEDULLA-FEDERATED-QUERY-R732.5). Per comparison the
/// estimate error of a side is `|estimated - champion_cost_us| / max(champion_cost_us, 1)`.
/// The metric mapping: `task_success_ppm` and `baseline_task_success_ppm` are the mean
/// estimate accuracy (`1 - min(error, 1)`) of the challenger and the champion;
/// `trace_completeness_ppm` is the row-digest agreement rate; `unsupported_replay_mass_ppm`
/// the share of failed challenger runs; `cost_micros` and the two latency fields are the
/// challenger's p95 cost and its p50 and p95 in milliseconds. `None` when fewer than
/// `min_comparisons` comparisons exist.
pub fn assess(shadows: &[ShadowComparison], min_comparisons: usize) -> Option<ShadowAssessment> {
    let kept = shadows.len().min(MAX_HELD_OUT_TRAJECTORIES);
    let shadows = &shadows[shadows.len() - kept..];
    if shadows.is_empty() || shadows.len() < min_comparisons.max(1) {
        return None;
    }
    let count = shadows.len() as f64;
    let error = |estimated: f64, champion_cost_us: u64| {
        (estimated - champion_cost_us as f64).abs() / champion_cost_us.max(1) as f64
    };
    let champion_errors: Vec<f64> = shadows
        .iter()
        .map(|c| error(c.champion_estimated_cost, c.champion_cost_us))
        .collect();
    let challenger_errors: Vec<f64> = shadows
        .iter()
        .map(|c| error(c.challenger_estimated_cost, c.champion_cost_us))
        .collect();
    let accuracy = |errors: &[f64]| errors.iter().map(|e| 1.0 - e.min(1.0)).sum::<f64>() / count;
    let champion_costs: Vec<u64> = shadows.iter().map(|c| c.champion_cost_us).collect();
    let challenger_costs: Vec<u64> = shadows.iter().map(|c| c.challenger_cost_us).collect();
    let mismatches = shadows.iter().filter(|c| !c.result_match).count();
    let failures = shadows.iter().filter(|c| c.challenger_failed).count();
    let champion_p95_error = p95(champion_errors.clone());
    let challenger_p95_error = p95(challenger_errors.clone());
    let champion_p95_cost_us = p95(champion_costs);
    let challenger_p95_cost_us = p95(challenger_costs.clone());
    let metrics = EvaluationMetrics {
        task_success_ppm: ppm(accuracy(&challenger_errors)),
        baseline_task_success_ppm: ppm(accuracy(&champion_errors)),
        cost_micros: challenger_p95_cost_us,
        latency_p50_ms: u32::try_from(p50(challenger_costs) / 1_000).unwrap_or(u32::MAX),
        latency_p95_ms: u32::try_from(challenger_p95_cost_us / 1_000).unwrap_or(u32::MAX),
        gpu_seconds: 0,
        trace_completeness_ppm: ppm((shadows.len() - mismatches) as f64 / count),
        unsupported_replay_mass_ppm: ppm(failures as f64 / count),
    };
    let accepted = mismatches == 0
        && failures == 0
        && challenger_p95_error < champion_p95_error
        && challenger_p95_cost_us < champion_p95_cost_us
        && metrics.task_success_ppm >= metrics.baseline_task_success_ppm;
    let held_out_ids: Vec<String> = shadows
        .iter()
        .map(|comparison| {
            // A comparison always serializes: plain strings, numbers and booleans.
            let body = serde_json::to_vec(comparison).unwrap_or_default();
            format!("shadow:{}", Digest256::sha256(&body).to_hex())
        })
        .collect();
    let evaluation_set_digest = Digest256::sha256(held_out_ids.join("\n").as_bytes());
    Some(ShadowAssessment {
        comparisons: shadows.len(),
        mismatches,
        failures,
        champion_p95_error,
        challenger_p95_error,
        champion_p95_cost_us,
        challenger_p95_cost_us,
        metrics,
        accepted,
        evaluation_set_digest,
        held_out_ids,
    })
}

/// What one promotion attempt did.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PromotionReceipt {
    /// `no_challenger`, `too_few_comparisons`, `commons_unavailable`, `challenger_rejected`,
    /// `held` or `promoted`.
    pub outcome: String,
    /// The challenger's revision reference (`sha256:<digest>`), when there was one.
    pub challenger: Option<String>,
    /// The champion's revision reference at the time.
    pub champion: Option<String>,
    /// The shadow comparisons the judgment covered.
    pub comparisons: usize,
    /// Comparisons whose rows differed from the champion's.
    pub mismatches: usize,
    /// The champion's and the challenger's p95 estimate error.
    pub p95_error: Option<(f64, f64)>,
    /// The champion's and the challenger's p95 cost in microseconds.
    pub p95_cost_us: Option<(u64, u64)>,
    /// The `PolicyEvaluation` record the run wrote.
    pub evaluation_id: Option<String>,
}

impl PromotionReceipt {
    fn bare(outcome: &str) -> Self {
        Self {
            outcome: outcome.to_string(),
            challenger: None,
            champion: None,
            comparisons: 0,
            mismatches: 0,
            p95_error: None,
            p95_cost_us: None,
            evaluation_id: None,
        }
    }
}

/// The revision reference a cost head carries in the component store and the pointer.
fn revision_ref(revision: &CostHeadRevision) -> String {
    format!("{COMPONENT_DIGEST_PREFIX}{}", revision.digest)
}

/// Distinguishes the calls of one run.
static CALLS: AtomicU64 = AtomicU64::new(0);

/// The engine's own service context for one call; its key names the call.
fn service_context(
    anonymous: bool,
    server: &medulla_server_api::server_context::ServerContext,
    key: String,
) -> Result<VerifiedRequestContext, String> {
    if anonymous {
        return Ok(VerifiedRequestContext::anonymous_local(Some(key)));
    }
    let service = VerifiedRequestContext::authenticated_fixed_service_actor(
        server,
        PLANNER_ACTOR,
        &PLANNER_SCOPES,
    )?;
    Ok(VerifiedRequestContext::from_verified_claims_with_nonce(
        service.claims().clone(),
        key,
        Some(Nonce::minted()),
    ))
}

/// Everything one run needs to reach the engine as the service principal.
struct Engine<'a> {
    state: &'a Arc<RwLock<ServerState>>,
    anonymous: bool,
    server: medulla_server_api::server_context::ServerContext,
    tenant: String,
}

impl Engine<'_> {
    /// One served call under the service actor; a refusal is an `Err`.
    async fn call(&self, purpose: &str, graph: &str, method: Method) -> Result<Response, String> {
        let key = format!(
            "planner-evolution:{purpose}:{}",
            CALLS.fetch_add(1, Ordering::Relaxed)
        );
        self.call_keyed(key, graph, method).await
    }

    async fn call_keyed(&self, key: String, graph: &str, method: Method) -> Result<Response, String> {
        let context = service_context(self.anonymous, &self.server, key)?;
        let request = Request {
            id: 0,
            graph: graph.to_string(),
            auth_token: String::new(),
            agent_id: Some(PLANNER_ACTOR.to_string()),
            method,
        };
        let response =
            medulla_server_api::api::reentry::dispatch_verified_request(self.state, request, context)
                .await;
        match response.refusal_text() {
            Some(refusal) => Err(refusal),
            None => Ok(response),
        }
    }
}

fn decode<T: serde::de::DeserializeOwned>(response: Response) -> Result<T, String> {
    match response.result {
        Some(ResultPayload::Raw(bytes)) => {
            rmp_serde::from_slice(&bytes).map_err(|error| format!("planner promotion: {error}"))
        }
        _ => Err("planner promotion: the answer carries no typed payload".to_string()),
    }
}

fn opaque(value: &str) -> Result<OpaqueId, String> {
    OpaqueId::new(value.to_string()).map_err(|_| format!("planner promotion: `{value}` is not an id"))
}

/// The one subject the planner revisions, the evaluation and the pointer name.
fn subject() -> RevisionSubject {
    RevisionSubject::KnobSet {
        policy_id: PLANNER_POLICY_ID.to_string(),
    }
}

/// The draft of one component revision published under the service tenant.
fn draft(
    tenant: &str,
    component_id: &str,
    kind: AgentComponentKind,
    content_digest: &str,
    summary: &str,
    attributes: BTreeMap<String, String>,
    policy_digest: &str,
) -> AgentComponentDraft {
    AgentComponentDraft {
        attributes,
        ..AgentComponentDraft::bare(
            DraftSubject {
                component_id,
                kind,
                version: "1.0.0",
                content_digest,
                summary,
            },
            DraftPublication {
                tenant_id: tenant,
                actor_scope: PLANNER_ACTOR,
                purpose_id: "agent-component:publish",
                policy_digest,
                source_revision: "planner-evolution",
                source_revision_digest: content_digest,
            },
        )
    }
}

impl Engine<'_> {
    /// Publish `component` as the next revision of its id unless a revision with the same
    /// content digest is already retained; answers the revision's definition digest.
    async fn publish(&self, component: AgentComponentDraft) -> Result<String, String> {
        let store = self.state.write().await.ensure_agent_library()?;
        let revisions = store.component_revisions(&self.tenant, &component.component_id)?;
        if let Some(held) = revisions
            .iter()
            .rev()
            .find(|entry| entry.content_digest == component.content_digest)
        {
            return Ok(held.definition_digest.clone());
        }
        let head = revisions.last().map_or(0, |entry| entry.entry_revision);
        let key = format!(
            "planner-evolution:publish:{}:{}",
            component.component_id, component.content_digest
        );
        let policy_digest = component.policy_digest.clone();
        let request = AgentComponentPublishRequest {
            context: AgentLibraryMutationContext {
                request_id: 0,
                principal: store.owner_principal().to_string(),
                caller_principal: format!("principal:sha256:{}", "a".repeat(64)),
                attempt_nonce: Nonce::minted(),
                tenant_id: self.tenant.clone(),
                actor_scope: PLANNER_ACTOR.to_string(),
                purpose_id: "agent-component:publish".to_string(),
                policy_revision: "planner-evolution".to_string(),
                policy_digest,
                policy_decision_id: "agent-component:decision:planner-evolution".to_string(),
                idempotency_key: key.clone(),
                expected_revision: Some(head),
                trace_id: None,
                created_at_ms: 0,
            },
            component,
            evaluation_receipt_digest: None,
            body: None,
        };
        let id = request.component.component_id.clone();
        let content = request.component.content_digest.clone();
        self.call_keyed(
            key,
            PLANNER_GRAPH,
            Method::AgentComponent {
                op: AgentComponentOp::Publish {
                    request: Box::new(request),
                },
            },
        )
        .await?;
        store
            .component_revisions(&self.tenant, &id)?
            .into_iter()
            .rev()
            .find(|entry| entry.content_digest == content)
            .map(|entry| entry.definition_digest)
            .ok_or_else(|| format!("planner promotion: the published revision of {id} is unreadable"))
    }

    /// Publish a cost-head revision as a `PlannerCostHead` component revision.
    async fn publish_revision(&self, revision: &CostHeadRevision) -> Result<(), String> {
        let body = serde_json::to_string(revision)
            .map_err(|error| format!("planner promotion: {error}"))?;
        let digest = revision_ref(revision);
        let policy_digest = medulla_persistence_records::agent_library::current_agent_library_policy_digest()?;
        let attributes = BTreeMap::from([(PLANNER_COST_HEAD_ATTRIBUTE.to_string(), body)]);
        self.publish(draft(
            &self.tenant,
            PLANNER_POLICY_ID,
            AgentComponentKind::PlannerCostHead,
            &digest,
            "planner cost head",
            attributes,
            &policy_digest,
        ))
        .await
        .map(drop)
    }

    /// The schema pin the `Promotion` question names, published once.
    async fn schema_pin(&self) -> Result<ComponentDependency, String> {
        let body = FeatureSchemaBody {
            schema_version: FEATURE_SCHEMA_VERSION,
            features: BoundedVec::new(vec![FeatureSpec {
                name: "score".to_string(),
                kind: FeatureKind::Number {
                    key: "score".to_string(),
                },
                missing: MissingValue::Abstain,
            }])
            .map_err(|_| "planner promotion: the schema has too many features".to_string())?,
        };
        let encoded = encode_body(&body)?;
        let policy_digest = medulla_persistence_records::agent_library::current_agent_library_policy_digest()?;
        let definition_digest = self
            .publish(draft(
                &self.tenant,
                SCHEMA_COMPONENT_ID,
                AgentComponentKind::FeatureSchema,
                &encoded.content_digest,
                "planner promotion feature schema",
                encoded.attributes,
                &policy_digest,
            ))
            .await?;
        Ok(ComponentDependency {
            component_id: SCHEMA_COMPONENT_ID.to_string(),
            kind: AgentComponentKind::FeatureSchema,
            definition_digest,
        })
    }
}

/// The `Promotion` question for `challenger` on the evaluation `receipt`.
fn promotion_request(
    tenant: &str,
    schema: ComponentDependency,
    receipt: &str,
    challenger: &str,
) -> Result<DecideRequest, String> {
    let text = |name: &str, value: String| TypedParam {
        name: name.to_string(),
        value: TypedValue::Text(value),
    };
    let subject = serde_json::to_string(&subject())
        .map_err(|error| format!("planner promotion: {error}"))?;
    let invalid = |_| "planner promotion: a request bound is exceeded".to_string();
    Ok(DecideRequest {
        tenant_id: tenant.to_string(),
        question: StatisticalQuestion {
            question_id: format!("planner.promotion.{challenger}"),
            kind: QuestionKind::Promotion,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(vec![DeclaredOption {
                option_id: challenger.to_string(),
                classification: BoundedVec::default(),
                numbers: BoundedVec::default(),
                texts: BoundedVec::default(),
            }])
            .map_err(invalid)?,
        },
        feature_schema: schema,
        head: None,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::new(vec![
            text("mode", "promote".to_string()),
            text("receipt_digest", receipt.to_string()),
            text("subject", subject),
            text("graph", PLANNER_GRAPH.to_string()),
        ])
        .map_err(invalid)?,
        max_records: None,
        belief_as_of: BoundedVec::default(),
        evidence_streams: Default::default(),
    })
}

/// Judge the challenger and, when it wins, promote it (MEDULLA-FEDERATED-QUERY-R732.5).
pub async fn promote_on_shadow_evidence(
    state: &Arc<RwLock<ServerState>>,
    store: &PlanRecordStore,
    min_comparisons: usize,
) -> Result<PromotionReceipt, String> {
    let Some(challenger) = store.challenger() else {
        return Ok(PromotionReceipt::bare("no_challenger"));
    };
    let champion = store.champion();
    let challenger_ref = revision_ref(&challenger);
    let champion_ref = revision_ref(&champion);
    let Some(assessment) = assess(&store.all_shadows(), min_comparisons) else {
        return Ok(PromotionReceipt {
            challenger: Some(challenger_ref),
            champion: Some(champion_ref),
            ..PromotionReceipt::bare("too_few_comparisons")
        });
    };
    let mut receipt = PromotionReceipt {
        challenger: Some(challenger_ref.clone()),
        champion: Some(champion_ref.clone()),
        comparisons: assessment.comparisons,
        mismatches: assessment.mismatches,
        p95_error: Some((assessment.champion_p95_error, assessment.challenger_p95_error)),
        p95_cost_us: Some((assessment.champion_p95_cost_us, assessment.challenger_p95_cost_us)),
        ..PromotionReceipt::bare("commons_unavailable")
    };
    let (resident, anonymous, server) = {
        let current = state.read().await;
        let resident = current
            .registry
            .all_entries()
            .iter()
            .any(|entry| entry.name == PLANNER_GRAPH);
        (resident, current.local_anonymous, current.context.clone())
    };
    if !resident {
        return Ok(receipt);
    }
    let tenant = service_context(anonymous, &server, "planner-evolution:tenant".to_string())?
        .tenant()
        .to_string();
    let engine = Engine {
        state,
        anonymous,
        server,
        tenant: tenant.clone(),
    };
    engine.publish_revision(&champion).await?;
    engine.publish_revision(&challenger).await?;

    let evaluation = PolicyEvaluation {
        subject: subject(),
        revision_ref: opaque(&challenger_ref)?,
        champion_revision_ref: Some(opaque(&champion_ref)?),
        evaluation_set_digest: assessment.evaluation_set_digest,
        evaluator_digest: Digest256::sha256(PLANNER_EVALUATOR.as_bytes()),
        evaluator_kind: GraderKind::TraceMetric,
        held_out_trajectory_ids: BoundedVec::new(
            assessment
                .held_out_ids
                .iter()
                .map(|id| opaque(id))
                .collect::<Result<Vec<_>, _>>()?,
        )
        .map_err(|_| "planner promotion: too many comparisons".to_string())?,
        metrics: assessment.metrics,
        safety: if assessment.mismatches == 0 && assessment.failures == 0 {
            SafetyOutcome::Passed
        } else {
            SafetyOutcome::Failed
        },
        verdict: if assessment.accepted {
            EvaluationVerdict::Accepted
        } else {
            EvaluationVerdict::Rejected
        },
        report: None,
    };
    let written: PolicyRecordReceipt = decode(
        engine
            .call(
                "evaluate",
                PLANNER_GRAPH,
                Method::PolicyEvolution {
                    op: Box::new(PolicyEvolutionOp::CommitPolicyEvaluation { request: evaluation }),
                },
            )
            .await?,
    )?;
    receipt.evaluation_id = Some(written.record_id.clone());
    if !assessment.accepted {
        receipt.outcome = "challenger_rejected".to_string();
        return Ok(receipt);
    }

    let schema = engine.schema_pin().await?;
    let request = promotion_request(&tenant, schema, &written.record_id, &challenger_ref)?;
    let batch: DecisionBatch = decode(
        engine
            .call(
                "decide",
                PLANNER_GRAPH,
                Method::Decide {
                    request: Box::new(request),
                },
            )
            .await?,
    )?;
    let Some(record) = batch.records.as_slice().first().cloned() else {
        return Err("planner promotion: the promotion decision returned no record".to_string());
    };
    if !record
        .premises
        .iter()
        .any(|premise| premise.fact == "promotion.verdict=promote")
    {
        receipt.outcome = "held".to_string();
        return Ok(receipt);
    }
    let record_id = record.record_id.clone();
    engine
        .call(
            "commit",
            PLANNER_GRAPH,
            Method::DecisionLog {
                op: Box::new(DecisionLogOp::Commit {
                    record: Box::new(record),
                    evaluator: None,
                    approval: None,
                }),
            },
        )
        .await?;
    engine
        .call(
            "move-pointer",
            PLANNER_GRAPH,
            Method::DecisionLog {
                op: Box::new(DecisionLogOp::Learn {
                    tenant_id: tenant,
                    write: LearningWrite::MovePointer {
                        pointer: PointerRef::Champion { subject: subject() },
                        movement: PointerMovement::Activate {
                            target: challenger_ref,
                            receipt_digest: record_id,
                        },
                    },
                }),
            },
        )
        .await?;
    store.promote((*challenger).clone());
    receipt.outcome = "promoted".to_string();
    Ok(receipt)
}

/// Rebuild the planner's champion from the durable pointer (MEDULLA-FEDERATED-QUERY-R732.5):
/// the pointer's target names a retained `PlannerCostHead` revision, whose body becomes the
/// store's champion. A pointer that was never moved leaves the built-in table in force.
pub async fn restore_champion(
    state: &Arc<RwLock<ServerState>>,
    store: &PlanRecordStore,
) -> Result<Option<String>, String> {
    let (anonymous, server) = {
        let current = state.read().await;
        (current.local_anonymous, current.context.clone())
    };
    let tenant = service_context(anonymous, &server, "planner-evolution:restore".to_string())?
        .tenant()
        .to_string();
    let library = state.write().await.ensure_agent_library()?;
    let key = format!("pointer:{}", subject().pointer_key());
    let Some(bytes) = library.decision_artifact(&tenant, &key)? else {
        return Ok(None);
    };
    let pointer: PointerState = decode_artifact(&bytes, "pointer state")?;
    let Some(target) = pointer.target() else {
        return Ok(None);
    };
    let Some(entry) = library
        .component_revisions(&tenant, PLANNER_POLICY_ID)?
        .into_iter()
        .rev()
        .find(|entry| entry.kind == AgentComponentKind::PlannerCostHead && entry.content_digest == target)
    else {
        return Err(format!(
            "planner promotion: the champion pointer names {target}, which is not a retained revision"
        ));
    };
    let body = entry
        .attributes
        .get(PLANNER_COST_HEAD_ATTRIBUTE)
        .ok_or_else(|| "planner promotion: the champion revision carries no body".to_string())?;
    let revision: CostHeadRevision = serde_json::from_str(body)
        .map_err(|error| format!("planner promotion: the champion body is unreadable: {error}"))?;
    if revision_ref(&revision) != target {
        return Err("planner promotion: the champion body does not match its digest".to_string());
    }
    store.promote(revision);
    Ok(Some(target.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One comparison whose champion measured `champion_cost_us`, estimated
    /// `champion_estimate`, and whose challenger measured `challenger_cost_us`, estimated
    /// `challenger_estimate`.
    fn comparison(
        serial: i64,
        champion: (f64, u64),
        challenger: (f64, u64),
        result_match: bool,
    ) -> ShadowComparison {
        ShadowComparison {
            tenant: "tenant-a".into(),
            shape_digest: "shape".into(),
            champion_plan_digest: "champion-plan".into(),
            challenger_plan_digest: "challenger-plan".into(),
            champion_estimated_cost: champion.0,
            challenger_estimated_cost: challenger.0,
            champion_cost_us: champion.1,
            challenger_cost_us: challenger.1,
            challenger_result_digest: Some("digest".into()),
            result_match,
            challenger_failed: false,
            recorded_at_ms: serial,
        }
    }

    fn run(count: i64, champion: (f64, u64), challenger: (f64, u64)) -> Vec<ShadowComparison> {
        (0..count)
            .map(|serial| comparison(serial, champion, challenger, true))
            .collect()
    }

    // spec: MEDULLA-FEDERATED-QUERY-R732.5
    #[test]
    fn a_challenger_that_improves_both_p95s_with_equal_rows_is_accepted() {
        // The champion estimates 200 for a 100 us run (error 1.0); the challenger estimates
        // 105 for the same run (error 0.05) and runs in 80 us.
        let shadows = run(64, (200.0, 100), (105.0, 80));
        let assessment = assess(&shadows, 64).expect("enough comparisons");
        assert!(assessment.accepted);
        assert_eq!(assessment.comparisons, 64);
        assert!(assessment.challenger_p95_error < assessment.champion_p95_error);
        assert_eq!(
            (assessment.champion_p95_cost_us, assessment.challenger_p95_cost_us),
            (100, 80)
        );
        // The metric mapping: mean estimate accuracy, agreement rate, p95 cost.
        assert_eq!(assessment.metrics.baseline_task_success_ppm, 0);
        assert_eq!(assessment.metrics.task_success_ppm, 950_000);
        assert_eq!(assessment.metrics.trace_completeness_ppm, 1_000_000);
        assert_eq!(assessment.metrics.unsupported_replay_mass_ppm, 0);
        assert_eq!(assessment.metrics.cost_micros, 80);
        assert_eq!(assessment.held_out_ids.len(), 64);
        assert!(assessment.held_out_ids.iter().all(|id| id.starts_with("shadow:")));
    }

    // spec: MEDULLA-FEDERATED-QUERY-R732.5
    #[test]
    fn a_challenger_that_loses_a_measure_or_differs_once_stays_a_challenger() {
        // Corrections already right: the challenger's error is no better.
        let right = run(64, (100.0, 100), (100.0, 100));
        assert!(!assess(&right, 64).unwrap().accepted);
        // Better estimates but slower.
        let slower = run(64, (200.0, 100), (105.0, 120));
        assert!(!assess(&slower, 64).unwrap().accepted);
        // One differing result digest refuses an otherwise winning challenger.
        let mut differing = run(64, (200.0, 100), (105.0, 80));
        differing[10].result_match = false;
        let assessment = assess(&differing, 64).unwrap();
        assert!(!assessment.accepted);
        assert_eq!(assessment.mismatches, 1);
        // A failed challenger run is never accepted.
        let mut failed = run(64, (200.0, 100), (105.0, 80));
        failed[3].challenger_failed = true;
        assert!(!assess(&failed, 64).unwrap().accepted);
    }

    // spec: MEDULLA-FEDERATED-QUERY-R732.5
    #[test]
    fn fewer_comparisons_than_the_minimum_are_not_judged() {
        assert!(assess(&run(63, (200.0, 100), (105.0, 80)), 64).is_none());
        assert!(assess(&[], 1).is_none());
    }

    // spec: MEDULLA-FEDERATED-QUERY-R732.5
    #[test]
    fn the_service_principal_holds_exactly_the_promotion_scopes() {
        let mut scopes = PLANNER_SCOPES.to_vec();
        scopes.sort_unstable();
        scopes.dedup();
        assert_eq!(scopes.len(), PLANNER_SCOPES.len(), "no scope is listed twice");
        assert_eq!(PLANNER_ACTOR, PLANNER_EVALUATOR);
        // Every step of the path names the scope that checks it.
        assert_eq!(
            PolicyEvolutionOp::CommitPolicyEvaluation {
                request: sample_evaluation()
            }
            .authz_action(),
            "admin:policy-evaluation-write"
        );
        assert_eq!(
            AgentComponentKind::PlannerCostHead.publish_authz_action(),
            "admin:decision-policy"
        );
        assert_eq!(
            AgentComponentKind::FeatureSchema.publish_authz_action(),
            "admin:decision-catalog"
        );
        for scope in [
            "query:decide",
            "agent:decision-write",
            "admin:decision-head",
        ] {
            assert!(PLANNER_SCOPES.contains(&scope), "{scope}");
        }
    }

    fn sample_evaluation() -> PolicyEvaluation {
        PolicyEvaluation {
            subject: subject(),
            revision_ref: OpaqueId::new("sha256:a".to_string()).unwrap(),
            champion_revision_ref: None,
            evaluation_set_digest: Digest256::sha256(b"set"),
            evaluator_digest: Digest256::sha256(PLANNER_EVALUATOR.as_bytes()),
            evaluator_kind: GraderKind::TraceMetric,
            held_out_trajectory_ids: BoundedVec::default(),
            metrics: EvaluationMetrics::default(),
            safety: SafetyOutcome::Passed,
            verdict: EvaluationVerdict::Rejected,
            report: None,
        }
    }
}
