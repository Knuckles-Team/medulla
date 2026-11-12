//! `Method::BeliefRevise`: belief revision over claims whose evidence changed
//! (MEDULLA-DECISION-ENGINE-R700.3, R700.4, R700.7).
//!
//! This handler is the single writer of `BeliefRevision`, `FrictionFinding`
//! and `ContradictionRecord`. The durable state of one graph is one instance of
//! the belief ledger chart (`medulla_store::belief_statechart`) in the
//! `medulla-statechart` store, keyed by the verified tenant and the graph, so
//! passes on different graphs never contend and passes on one graph are
//! serialised by the instance version. A batch that touched claims names them
//! in its outbox intent (`TOUCHED_CLAIMS_HEADER`); [`enqueue_touched`] queues
//! them on the ledger and the next pass drains the queue.
//!
//! The kernels live in `medulla_epistemic::belief_revision`. Every refusal
//! returns before the ledger write, so a refusal changes no durable state. A
//! revision below the retraction threshold yields a `RetractionProposal`; the
//! handler never writes a claim. A detected contradiction is persisted as a
//! `ContradictionRecord` and also asserted as a `CONTRADICTS` edge between the
//! two claim nodes of the named graph when both nodes exist there.
//!
//! A build without the `statechart` feature answers a typed refusal naming it.

use super::SharedState;
use medulla_protocol_types::protocol::Response;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_types::belief_revision::BeliefReviseRequest;

/// Serve one belief pass.
pub async fn handle_belief_revise(
    state: &SharedState,
    req_id: u64,
    verified: &VerifiedRequestContext,
    request: BeliefReviseRequest,
) -> Response {
    #[cfg(feature = "statechart")]
    {
        let target = match served::graph_target(state, verified, &request.graph).await {
            Ok(target) => target,
            Err(error) => return Response::err(req_id, error),
        };
        let served = served::BeliefLedger::open(
            target.persist_dir.as_deref(),
            verified.tenant(),
            &verified.principal_persistence_id(),
            &request.graph,
        )
        .and_then(|ledger| ledger.revise(request, target.core.as_deref()));
        match served {
            Ok(payload) => Response::ok(req_id, payload),
            Err(error) => Response::err(req_id, error),
        }
    }
    #[cfg(not(feature = "statechart"))]
    {
        let _ = (state, verified, request);
        Response::err(req_id, "BeliefRevise requires the `statechart` feature")
    }
}

/// Queue the claims a committed batch touched on the belief ledger of `graph`
/// (the per-graph job of MEDULLA-DECISION-ENGINE-R700.3). Returns how many
/// claims are waiting after the call.
#[cfg(feature = "statechart")]
pub fn enqueue_touched(
    persist_dir: Option<&str>,
    tenant: &str,
    actor: &str,
    graph: &str,
    intent: &medulla_mutation_types::mutation_scope::MutationOutboxIntent,
) -> Result<usize, String> {
    let touched = intent.touched_claims();
    served::BeliefLedger::open(persist_dir, tenant, actor, graph)?.enqueue(&touched)
}

#[cfg(feature = "statechart")]
pub(super) mod served {
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Arc;

    use medulla_epistemic::belief_revision::{
        run_pass, BeliefKnobs, ClaimInput, EvidenceInput, TokenIndex,
    };
    use medulla_isolation::isolation::AccessLevel;
    use medulla_protocol_types::protocol::ResultPayload;
    use medulla_protocol_types::result_contract::coordination::BeliefRevise;
    use medulla_statechart::{
        instance_id_for_request, Context, EventInput, MachineInstance, StatechartError,
        StatechartStore,
    };
    use medulla_store::belief_statechart::belief_statechart_def;
    use medulla_types::belief_revision::{
        BeliefPassResult, BeliefReviseRequest, BeliefRevision, ContradictionRecord,
        ContradictionStatus, RetractionProposal,
    };
    use medulla_types::claims::{ClaimRecord, EvidenceChannel};
    use medulla_types::contract::ServerErrorCode;
    use medulla_types::evidence_class::EvidenceClass;
    use sha2::{Digest, Sha256};

    use super::super::claims::served::{
        claim_knobs, field, invalid, refuse, request_key, ClaimStore,
    };
    use super::SharedState;
    use medulla_server_api::authority_context::VerifiedRequestContext;

