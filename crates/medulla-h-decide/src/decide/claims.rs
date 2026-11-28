//! `Method::Claims`: research claims, their evidence and sealed artifacts
//! (MEDULLA-DECISION-ENGINE-R652.3 .. .8).
//!
//! This handler is the single writer of claim state. A claim is one instance
//! of the claim chart in the `medulla-statechart` store
//! (`medulla_store::claim_statechart`); a sealed artifact is one instance of
//! the one-state artifact chart. Both instance ids derive from the verified
//! tenant and the caller's id, so a read under another tenant derives another
//! id and answers `CLAIM_NOT_FOUND`. The kernels live in
//! `medulla_epistemic::claims`; the handler computes the guard booleans and
//! passes them in the event payload. Every refusal returns before the store
//! write.
//!
//! A build without the `statechart` feature answers a typed refusal naming it.

use super::SharedState;
use medulla_decision_statistical_types::claims::ClaimOp;
use medulla_protocol_types::protocol::Response;
use medulla_server_api::authority_context::VerifiedRequestContext;

/// One served claim operation and the memory events its commit announces
/// (MEDULLA-DECISION-ENGINE-R073.6). The events are announced only after the claim store has
/// committed: the caller writes them to the graph outbox once `response` is a success.
pub struct ClaimsServed {
    /// The operation's answer.
    pub response: Response,
    /// One `memory.claim.changed` event per committed lifecycle transition of a claim that has a
    /// perspective; empty for every other operation and for any refusal.
    pub events: Vec<medulla_decision_statistical_types::memory_events::MemoryEvent>,
}

/// Serve one claim operation, dropping the memory events (a caller with no graph outbox to
/// write them to).
pub async fn handle_claims(
    state: &SharedState,
    req_id: u64,
    graph: &str,
    verified: &VerifiedRequestContext,
    op: ClaimOp,
) -> Response {
    handle_claims_planned(state, req_id, graph, verified, op)
        .await
        .response
}

/// Serve one claim operation and return the memory events it committed.
pub async fn handle_claims_planned(
    state: &SharedState,
    req_id: u64,
    graph: &str,
    verified: &VerifiedRequestContext,
    op: ClaimOp,
) -> ClaimsServed {
    let refused = |error: String| ClaimsServed {
        response: Response::err(req_id, error),
        events: Vec::new(),
    };
    #[cfg(feature = "statechart")]
    {
        let persist_dir = { state.read().await.persist_dir.clone() };
        // A perspective names participants of the request graph; a read is judged in the view the
        // verified principal holds there (MEDULLA-DECISION-ENGINE-R652.9).
        let graph_view = match &op {
            ClaimOp::Propose {
                perspective: Some(_),
                ..
            }
            | ClaimOp::Get {
                claim_id: Some(_), ..
            }
            | ClaimOp::ProvenClaims { .. } => participants_of(state, graph).await,
            _ => None,
        };
        let checked = match (&op, graph_view.as_ref()) {
            (
                ClaimOp::Propose {
                    perspective: Some(perspective),
                    ..
                },
                view,
            ) => check_perspective(view, verified.tenant(), perspective),
            _ => Ok(()),
        };
        if let Err(error) = checked {
            return refused(error);
        }
        let reader = match &op {
            ClaimOp::Get {
                claim_id: Some(_),
                as_observer,
                ..
            } => match reader_view(graph_view.as_ref(), verified, as_observer.as_ref()) {
                Ok(view) => Some(view),
                Err(error) => return refused(error),
            },
            // A list is read in the view the verified principal holds, like a `Get`; the claims
            // it may not see are omitted (MEDULLA-DECISION-ENGINE-R652.13).
            ClaimOp::ProvenClaims { .. } => {
                match reader_view(graph_view.as_ref(), verified, None) {
                    Ok(view) => Some(view),
                    Err(error) => return refused(error),
                }
            }
            _ => None,
        };
        let served = served::ClaimStore::open(
            persist_dir.as_deref(),
            verified.tenant(),
            &verified.principal_persistence_id(),
        )
        .map(|claims| claims.in_graph(graph))
        .and_then(|claims| {
            let payload = claims.serve(op, reader.as_ref())?;
            Ok((payload, claims.take_events()))
        });
        match served {
            Ok((payload, events)) => ClaimsServed {
                response: Response::ok(req_id, payload),
                events,
            },
            Err(error) => refused(error),
        }
    }
    #[cfg(not(feature = "statechart"))]
    {
        let _ = (state, graph, verified, op);
        refused("Claims requires the `statechart` feature".to_string())
    }
}

