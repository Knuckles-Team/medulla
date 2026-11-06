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
use medulla_protocol_types::protocol::Response;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_types::claims::ClaimOp;

/// Serve one claim operation.
pub async fn handle_claims(
    state: &SharedState,
    req_id: u64,
    verified: &VerifiedRequestContext,
    op: ClaimOp,
) -> Response {
    #[cfg(feature = "statechart")]
    {
        let persist_dir = { state.read().await.persist_dir.clone() };
        let served = served::ClaimStore::open(
            persist_dir.as_deref(),
            verified.tenant(),
            &verified.principal_persistence_id(),
        )
        .and_then(|claims| claims.serve(op));
        match served {
            Ok(payload) => Response::ok(req_id, payload),
            Err(error) => Response::err(req_id, error),
        }
    }
    #[cfg(not(feature = "statechart"))]
    {
        let _ = (state, verified, op);
        Response::err(req_id, "Claims requires the `statechart` feature")
    }
}

#[cfg(feature = "statechart")]
pub(super) mod served {
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Arc;

    use serde::de::DeserializeOwned;
    use sha2::{Digest, Sha256};

    use medulla_epistemic::claims::{
        check_transition, combine_evidence, compile_artifact, contradiction_pairs,
        feed_is_relevant, grade_feed, verify_seal, ClaimKnobs,
    };
    use medulla_protocol_types::protocol::ResultPayload;
    use medulla_protocol_types::result_contract::coordination::{
        ClaimsAdvance, ClaimsCompileArtifact, ClaimsGet, ClaimsPropose, ClaimsRecordEvidence,
    };
    use medulla_statechart::{
        instance_id_for_request, Context, EventInput, MachineInstance, StatechartError,
        StatechartStore,
    };
    use medulla_store::claim_statechart::{claim_statechart_def, research_artifact_statechart_def};
    use medulla_types::claims::{
        ClaimGetResult, ClaimOp, ClaimRecord, ClaimState, Evidence, FeedGrade, LifecycleTransition,
        ResearchArtifact, Stance, WEIGHT_PPM_MAX,
    };
    use medulla_types::contract::ServerErrorCode;

    /// Domain of the instance keys.
    const KEY_DOMAIN: &str = "medulla-claims/v1";

    /// The `"CODE: detail"` text of one refusal.
    fn refuse(code: ServerErrorCode, detail: impl std::fmt::Display) -> String {
        format!("{}: {detail}", code.as_str())
    }

    fn invalid(detail: &str) -> String {
        format!("PARAMETER_INVALID: {detail}")
    }

    /// The opaque request key an instance is created under.
    fn request_key(kind: &str, tenant: &str, id: &str) -> String {
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
    fn claim_knobs() -> ClaimKnobs {
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

    fn field<T: DeserializeOwned + Default>(context: &Context, key: &str) -> Result<T, String> {
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
        Ok(ClaimRecord {
            tenant_id: field(context, "tenant_id")?,
            claim_id: field(context, "claim_id")?,
            state: state_of(instance)?,
            log_odds_micro: field(context, "log_odds_micro")?,
            transitions: field(context, "transitions")?,
            evidence: field(context, "evidence")?,
            stances: field(context, "stances")?,
            contradiction_pairs: field(context, "contradiction_pairs")?,
            feeds: field(context, "feeds")?,
        })
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
            })
        }

        /// Run one operation to its typed result.
        pub(in crate::decide) fn serve(&self, op: ClaimOp) -> Result<ResultPayload, String> {
            match op {
                ClaimOp::Propose { claim_id } => {
                    payload_of::<ClaimsPropose>(self.propose(&claim_id)?)
                }
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
                    stances,
                    feed,
                } => {
                    let evidence = self.evidence_of(
                        evidence_id,
                        claim_id,
                        channel,
                        supports,
                        weight_ppm,
                        source_digest,
                    )?;
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
                ClaimOp::Get {
                    claim_id,
                    artifact_id,
                } => {
                    payload_of::<ClaimsGet>(self.get(claim_id.as_deref(), artifact_id.as_deref())?)
                }
            }
        }

        fn now_ms() -> u64 {
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
            let outcome = self
                .store
                .send_event(
                    &instance.instance_id,
                    &event,
                    Some(instance.version),
                    Self::now_ms() as i64,
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

        fn propose(&self, claim_id: &str) -> Result<ClaimRecord, String> {
            if claim_id.is_empty() {
                return Err(invalid("claim_id is empty"));
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
            record_of(&self.send(&instance, event)?)
        }

        fn evidence_of(
            &self,
            evidence_id: String,
            claim_id: String,
            channel: Option<medulla_types::claims::EvidenceChannel>,
            supports: bool,
            weight_ppm: u32,
            source_digest: String,
        ) -> Result<Evidence, String> {
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
            Ok(Evidence {
                tenant_id: self.tenant.clone(),
                evidence_id,
                claim_id,
                channel,
                supports,
                weight_ppm,
                source_digest,
            })
        }

        fn record_evidence(
            &self,
            evidence: Evidence,
            new_stances: BTreeMap<String, Stance>,
            feed: Option<medulla_types::claims::FeedItem>,
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

        fn get(
            &self,
            claim_id: Option<&str>,
            artifact_id: Option<&str>,
        ) -> Result<ClaimGetResult, String> {
            match (claim_id, artifact_id) {
                (Some(claim_id), None) => Ok(ClaimGetResult {
                    claim: Some(record_of(&self.require("claim", claim_id)?)?),
                    artifact: None,
                }),
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