    /// Where a pass runs: the persistence directory and, when the named graph
    /// is resident, its core (for the `CONTRADICTS` edges).
    pub(in crate::decide) struct GraphTarget {
        pub(in crate::decide) persist_dir: Option<String>,
        pub(in crate::decide) core: Option<Arc<medulla_graph_core::graph::GraphCore>>,
    }

    /// Resolve the target of a pass. A resident graph is write-checked against
    /// the verified principal before anything else happens.
    pub(in crate::decide) async fn graph_target(
        state: &SharedState,
        verified: &VerifiedRequestContext,
        graph: &str,
    ) -> Result<GraphTarget, String> {
        if graph.is_empty() {
            return Err(invalid("graph is empty"));
        }
        let guard = state.read().await;
        let core = match guard.registry.get(graph) {
            Some(entry) => {
                medulla_server_auth::access::check_graph_access(
                    &guard.isolation,
                    Some(verified.agent_id()),
                    graph,
                    entry.graph_type,
                    entry.owner.as_deref(),
                    AccessLevel::Write,
                )?;
                Some(entry.core.clone())
            }
            None => None,
        };
        Ok(GraphTarget {
            persist_dir: guard.persist_dir.clone(),
            core,
        })
    }

    fn belief_knobs() -> BeliefKnobs {
        let knobs = medulla_server_api::knob_config::knobs();
        BeliefKnobs {
            claims: claim_knobs(),
            weight_cap_micro: u32::try_from(knobs.belief_weight_cap_micro).unwrap_or(u32::MAX),
            opposition_similarity_min: u32::try_from(knobs.belief_opposition_similarity_min)
                .unwrap_or(u32::MAX),
            retraction_threshold_micro: knobs.belief_retraction_threshold_micro,
        }
    }

    /// The class a channel's evidence is graded at.
    fn class_of(channel: EvidenceChannel) -> EvidenceClass {
        match channel {
            EvidenceChannel::Outcome => EvidenceClass::Proof,
            EvidenceChannel::Optimization | EvidenceChannel::Health | EvidenceChannel::Process => {
                EvidenceClass::Observation
            }
            EvidenceChannel::Mined => EvidenceClass::Claim,
        }
    }

    /// The durable state of one graph's belief ledger.
    #[derive(Default)]
    struct Ledger {
        pending: BTreeSet<String>,
        texts: BTreeMap<String, String>,
        revisions: BTreeMap<String, BeliefRevision>,
        contradictions: BTreeMap<String, ContradictionRecord>,
        retractions: BTreeMap<String, RetractionProposal>,
        passes: u64,
    }

    impl Ledger {
        fn of(instance: &MachineInstance) -> Result<Self, String> {
            let context = &instance.context;
            Ok(Self {
                pending: field(context, "pending")?,
                texts: field(context, "texts")?,
                revisions: field(context, "revisions")?,
                contradictions: field(context, "contradictions")?,
                retractions: field(context, "retractions")?,
                passes: field(context, "passes")?,
            })
        }

        fn pass_payload(&self) -> serde_json::Value {
            serde_json::json!({
                "pending": self.pending,
                "texts": self.texts,
                "revisions": self.revisions,
                "contradictions": self.contradictions,
                "retractions": self.retractions,
                "passes": self.passes,
            })
        }
    }

    /// The belief ledger of one `(tenant, graph)`.
    pub(in crate::decide) struct BeliefLedger {
        store: Arc<StatechartStore>,
        def_id: String,
        claims: ClaimStore,
        tenant: String,
        graph: String,
        actor: String,
    }

    impl BeliefLedger {
        pub(in crate::decide) fn open(
            persist_dir: Option<&str>,
            tenant: &str,
            actor: &str,
            graph: &str,
        ) -> Result<Self, String> {
            let store = medulla_h_jobs::statechart::statechart_store(persist_dir)?;
            let def_id = store
                .define(&belief_statechart_def())
                .map_err(|error| error.to_string())?;
            Ok(Self {
                store,
                def_id,
                claims: ClaimStore::open(persist_dir, tenant, actor)?,
                tenant: tenant.to_string(),
                graph: graph.to_string(),
                actor: actor.to_string(),
            })
        }

        fn key(&self) -> String {
            request_key("belief_ledger", &self.tenant, &self.graph)
        }