/// The core of the request graph, or `None` when the server holds no such graph (which holds no
/// participant either).
#[cfg(feature = "statechart")]
pub(super) async fn participants_of(
    state: &SharedState,
    graph: &str,
) -> Option<std::sync::Arc<medulla_graph_core::graph::GraphCore>> {
    state
        .read()
        .await
        .registry
        .get(graph)
        .map(|entry| entry.core.clone())
}

/// Refuse a perspective that names a participant outside the caller's tenant
/// (`PARTICIPANT_NOT_VISIBLE`, the envelope an absent participant gets).
#[cfg(feature = "statechart")]
fn check_perspective(
    core: Option<&std::sync::Arc<medulla_graph_core::graph::GraphCore>>,
    tenant: &str,
    perspective: &medulla_decision_statistical_types::claims::ClaimPerspective,
) -> Result<(), String> {
    use medulla_decision_statistical_types::claims::ObserverRef;
    use {
        medulla_h_graph_gateway::graph_ops::participant_surface::participant_exists,
        medulla_h_graph_gateway::graph_ops::participant_surface::participant_unavailable,
    };
    let named = std::iter::once(&perspective.subject).chain(match &perspective.observer {
        ObserverRef::Participant(observer) => Some(observer),
        ObserverRef::System => None,
    });
    for slug in named {
        if !core.is_some_and(|core| participant_exists(core, tenant, slug)) {
            return Err(participant_unavailable(slug.as_str()));
        }
    }
    Ok(())
}

/// The view the verified principal reads claims in: the participant it acts as and the sessions
/// that participant belongs to, read from the request graph. A principal that acts as no
/// participant reads as the shared system observer over no session.
#[cfg(feature = "statechart")]
fn reader_view(
    core: Option<&std::sync::Arc<medulla_graph_core::graph::GraphCore>>,
    verified: &VerifiedRequestContext,
    requested: Option<&medulla_decision_statistical_types::claims::ObserverRef>,
) -> Result<medulla_epistemic::claims::ReaderView, String> {
    use medulla_h_graph_gateway::graph_ops::participant_surface::principal_view;
    let empty;
    let core = match core {
        Some(core) => core.as_ref(),
        None => {
            empty = medulla_graph_core::graph::GraphCore::new();
            &empty
        }
    };
    let view = principal_view(core, verified.tenant(), verified.agent_id(), requested)?;
    Ok(medulla_epistemic::claims::ReaderView::of(
        view.observer,
        view.sessions,
    ))
}

#[cfg(feature = "statechart")]
pub(super) mod served {
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Arc;

    use serde::de::DeserializeOwned;
    use sha2::{Digest, Sha256};

    use medulla_epistemic::claims::{
        check_transition, claim_visible_to, combine_evidence, compile_artifact,
        contradiction_pairs, feed_is_relevant, grade_feed, proof_standing, proven_rows,
        verify_seal, ClaimKnobs, ReaderView,
    };
    use medulla_kernel::lock_recovery::LockRecovery;
    use medulla_protocol_types::protocol::ResultPayload;
    use medulla_protocol_types::result_contract::coordination::{
        ClaimsAdvance, ClaimsCompileArtifact, ClaimsGet, ClaimsPropose, ClaimsProvenClaims,
        ClaimsRecordEvidence,
    };
    use medulla_statechart::{
        instance_id_for_request, Context, EventInput, MachineInstance, StatechartError,
        StatechartStore,
    };
    use medulla_store::claim_statechart::{claim_statechart_def, research_artifact_statechart_def};
    use medulla_types::contract::ServerErrorCode;
    use medulla_work_types::participant::ParticipantId;
    use {
        medulla_decision_statistical_types::claims::ClaimGetResult,
        medulla_decision_statistical_types::claims::ClaimOp,
        medulla_decision_statistical_types::claims::ClaimRecord,
        medulla_decision_statistical_types::claims::ClaimState,
        medulla_decision_statistical_types::claims::Evidence,
        medulla_decision_statistical_types::claims::FeedGrade,
        medulla_decision_statistical_types::claims::LifecycleTransition,
        medulla_decision_statistical_types::claims::ProvenClaimsResult,
        medulla_decision_statistical_types::claims::ResearchArtifact,
        medulla_decision_statistical_types::claims::Stance,
        medulla_decision_statistical_types::claims::PROVEN_CLAIMS_DEFAULT_LIMIT,
        medulla_decision_statistical_types::claims::PROVEN_CLAIMS_MAX_LIMIT,
        medulla_decision_statistical_types::claims::WEIGHT_PPM_MAX,
    };
    use {
        medulla_decision_statistical_types::claims::ClaimPerspective,
        medulla_decision_statistical_types::claims::ObserverRef,
    };
    use {
        medulla_decision_statistical_types::memory_events::ClaimChanged,
        medulla_decision_statistical_types::memory_events::MemoryEvent,
    };

    /// Domain of the instance keys.
    const KEY_DOMAIN: &str = "medulla-claims/v1";

    /// The `"CODE: detail"` text of one refusal.
    pub(in crate::decide) fn refuse(
        code: ServerErrorCode,
        detail: impl std::fmt::Display,
    ) -> String {
        format!("{}: {detail}", code.as_str())
    }

    pub(in crate::decide) fn invalid(detail: &str) -> String {
        format!("PARAMETER_INVALID: {detail}")
    }

    /// The opaque request key an instance is created under.
    pub(in crate::decide) fn request_key(kind: &str, tenant: &str, id: &str) -> String {
        let mut hasher = Sha256::new();
        for part in [KEY_DOMAIN, kind, tenant, id] {
            hasher.update((part.len() as u64).to_be_bytes());
            hasher.update(part.as_bytes());
        }
        hex::encode(hasher.finalize())
    }

    fn knob_u32(value: u64) -> u32 {
        u32::try_from(value).unwrap_or(u32::MAX)
    }

    /// The claim knobs loaded at startup.
    pub(in crate::decide) fn claim_knobs() -> ClaimKnobs {
        let knobs = medulla_server_api::knob_config::knobs();
        ClaimKnobs {
            weight_cap_ppm: knob_u32(knobs.claims_weight_cap_ppm),
            prior_outcome: knob_u32(knobs.claims_prior_outcome),
            prior_optimization: knob_u32(knobs.claims_prior_optimization),
            prior_health: knob_u32(knobs.claims_prior_health),
            prior_process: knob_u32(knobs.claims_prior_process),
            prior_mined: knob_u32(knobs.claims_prior_mined),
            support_min_micro: knobs.claims_support_min_micro,
            accept_min_micro: knobs.claims_accept_min_micro,
            contest_max_micro: knobs.claims_contest_max_micro,
            feed_relevance_min: knob_u32(knobs.claims_feed_relevance_min),
        }
    }

    pub(in crate::decide) fn field<T: DeserializeOwned + Default>(
        context: &Context,
        key: &str,
    ) -> Result<T, String> {
        match context.get(key) {
            None | Some(serde_json::Value::Null) => Ok(T::default()),
            Some(value) => serde_json::from_value(value.clone())
                .map_err(|error| format!("claim record field {key} is unreadable: {error}")),
        }
    }

    fn state_of(instance: &MachineInstance) -> Result<ClaimState, String> {
        let active = instance
            .configuration
            .active
            .iter()
            .next()
            .ok_or_else(|| "claim instance has no active state".to_string())?;
        serde_json::from_value(serde_json::json!(active))
            .map_err(|_| format!("claim instance is in unknown state {active}"))
    }

    fn record_of(instance: &MachineInstance) -> Result<ClaimRecord, String> {
        let context = &instance.context;
        let evidence: Vec<Evidence> = field(context, "evidence")?;
        Ok(ClaimRecord {
            tenant_id: field(context, "tenant_id")?,
            claim_id: field(context, "claim_id")?,
            perspective: perspective_of(context)?,
            state: state_of(instance)?,
            log_odds_micro: field(context, "log_odds_micro")?,
            transitions: field(context, "transitions")?,
            stances: field(context, "stances")?,
            contradiction_pairs: field(context, "contradiction_pairs")?,
            feeds: field(context, "feeds")?,
            // Derived on every read (MEDULLA-DECISION-ENGINE-R652.11), never stored.
            proof_standing: proof_standing(&evidence),
            evidence,
        })
    }

    /// The perspective stored as the claim's `subject` and `observer` properties; absent when the
    /// claim was proposed without one.
    fn perspective_of(context: &Context) -> Result<Option<ClaimPerspective>, String> {
        let subject: Option<String> = field(context, "subject")?;
        let observer: Option<String> = field(context, "observer")?;
        let statement: Option<String> = field(context, "statement")?;
        match (subject, observer) {
            (None, None) => Ok(None),
            (Some(subject), Some(observer)) => Ok(Some(ClaimPerspective {
                subject: ParticipantId::new(subject).map_err(|error| error.to_string())?,
                observer: ObserverRef::from_property(&observer).ok_or_else(|| {
                    format!(
                        "claim record observer {observer:?} is neither system nor a participant"
                    )
                })?,
                statement,
            })),
            _ => Err("claim record holds only one of subject and observer".to_string()),
        }
    }