        /// The ledger instance if one was ever written. Reading never creates it,
        /// so a refused pass leaves no trace.
        fn existing(&self) -> Result<Option<MachineInstance>, String> {
            match self
                .store
                .get_instance(&instance_id_for_request(&self.key()))
            {
                Ok(instance) if instance.tenant == self.tenant => Ok(Some(instance)),
                Ok(_) | Err(StatechartError::NotFound(_)) => Ok(None),
                Err(error) => Err(error.to_string()),
            }
        }

        /// The ledger state and the instance it was read from, if any.
        fn load(&self) -> Result<(Option<MachineInstance>, Ledger), String> {
            let existing = self.existing()?;
            let ledger = match &existing {
                Some(instance) => Ledger::of(instance)?,
                None => Ledger::default(),
            };
            Ok((existing, ledger))
        }

        /// The ledger instance to write to, created empty on first write.
        fn instance_for_write(
            &self,
            existing: Option<MachineInstance>,
        ) -> Result<MachineInstance, String> {
            match existing {
                Some(instance) => Ok(instance),
                None => self.create(),
            }
        }

        fn create(&self) -> Result<MachineInstance, String> {
            let (instance, _replayed) = self
                .store
                .instantiate_batch(
                    &self.def_id,
                    Context::from_value(serde_json::json!({
                        "tenant_id": self.tenant,
                        "graph": self.graph,
                        "pending": [],
                        "texts": {},
                        "revisions": {},
                        "contradictions": {},
                        "retractions": {},
                        "passes": 0,
                    }))?,
                    &self.tenant,
                    &self.actor,
                    &self.key(),
                    ClaimStore::now_ms(),
                )
                .map_err(|error| error.to_string())?;
            Ok(instance)
        }

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
                    ClaimStore::now_ms() as i64,
                )
                .map_err(|error| match error {
                    StatechartError::VersionConflict { .. } => {
                        refuse(ServerErrorCode::RevisionConflict, error)
                    }
                    other => other.to_string(),
                })?;
            if !outcome.outcome.fired {
                return Err("belief ledger event did not fire".to_string());
            }
            Ok(outcome.instance)
        }

        /// Queue `claims` for the next pass; returns the queue length.
        pub(in crate::decide) fn enqueue(&self, claims: &[String]) -> Result<usize, String> {
            let (existing, mut ledger) = self.load()?;
            let before = ledger.pending.len();
            ledger
                .pending
                .extend(claims.iter().filter(|claim| !claim.is_empty()).cloned());
            if ledger.pending.len() == before {
                return Ok(before);
            }
            let event = EventInput::with_payload(
                "enqueue",
                serde_json::json!({ "pending": ledger.pending }),
            );
            self.send(&self.instance_for_write(existing)?, event)?;
            Ok(ledger.pending.len())
        }

        fn pass_id(&self, ids: &BTreeSet<String>, sequence: u64) -> String {
            let mut hasher = Sha256::new();
            for part in ["medulla-belief-pass/v1", &self.tenant, &self.graph] {
                hasher.update((part.len() as u64).to_be_bytes());
                hasher.update(part.as_bytes());
            }
            hasher.update(sequence.to_be_bytes());
            for id in ids {
                hasher.update((id.len() as u64).to_be_bytes());
                hasher.update(id.as_bytes());
            }
            hex::encode(hasher.finalize())[..32].to_string()
        }

        /// Run one pass: the queued claims plus the request's, then the
        /// requested contradiction moves. Nothing is written until every
        /// refusal has been ruled out.
        pub(in crate::decide) fn revise(
            &self,
            request: BeliefReviseRequest,
            core: Option<&medulla_graph_core::graph::GraphCore>,
        ) -> Result<ResultPayload, String> {
            let (existing, mut ledger) = self.load()?;
            let ids: BTreeSet<String> = ledger
                .pending
                .iter()
                .cloned()
                .chain(request.claim_ids.iter().cloned())
                .collect();
            if ids.is_empty() && request.advance.is_empty() {
                return Err(invalid("a pass names no claim and no contradiction move"));
            }
            let records = self.load_claims(&ids)?;
            self.check_premises(&request, &ids)?;
            let moves = self.check_moves(&ledger, &request)?;

            let knobs = belief_knobs();
            let changed = self.changed_inputs(&ledger, &records, &request);
            let pass_id = self.pass_id(&ids, ledger.passes);
            for (claim_id, text) in &request.texts {
                ledger.texts.insert(claim_id.clone(), text.clone());
            }
            for claim_id in &ids {
                ledger
                    .texts
                    .entry(claim_id.clone())
                    .or_insert_with(|| claim_id.clone());
            }
            let mut index = TokenIndex::default();
            for (claim_id, text) in &ledger.texts {
                index.insert(claim_id, text);
            }
            let changed_inputs: BTreeMap<String, ClaimInput> = changed
                .iter()
                .map(|(claim_id, input)| {
                    let mut input = input.clone();
                    input.text = ledger
                        .texts
                        .get(claim_id)
                        .cloned()
                        .unwrap_or_else(|| claim_id.clone());
                    (claim_id.clone(), input)
                })
                .collect();
            let output = run_pass(&pass_id, &changed_inputs, &index, &knobs);

            let now = ClaimStore::now_ms();
            for finding in &output.friction {
                let contradiction_id = contradiction_id(&finding.left, &finding.right);
                let evidence = [&finding.left, &finding.right]
                    .into_iter()
                    .filter_map(|claim| records.get(claim))
                    .flat_map(|record| record.evidence.iter().map(|e| e.evidence_id.clone()))
                    .collect();
                ledger
                    .contradictions
                    .entry(contradiction_id.clone())
                    .or_insert(ContradictionRecord {
                        contradiction_id,
                        left: finding.left.clone(),
                        right: finding.right.clone(),
                        detected_at_ms: now,
                        evidence,
                        status: ContradictionStatus::Open,
                    });
            }
            for (contradiction_id, to) in moves {
                if let Some(record) = ledger.contradictions.get_mut(&contradiction_id) {
                    record.status = to;
                }
            }
            for (claim_id, revision) in &output.revisions {
                ledger.revisions.insert(claim_id.clone(), revision.clone());
                ledger.retractions.remove(claim_id);
            }
            for proposal in &output.retractions {
                ledger
                    .retractions
                    .insert(proposal.claim_id.clone(), proposal.clone());
            }
            ledger.pending.clear();
            ledger.passes += 1;

            let event = EventInput::with_payload("record_pass", ledger.pass_payload());
            self.send(&self.instance_for_write(existing)?, event)?;
            if let Some(core) = core {
                assert_contradiction_edges(core, ledger.contradictions.values());
            }
            let result = BeliefPassResult {
                pass_id,
                revisions: output.revisions.values().cloned().collect(),
                friction: output.friction,
                retractions: output.retractions,
                contradictions: ledger.contradictions.values().cloned().collect(),
                claims_visited: output.claims_visited as u64,
                evidence_edges: output.evidence_edges as u64,
            };
            ResultPayload::of::<BeliefRevise>(result)
        }

        /// Every named claim must exist for the tenant.
        fn load_claims(
            &self,
            ids: &BTreeSet<String>,
        ) -> Result<BTreeMap<String, ClaimRecord>, String> {
            let mut records = BTreeMap::new();
            for id in ids {
                match self.claims.claim_record(id)? {
                    Some(record) => {
                        records.insert(id.clone(), record);
                    }
                    None => return Err(refuse(ServerErrorCode::BeliefClaimNotFound, id)),
                }
            }
            Ok(records)
        }

        /// A premise must be an existing claim, and its claim must be in the pass.
        fn check_premises(
            &self,
            request: &BeliefReviseRequest,
            ids: &BTreeSet<String>,
        ) -> Result<(), String> {
            for premise in &request.premises {
                if !ids.contains(&premise.claim_id) {
                    return Err(invalid(&format!(
                        "premise {} cites claim {} that is not in the pass",
                        premise.premise_id, premise.claim_id
                    )));
                }
                if self.claims.claim_record(&premise.premise_id)?.is_none() {
                    return Err(refuse(
                        ServerErrorCode::BeliefPremiseMissing,
                        &premise.premise_id,
                    ));
                }
            }
            Ok(())
        }

        /// Each requested move must be the next status of a tracked
        /// contradiction; moves are applied in request order.
        fn check_moves(
            &self,
            ledger: &Ledger,
            request: &BeliefReviseRequest,
        ) -> Result<Vec<(String, ContradictionStatus)>, String> {
            let mut current: BTreeMap<&str, ContradictionStatus> = ledger
                .contradictions
                .iter()
                .map(|(id, record)| (id.as_str(), record.status))
                .collect();
            let mut moves = Vec::new();
            for step in &request.advance {
                let Some(status) = current.get(step.contradiction_id.as_str()).copied() else {
                    return Err(invalid(&format!(
                        "contradiction {} is not tracked",
                        step.contradiction_id
                    )));
                };
                if status.next() != Some(step.to) {
                    return Err(refuse(
                        ServerErrorCode::ContradictionStatusOrder,
                        format!(
                            "{} cannot move from {} to {}",
                            step.contradiction_id,
                            status.as_str(),
                            step.to.as_str()
                        ),
                    ));
                }
                current.insert(step.contradiction_id.as_str(), step.to);
                moves.push((step.contradiction_id.clone(), step.to));
            }
            Ok(moves)
        }

        /// The claims whose evidence changed since their last revision, with
        /// the evidence the kernel reads. The prior of a claim is its last
        /// posterior; only evidence the last revision did not record counts.
        fn changed_inputs(
            &self,
            ledger: &Ledger,
            records: &BTreeMap<String, ClaimRecord>,
            request: &BeliefReviseRequest,
        ) -> BTreeMap<String, ClaimInput> {
            let mut changed = BTreeMap::new();
            for (claim_id, record) in records {
                let last = ledger.revisions.get(claim_id);
                let seen: BTreeSet<&str> = last
                    .into_iter()
                    .flat_map(|revision| revision.support.iter().chain(&revision.oppose))
                    .map(|reference| reference.evidence_id.as_str())
                    .collect();
                let irrelevant: BTreeSet<&str> = record
                    .feeds
                    .iter()
                    .filter(|grade| !grade.relevant)
                    .map(|grade| grade.evidence_id.as_str())
                    .collect();
                let mut input = ClaimInput {
                    text: String::new(),
                    prior_micro: last.map_or(0, |revision| revision.posterior_micro),
                    support: Vec::new(),
                    oppose: Vec::new(),
                };
                let mut fresh = false;
                for item in &record.evidence {
                    let id = item.evidence_id.as_str();
                    if seen.contains(id) || irrelevant.contains(id) {
                        continue;
                    }
                    fresh = true;
                    let entry = EvidenceInput {
                        evidence_id: item.evidence_id.clone(),
                        channel: item.channel,
                        weight_ppm: item.weight_ppm,
                        class: class_of(item.channel),
                    };
                    if item.supports {
                        input.support.push(entry);
                    } else {
                        input.oppose.push(entry);
                    }
                }
                for premise in request.premises.iter().filter(|p| &p.claim_id == claim_id) {
                    let evidence_id = format!("premise:{}", premise.premise_id);
                    if seen.contains(evidence_id.as_str()) {
                        continue;
                    }
                    fresh = true;
                    let entry = EvidenceInput {
                        evidence_id,
                        channel: premise.channel,
                        weight_ppm: premise.weight_ppm,
                        class: premise.class,
                    };
                    if premise.supports {
                        input.support.push(entry);
                    } else {
                        input.oppose.push(entry);
                    }
                }
                if fresh || last.is_none() {
                    changed.insert(claim_id.clone(), input);
                }
            }
            changed
        }
    }

    /// The id of the contradiction between two claims (`left` sorts first).
    fn contradiction_id(left: &str, right: &str) -> String {
        let mut hasher = Sha256::new();
        for part in ["medulla-contradiction/v1", left, right] {
            hasher.update((part.len() as u64).to_be_bytes());
            hasher.update(part.as_bytes());
        }
        format!("contra-{}", &hex::encode(hasher.finalize())[..24])
    }

    /// Assert a `CONTRADICTS` edge for each record whose two claim nodes exist
    /// in `core` and are not yet linked. The edge is a projection of the
    /// persisted record, so every pass re-asserts what a restart dropped.
    fn assert_contradiction_edges<'a>(
        core: &medulla_graph_core::graph::GraphCore,
        records: impl Iterator<Item = &'a ContradictionRecord>,
    ) {
        for record in records {
            if !core.has_node(&record.left)
                || !core.has_node(&record.right)
                || core.has_edge(&record.left, &record.right)
            {
                continue;
            }
            if let Ok(properties) =
                rmp_serde::to_vec_named(&serde_json::json!({ "relationship": "CONTRADICTS" }))
            {
                let _ = core.add_edge(record.left.clone(), record.right.clone(), properties);
            }
        }
    }
}