    fn payload_of<M>(body: M::Body) -> Result<ResultPayload, String>
    where
        M: medulla_protocol_types::result_contract::MethodResult,
    {
        ResultPayload::of::<M>(body)
    }

    /// The claim store of one verified principal.
    pub(in crate::decide) struct ClaimStore {
        store: Arc<StatechartStore>,
        claim_def: String,
        artifact_def: String,
        tenant: String,
        actor: String,
        knobs: ClaimKnobs,
        /// The graph the store's events are published into; empty for a read-only caller.
        graph: String,
        /// Events of the transitions this store committed, in order.
        events:
            std::sync::Mutex<Vec<medulla_decision_statistical_types::memory_events::MemoryEvent>>,
    }

    /// The fields of one evidence record as a caller sent them, before they are validated.
    struct EvidenceDraft {
        evidence_id: String,
        claim_id: String,
        channel: Option<medulla_decision_statistical_types::claims::EvidenceChannel>,
        supports: bool,
        weight_ppm: u32,
        source_digest: String,
        session_id: Option<String>,
        proof: Option<medulla_decision_statistical_types::claims::ProofReceipt>,
    }

    impl ClaimStore {
        /// Open the shared statechart store and register both charts.
        pub(in crate::decide) fn open(
            persist_dir: Option<&str>,
            tenant: &str,
            actor: &str,
        ) -> Result<Self, String> {
            let store = medulla_h_jobs::statechart::statechart_store(persist_dir)?;
            let claim_def = store
                .define(&claim_statechart_def())
                .map_err(|error| error.to_string())?;
            let artifact_def = store
                .define(&research_artifact_statechart_def())
                .map_err(|error| error.to_string())?;
            Ok(Self {
                store,
                claim_def,
                artifact_def,
                tenant: tenant.to_string(),
                actor: actor.to_string(),
                knobs: claim_knobs(),
                graph: String::new(),
                events: std::sync::Mutex::new(Vec::new()),
            })
        }

        /// The store of a caller whose transitions announce events into `graph`.
        pub(in crate::decide) fn in_graph(mut self, graph: &str) -> Self {
            self.graph = graph.to_string();
            self
        }

        /// The memory events of the transitions committed so far, taken once.
        pub(in crate::decide) fn take_events(
            &self,
        ) -> Vec<medulla_decision_statistical_types::memory_events::MemoryEvent> {
            std::mem::take(&mut *self.events.lock_recovering("claim memory events"))
        }

        /// Run one operation to its typed result.
        pub(in crate::decide) fn serve(
            &self,
            op: ClaimOp,
            reader: Option<&ReaderView>,
        ) -> Result<ResultPayload, String> {
            match op {
                ClaimOp::Propose {
                    claim_id,
                    perspective,
                } => payload_of::<ClaimsPropose>(self.propose(&claim_id, perspective.as_ref())?),
                ClaimOp::Advance { claim_id, to } => {
                    payload_of::<ClaimsAdvance>(self.advance(&claim_id, to)?)
                }
                ClaimOp::RecordEvidence {
                    evidence_id,
                    claim_id,
                    channel,
                    supports,
                    weight_ppm,
                    source_digest,
                    session_id,
                    stances,
                    feed,
                    proof,
                } => {
                    let evidence = self.evidence_of(EvidenceDraft {
                        evidence_id,
                        claim_id,
                        channel,
                        supports,
                        weight_ppm,
                        source_digest,
                        session_id,
                        proof,
                    })?;
                    payload_of::<ClaimsRecordEvidence>(
                        self.record_evidence(evidence, stances, feed)?,
                    )
                }
                ClaimOp::CompileArtifact {
                    artifact_id,
                    cohort_id,
                    claim_ids,
                    code_specs,
                    exploration_edges,
                } => payload_of::<ClaimsCompileArtifact>(self.compile(
                    &artifact_id,
                    &cohort_id,
                    &claim_ids,
                    &code_specs,
                    &exploration_edges,
                )?),
                ClaimOp::ProvenClaims { axiom, limit } => payload_of::<ClaimsProvenClaims>(
                    self.proven_claims(axiom.as_deref(), limit, reader)?,
                ),
                ClaimOp::Get {
                    claim_id,
                    artifact_id,
                    as_observer: _,
                } => payload_of::<ClaimsGet>(self.get(
                    claim_id.as_deref(),
                    artifact_id.as_deref(),
                    reader,
                )?),
            }
        }

        pub(in crate::decide) fn now_ms() -> u64 {
            medulla_kernel::kernel::dispatch_support::authoritative_now_ms()
        }

        /// The instance of `kind`/`id` in this tenant, if any.
        fn find(&self, kind: &str, id: &str) -> Result<Option<MachineInstance>, String> {
            let instance_id = instance_id_for_request(&request_key(kind, &self.tenant, id));
            match self.store.get_instance(&instance_id) {
                Ok(instance) if instance.tenant == self.tenant => Ok(Some(instance)),
                Ok(_) | Err(StatechartError::NotFound(_)) => Ok(None),
                Err(error) => Err(error.to_string()),
            }
        }

        fn require(&self, kind: &str, id: &str) -> Result<MachineInstance, String> {
            self.find(kind, id)?
                .ok_or_else(|| refuse(ServerErrorCode::ClaimNotFound, id))
        }

        fn create(
            &self,
            kind: &str,
            id: &str,
            def_id: &str,
            context: serde_json::Value,
        ) -> Result<MachineInstance, String> {
            let (instance, _replayed) = self
                .store
                .instantiate_batch(
                    def_id,
                    Context::from_value(context)?,
                    &self.tenant,
                    &self.actor,
                    &request_key(kind, &self.tenant, id),
                    Self::now_ms(),
                )
                .map_err(|error| error.to_string())?;
            Ok(instance)
        }

        /// Deliver one event under the instance's version; a refused or
        /// conflicting event writes nothing.
        fn send(
            &self,
            instance: &MachineInstance,
            event: EventInput,
        ) -> Result<MachineInstance, String> {
            self.send_with_outbox(instance, event, &[])
        }

        /// [`Self::send`] with outbox intents that commit in the same transaction as the event
        /// (MEDULLA-DECISION-ENGINE-R073.6).
        fn send_with_outbox(
            &self,
            instance: &MachineInstance,
            event: EventInput,
            outbox: &[medulla_mutation_types::mutation_scope::MutationOutboxIntent],
        ) -> Result<MachineInstance, String> {
            let outcome = self
                .store
                .send_event_with_outbox(
                    &instance.instance_id,
                    &event,
                    Some(instance.version),
                    Self::now_ms() as i64,
                    outbox,
                )
                .map_err(|error| match error {
                    StatechartError::VersionConflict { .. } => {
                        refuse(ServerErrorCode::RevisionConflict, error)
                    }
                    other => other.to_string(),
                })?;
            if !outcome.outcome.fired {
                return Err("claim event did not fire".to_string());
            }
            Ok(outcome.instance)
        }

        fn propose(
            &self,
            claim_id: &str,
            perspective: Option<&ClaimPerspective>,
        ) -> Result<ClaimRecord, String> {
            if claim_id.is_empty() {
                return Err(invalid("claim_id is empty"));
            }
            if let Some(statement) = perspective.and_then(|p| p.statement.as_deref()) {
                if statement.trim().is_empty()
                    || statement.chars().count()
                        > medulla_decision_statistical_types::claims::CLAIM_STATEMENT_MAX_CHARS
                {
                    return Err(invalid(
                        "a claim statement is empty or over its character limit",
                    ));
                }
            }
            if let Some(existing) = self.find("claim", claim_id)? {
                return record_of(&existing);
            }
            let instance = self.create(
                "claim",
                claim_id,
                &self.claim_def,
                serde_json::json!({
                    "tenant_id": self.tenant,
                    "claim_id": claim_id,
                    "subject": perspective.map(|p| p.subject.as_str()),
                    "observer": perspective.map(|p| p.observer.as_property()),
                    "statement": perspective.and_then(|p| p.statement.as_deref()),
                    "transitions": [],
                    "evidence": [],
                    "log_odds_micro": 0,
                    "stances": {},
                    "contradiction_pairs": [],
                    "feeds": [],
                }),
            )?;
            record_of(&instance)
        }

        fn advance(&self, claim_id: &str, to: ClaimState) -> Result<ClaimRecord, String> {
            let instance = self.require("claim", claim_id)?;
            let record = record_of(&instance)?;
            check_transition(record.state, to, record.log_odds_micro, &self.knobs).map_err(
                |code| {
                    refuse(
                        code,
                        format!(
                            "{} -> {} at log-odds {}",
                            record.state.as_str(),
                            to.as_str(),
                            record.log_odds_micro
                        ),
                    )
                },
            )?;
            let mut transitions = record.transitions;
            transitions.push(LifecycleTransition {
                tenant_id: self.tenant.clone(),
                claim_id: claim_id.to_string(),
                from: record.state,
                to,
                at_ms: Self::now_ms(),
                actor: self.actor.clone(),
            });
            let event = EventInput::with_payload(
                "advance",
                serde_json::json!({
                    "to": to.as_str(),
                    "guard_met": true,
                    "transitions": transitions,
                }),
            );
            // A claim with a perspective announces its transition; the intent joins the claim
            // store's own transaction and the event is handed to the caller for the graph outbox.
            let changed = record.perspective.as_ref().map(|perspective| {
                MemoryEvent::ClaimChanged(ClaimChanged {
                    claim_id: claim_id.to_string(),
                    from: record.state,
                    to,
                    subject: perspective.subject.clone(),
                    observer: perspective.observer.clone(),
                })
            });
            let intents = match &changed {
                Some(changed) => vec![changed.outbox_intent_in(&self.tenant, &self.graph)?],
                None => Vec::new(),
            };
            let advanced = record_of(&self.send_with_outbox(&instance, event, &intents)?)?;
            if let Some(changed) = changed {
                self.events
                    .lock_recovering("claim memory events")
                    .push(changed);
            }
            Ok(advanced)
        }

        fn evidence_of(&self, draft: EvidenceDraft) -> Result<Evidence, String> {
            let EvidenceDraft {
                evidence_id,
                claim_id,
                channel,
                supports,
                weight_ppm,
                source_digest,
                session_id,
                proof,
            } = draft;
            let Some(channel) = channel else {
                return Err(refuse(ServerErrorCode::EvidenceNoChannel, &evidence_id));
            };
            if weight_ppm > WEIGHT_PPM_MAX {
                return Err(refuse(
                    ServerErrorCode::EvidenceWeightOutOfRange,
                    format!("{weight_ppm} ppm"),
                ));
            }
            if evidence_id.is_empty() {
                return Err(invalid("evidence_id is empty"));
            }
            let evidence = Evidence {
                tenant_id: self.tenant.clone(),
                evidence_id,
                claim_id,
                channel,
                supports,
                weight_ppm,
                source_digest,
                session_id,
                proof: proof
                    .map(medulla_decision_statistical_types::claims::ProofReceipt::normalised),
            };
            // A proof receipt is checked before any state changes: a refusal
            // returns here and records nothing.
            if evidence.proof.is_some() {
                evidence
                    .check()
                    .map_err(|code| refuse(code, &evidence.evidence_id))?;
            }
            Ok(evidence)
        }

        fn record_evidence(
            &self,
            evidence: Evidence,
            new_stances: BTreeMap<String, Stance>,
            feed: Option<medulla_decision_statistical_types::claims::FeedItem>,
        ) -> Result<ClaimRecord, String> {
            let instance = self.require("claim", &evidence.claim_id)?;
            let mut record = record_of(&instance)?;
            if record.state == ClaimState::Retired {
                return Err(refuse(
                    ServerErrorCode::ClaimIllegalTransition,
                    "a retired claim takes no evidence",
                ));
            }
            // A repeated evidence id is a replay: nothing is counted twice.
            if record
                .evidence
                .iter()
                .any(|e| e.evidence_id == evidence.evidence_id)
            {
                return Ok(record);
            }
            if let Some(feed) = feed {
                let grade_ppm = grade_feed(&feed.text, &feed.fleet_topics);
                record.feeds.push(FeedGrade {
                    evidence_id: evidence.evidence_id.clone(),
                    grade_ppm,
                    relevant: feed_is_relevant(grade_ppm, &self.knobs),
                });
            }
            record.evidence.push(evidence);
            record.stances.extend(new_stances);
            record.contradiction_pairs = contradiction_pairs(&record.stances);
            // Evidence from a feed item below the relevance floor is kept but
            // does not move the log-odds.
            let irrelevant: BTreeSet<&str> = record
                .feeds
                .iter()
                .filter(|grade| !grade.relevant)
                .map(|grade| grade.evidence_id.as_str())
                .collect();
            record.log_odds_micro = combine_evidence(
                record
                    .evidence
                    .iter()
                    .filter(|e| !irrelevant.contains(e.evidence_id.as_str())),
                &self.knobs,
            );
            let event = EventInput::with_payload(
                "record_evidence",
                serde_json::json!({
                    "evidence": record.evidence,
                    "log_odds_micro": record.log_odds_micro,
                    "stances": record.stances,
                    "contradiction_pairs": record.contradiction_pairs,
                    "feeds": record.feeds,
                }),
            );
            record_of(&self.send(&instance, event)?)
        }

        fn compile(
            &self,
            artifact_id: &str,
            cohort_id: &str,
            claim_ids: &[String],
            code_specs: &[String],
            exploration_edges: &[(String, String)],
        ) -> Result<ResearchArtifact, String> {
            if artifact_id.is_empty() || cohort_id.is_empty() {
                return Err(invalid("artifact_id and cohort_id are required"));
            }
            for claim_id in claim_ids {
                self.require("claim", claim_id)?;
            }
            let artifact = compile_artifact(
                &self.tenant,
                artifact_id,
                cohort_id,
                claim_ids,
                code_specs,
                exploration_edges,
            );
            self.store_artifact(&artifact)
        }

        /// Store a sealed artifact; an id that already holds the same artifact
        /// answers it, one that holds another is a revision conflict.
        pub(in crate::decide) fn store_artifact(
            &self,
            artifact: &ResearchArtifact,
        ) -> Result<ResearchArtifact, String> {
            if let Some(existing) = self.find("artifact", &artifact.artifact_id)? {
                let stored = artifact_of(&existing)?;
                return if stored == *artifact {
                    Ok(stored)
                } else {
                    Err(refuse(
                        ServerErrorCode::RevisionConflict,
                        "the artifact id already holds a different artifact",
                    ))
                };
            }
            self.create(
                "artifact",
                &artifact.artifact_id,
                &self.artifact_def,
                serde_json::json!({
                    "tenant_id": self.tenant,
                    "artifact": artifact,
                }),
            )?;
            Ok(artifact.clone())
        }

        /// One claim of this tenant, or `None` when it does not exist
        /// (MEDULLA-DECISION-ENGINE-R700.3 reads claims through here).
        pub(in crate::decide) fn claim_record(
            &self,
            claim_id: &str,
        ) -> Result<Option<ClaimRecord>, String> {
            self.find("claim", claim_id)?
                .map(|instance| record_of(&instance))
                .transpose()
        }

        /// The claims of this tenant that hold a passing, sorry-free receipt, one row per claim
        /// and receipt, filtered by `axiom`, ordered by claim id then declaration and cut at
        /// `limit` (MEDULLA-DECISION-ENGINE-R652.13). A claim another tenant holds derives
        /// another instance and is never listed; a claim the reader's view excludes is omitted.
        fn proven_claims(
            &self,
            axiom: Option<&str>,
            limit: Option<u32>,
            reader: Option<&ReaderView>,
        ) -> Result<ProvenClaimsResult, String> {
            let limit = limit.unwrap_or(PROVEN_CLAIMS_DEFAULT_LIMIT);
            if limit > PROVEN_CLAIMS_MAX_LIMIT {
                return Err(invalid(&format!(
                    "limit {limit} is above the most a ProvenClaims answer holds ({PROVEN_CLAIMS_MAX_LIMIT})"
                )));
            }
            let view = reader.ok_or_else(|| invalid("a claim list needs a reader view"))?;
            let mut rows = Vec::new();
            for instance_id in self
                .store
                .list_instance_ids(Some(&self.claim_def))
                .map_err(|error| error.to_string())?
            {
                let instance = match self.store.get_instance(&instance_id) {
                    Ok(instance) if instance.tenant == self.tenant => instance,
                    Ok(_) | Err(StatechartError::NotFound(_)) => continue,
                    Err(error) => return Err(error.to_string()),
                };
                let record = record_of(&instance)?;
                if !claim_visible_to(&record, view) {
                    continue;
                }
                rows.extend(
                    proven_rows(&record).into_iter().filter(|row| {
                        axiom.is_none_or(|name| row.axioms.iter().any(|a| a == name))
                    }),
                );
            }
            rows.sort_by(|a, b| {
                (
                    &a.claim_id,
                    &a.declaration,
                    &a.artifact_digest,
                    &a.checker_id,
                )
                    .cmp(&(
                        &b.claim_id,
                        &b.declaration,
                        &b.artifact_digest,
                        &b.checker_id,
                    ))
            });
            rows.truncate(limit as usize);
            Ok(ProvenClaimsResult { claims: rows })
        }

        /// Every claim of the tenant that states something about `subject` as `observer` and that
        /// `reader` may see, each with the instant it last changed, in claim-id order
        /// (MEDULLA-DECISION-ENGINE-R655.2, R655.3). A claim with no statement is not about the
        /// subject in words and is left out.
        pub(in crate::decide) fn perspective_claims(
            &self,
            subject: &ParticipantId,
            observer: &ObserverRef,
            reader: &ReaderView,
        ) -> Result<Vec<(ClaimRecord, i64)>, String> {
            let mut rows = Vec::new();
            for instance_id in self
                .store
                .list_instance_ids(Some(&self.claim_def))
                .map_err(|error| error.to_string())?
            {
                let instance = match self.store.get_instance(&instance_id) {
                    Ok(instance) if instance.tenant == self.tenant => instance,
                    Ok(_) | Err(StatechartError::NotFound(_)) => continue,
                    Err(error) => return Err(error.to_string()),
                };
                let record = record_of(&instance)?;
                let about = record.perspective.as_ref().is_some_and(|p| {
                    &p.subject == subject && &p.observer == observer && p.statement.is_some()
                });
                if about && record.state != ClaimState::Retired && claim_visible_to(&record, reader)
                {
                    rows.push((record, instance.updated_at_ms));
                }
            }
            rows.sort_by(|a, b| a.0.claim_id.cmp(&b.0.claim_id));
            Ok(rows)
        }

        /// How many claims of this tenant one derivation range wrote: the claims whose id carries
        /// `prefix` (the derivation key) and that cite at least one of `turn_ids` as evidence
        /// (MEDULLA-DECISION-ENGINE-R651.12). A claim a later range of the same key wrote cites
        /// other turns and is not counted.
        pub(in crate::decide) fn derived_claim_count(
            &self,
            prefix: &str,
            turn_ids: &BTreeSet<&str>,
        ) -> Result<u32, String> {
            use medulla_decision_stats_records::decision::participant_derive::cited_turn;
            let mut count = 0u32;
            for instance_id in self
                .store
                .list_instance_ids(Some(&self.claim_def))
                .map_err(|error| error.to_string())?
            {
                let instance = match self.store.get_instance(&instance_id) {
                    Ok(instance) if instance.tenant == self.tenant => instance,
                    Ok(_) | Err(StatechartError::NotFound(_)) => continue,
                    Err(error) => return Err(error.to_string()),
                };
                let record = record_of(&instance)?;
                let cites = record.evidence.iter().any(|e| {
                    cited_turn(&e.source_digest).is_some_and(|turn| turn_ids.contains(turn))
                });
                if record.claim_id.starts_with(prefix) && cites {
                    count = count.saturating_add(1);
                }
            }
            Ok(count)
        }

        fn get(
            &self,
            claim_id: Option<&str>,
            artifact_id: Option<&str>,
            reader: Option<&ReaderView>,
        ) -> Result<ClaimGetResult, String> {
            match (claim_id, artifact_id) {
                (Some(claim_id), None) => {
                    let record = record_of(&self.require("claim", claim_id)?)?;
                    let view = reader.ok_or_else(|| invalid("a claim read needs a reader view"))?;
                    // A claim the reader's view excludes reads like an absent participant; another
                    // tenant's claim never reaches here (instance ids derive from the tenant).
                    if !claim_visible_to(&record, view) {
                        return Err(format!(
                            "PARTICIPANT_NOT_VISIBLE: claim {claim_id} is outside the reader's view"
                        ));
                    }
                    Ok(ClaimGetResult {
                        claim: Some(record),
                        artifact: None,
                    })
                }
                (None, Some(artifact_id)) => {
                    let artifact = artifact_of(&self.require("artifact", artifact_id)?)?;
                    verify_seal(&artifact).map_err(|code| refuse(code, artifact_id))?;
                    Ok(ClaimGetResult {
                        claim: None,
                        artifact: Some(artifact),
                    })
                }
                _ => Err(invalid("Get names exactly one of claim_id and artifact_id")),
            }
        }
    }

    fn artifact_of(instance: &MachineInstance) -> Result<ResearchArtifact, String> {
        match instance.context.get("artifact") {
            Some(value) => serde_json::from_value(value.clone())
                .map_err(|error| format!("stored artifact is unreadable: {error}")),
            None => Err("stored artifact is missing".to_string()),
        }
    }
}
