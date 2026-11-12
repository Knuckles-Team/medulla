//! `Method::Loops`: the improvement-loop lease, gap market, ledger and ratchet
//! (MEDULLA-DECISION-ENGINE-R653.3 .. .5).
//!
//! This handler is the single writer of loop state. A loop is two instances in
//! the `medulla-statechart` store: its state is an instance of
//! `medulla_store::loop_statechart::loop_statechart_def()`, and its lease,
//! gaps, offers, beacons, ledger, patterns, ratchet comparisons and
//! transition history are one `LoopView` held by an instance of the one-state
//! `loop_record` chart. Both instance ids derive from the verified tenant and
//! the caller's loop id, so a read under another tenant derives another id and
//! answers `LOOP_NOT_FOUND`. The kernels live in `medulla_statechart::loops`
//! and take time as an argument. Every refusal returns before a store write.
//!
//! A build without the `statechart` feature answers a typed refusal naming it.

use super::SharedState;
use medulla_protocol_types::protocol::Response;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_types::loops::LoopOp;

/// Serve one loop operation.
pub async fn handle_loops(
    state: &SharedState,
    req_id: u64,
    verified: &VerifiedRequestContext,
    op: LoopOp,
) -> Response {
    #[cfg(feature = "statechart")]
    {
        let persist_dir = { state.read().await.persist_dir.clone() };
        let served = served::LoopStore::open(
            persist_dir.as_deref(),
            verified.tenant(),
            &verified.principal_persistence_id(),
        )
        .and_then(|loops| loops.serve(op));
        match served {
            Ok(payload) => Response::ok(req_id, payload),
            Err(error) => Response::err(req_id, error),
        }
    }
    #[cfg(not(feature = "statechart"))]
    {
        let _ = (state, verified, op);
        Response::err(req_id, "Loops requires the `statechart` feature")
    }
}

#[cfg(feature = "statechart")]
pub(super) mod served {
    use std::sync::Arc;

    use medulla_protocol_types::protocol::ResultPayload;
    use medulla_protocol_types::result_contract::coordination::{
        LoopsBeacon, LoopsClaim, LoopsFileGap, LoopsGet, LoopsMine, LoopsOffer, LoopsRatchet,
        LoopsRecordSample, LoopsRelease, LoopsSettle,
    };
    use medulla_statechart::loops::{
        check_epoch, claim_loop, mine_patterns, offer_rate, ratchet_verdict, saturation_ppm,
        settle_gap, LoopError, LoopKnobs,
    };
    use medulla_statechart::{
        instance_id_for_request, Context, EventInput, MachineInstance, StatechartError,
        StatechartStore,
    };
    use medulla_store::loop_statechart::{
        loop_record_statechart_def, loop_statechart_def, FINAL_STATES, LOOP_RECORD_KEY,
    };
    use medulla_types::contract::ServerErrorCode;
    use medulla_types::loops::{
        Gap, GapState, ImprovementLedger, LoopInstance, LoopOp, LoopTransition, LoopView,
        OfferEvidence, StageBeacon, PPM_MAX,
    };

    use crate::decide::claims::served::{invalid, refuse, request_key};

    /// The states a lease holder may act in.
    const ACTIVE: &[&str] = &["running", "pending", "validating"];

    fn knob_u32(value: u64) -> u32 {
        u32::try_from(value).unwrap_or(u32::MAX)
    }

    /// The loop knobs loaded at startup.
    pub(in crate::decide) fn loop_knobs() -> LoopKnobs {
        let knobs = medulla_server_api::knob_config::knobs();
        LoopKnobs {
            lease_ttl_s: knobs.loops_lease_ttl_s,
            offer_rate_cap_ppm: knob_u32(knobs.loops_offer_rate_cap_ppm),
            offer_capacity: knob_u32(knobs.loops_offer_capacity),
            miner_max_len: knob_u32(knobs.loops_miner_max_len),
            miner_min_support: knob_u32(knobs.loops_miner_min_support),
            miner_max_patterns: knob_u32(knobs.loops_miner_max_patterns),
            ratchet_min_samples: knob_u32(knobs.loops_ratchet_min_samples),
            ratchet_min_gain_micro: knobs.loops_ratchet_min_gain_micro,
        }
    }

    /// The typed refusal text of one kernel error.
    fn kernel_refusal(error: LoopError) -> String {
        let code = match &error {
            LoopError::LeaseHeld { .. } => ServerErrorCode::LoopLeaseHeld,
            LoopError::LeaseStale { .. } => ServerErrorCode::LoopLeaseStale,
            LoopError::GapAlreadySettled { .. } => ServerErrorCode::GapAlreadySettled,
            LoopError::GapNotOffered { .. } => ServerErrorCode::GapNotOffered,
            LoopError::RatchetNoBaseline { .. } => ServerErrorCode::RatchetNoBaseline,
        };
        let text = error.to_string();
        refuse(
            code,
            text.split_once(": ").map_or(text.as_str(), |(_, d)| d),
        )
    }

    fn payload_of<M>(body: M::Body) -> Result<ResultPayload, String>
    where
        M: medulla_protocol_types::result_contract::MethodResult,
    {
        ResultPayload::of::<M>(body)
    }

    /// The loop store of one verified principal.
    pub(in crate::decide) struct LoopStore {
        store: Arc<StatechartStore>,
        loop_def: String,
        record_def: String,
        tenant: String,
        actor: String,
        knobs: LoopKnobs,
    }

    impl LoopStore {
        /// Open the shared statechart store and register both charts.
        pub(in crate::decide) fn open(
            persist_dir: Option<&str>,
            tenant: &str,
            actor: &str,
        ) -> Result<Self, String> {
            let store = medulla_h_jobs::statechart::statechart_store(persist_dir)?;
            let loop_def = store
                .define(&loop_statechart_def())
                .map_err(|error| error.to_string())?;
            let record_def = store
                .define(&loop_record_statechart_def())
                .map_err(|error| error.to_string())?;
            Ok(Self {
                store,
                loop_def,
                record_def,
                tenant: tenant.to_string(),
                actor: actor.to_string(),
                knobs: loop_knobs(),
            })
        }

        /// The same store with other knobs (tests).
        #[cfg(test)]
        pub(in crate::decide) fn with_knobs(mut self, knobs: LoopKnobs) -> Self {
            self.knobs = knobs;
            self
        }

        /// The underlying statechart store (tests seed corrupt state with it).
        #[cfg(test)]
        pub(in crate::decide) fn statecharts(&self) -> &Arc<StatechartStore> {
            &self.store
        }

        /// The instance key of `kind` for `loop_id` in this tenant.
        #[cfg(test)]
        pub(in crate::decide) fn key(&self, kind: &str, loop_id: &str) -> String {
            request_key(kind, &self.tenant, loop_id)
        }

        pub(in crate::decide) fn now_ms() -> u64 {
            medulla_kernel::kernel::dispatch_support::authoritative_now_ms()
        }

        /// Run one operation at the authoritative time.
        pub(in crate::decide) fn serve(&self, op: LoopOp) -> Result<ResultPayload, String> {
            self.serve_at(op, Self::now_ms())
        }

        /// Run one operation at `now_ms`.
        pub(in crate::decide) fn serve_at(
            &self,
            op: LoopOp,
            now_ms: u64,
        ) -> Result<ResultPayload, String> {
            match op {
                LoopOp::Claim {
                    loop_id,
                    objective_digest,
                    priority,
                } => payload_of::<LoopsClaim>(self.claim(
                    &loop_id,
                    &objective_digest,
                    priority,
                    now_ms,
                )?),
                LoopOp::Release { loop_id, epoch } => {
                    payload_of::<LoopsRelease>(self.release(&loop_id, epoch, now_ms)?)
                }
                LoopOp::Beacon {
                    loop_id,
                    epoch,
                    stage,
                } => payload_of::<LoopsBeacon>(self.mutate(&loop_id, epoch, now_ms, |view| {
                    if stage.is_empty() {
                        return Err(invalid("stage is empty"));
                    }
                    view.beacons.push(StageBeacon {
                        loop_id: loop_id.clone(),
                        stage,
                        at_ms: now_ms,
                    });
                    Ok(())
                })?),
                LoopOp::FileGap {
                    loop_id,
                    epoch,
                    gap_id,
                    severity_ppm,
                    evidence,
                } => payload_of::<LoopsFileGap>(self.mutate(&loop_id, epoch, now_ms, |view| {
                    if gap_id.is_empty() {
                        return Err(invalid("gap_id is empty"));
                    }
                    if severity_ppm > PPM_MAX {
                        return Err(invalid("severity_ppm exceeds 1000000"));
                    }
                    if view.gaps.iter().all(|gap| gap.gap_id != gap_id) {
                        view.gaps.push(Gap {
                            gap_id,
                            loop_id: loop_id.clone(),
                            severity_ppm,
                            evidence,
                            state: GapState::Open,
                        });
                    }
                    Ok(())
                })?),
                LoopOp::Offer {
                    loop_id,
                    epoch,
                    gap_id,
                } => {
                    let knobs = self.knobs;
                    payload_of::<LoopsOffer>(self.mutate(&loop_id, epoch, now_ms, |view| {
                        let open_offers = view
                            .gaps
                            .iter()
                            .filter(|gap| gap.state == GapState::Offered)
                            .count() as u32;
                        let gap = find_gap(view, &gap_id)?;
                        match gap.state {
                            GapState::Offered => return Ok(()),
                            GapState::Settled => {
                                return Err(kernel_refusal(LoopError::GapAlreadySettled { gap_id }))
                            }
                            GapState::Open => {}
                        }
                        let severity_ppm = gap.severity_ppm;
                        gap.state = GapState::Offered;
                        view.offers.push(OfferEvidence {
                            gap_id,
                            rate_ppm: offer_rate(severity_ppm, open_offers, &knobs),
                            severity_ppm,
                            saturation_ppm: saturation_ppm(open_offers, knobs.offer_capacity),
                            open_offers,
                            capacity: knobs.offer_capacity,
                        });
                        Ok(())
                    })?)
                }
                LoopOp::Settle {
                    loop_id,
                    epoch,
                    gap_id,
                } => payload_of::<LoopsSettle>(self.mutate(&loop_id, epoch, now_ms, |view| {
                    settle_gap(find_gap(view, &gap_id)?).map_err(kernel_refusal)
                })?),
                LoopOp::RecordSample {
                    loop_id,
                    epoch,
                    sample,
                } => payload_of::<LoopsRecordSample>(self.mutate(
                    &loop_id,
                    epoch,
                    now_ms,
                    |view| {
                        if sample.metric.is_empty() {
                            return Err(invalid("metric is empty"));
                        }
                        view.ledger.samples.push(sample);
                        Ok(())
                    },
                )?),
                LoopOp::Ratchet {
                    loop_id,
                    epoch,
                    metric,
                    change_at_ms,
                } => {
                    let knobs = self.knobs;
                    payload_of::<LoopsRatchet>(self.mutate(&loop_id, epoch, now_ms, |view| {
                        let evidence =
                            ratchet_verdict(&view.ledger.samples, &metric, change_at_ms, &knobs)
                                .map_err(kernel_refusal)?;
                        view.ratchets.retain(|held| {
                            !(held.metric == metric && held.change_at_ms == change_at_ms)
                        });
                        view.ratchets.push(evidence);
                        Ok(())
                    })?)
                }
                LoopOp::Mine {
                    loop_id,
                    epoch,
                    failure_sequences,
                } => {
                    let knobs = self.knobs;
                    payload_of::<LoopsMine>(self.mutate(&loop_id, epoch, now_ms, |view| {
                        view.patterns = mine_patterns(&failure_sequences, &knobs);
                        Ok(())
                    })?)
                }
                LoopOp::Get { loop_id } => payload_of::<LoopsGet>(self.get(&loop_id)?),
            }
        }

        fn find(&self, kind: &str, id: &str) -> Result<Option<MachineInstance>, String> {
            let instance_id = instance_id_for_request(&request_key(kind, &self.tenant, id));
            match self.store.get_instance(&instance_id) {
                Ok(instance) if instance.tenant == self.tenant => Ok(Some(instance)),
                Ok(_) | Err(StatechartError::NotFound(_)) => Ok(None),
                Err(error) => Err(error.to_string()),
            }
        }

        /// Both instances of a loop, or `LOOP_NOT_FOUND`.
        fn require(&self, loop_id: &str) -> Result<(MachineInstance, MachineInstance), String> {
            let state = self.find("loop", loop_id)?;
            let record = self.find("loop_record", loop_id)?;
            match (state, record) {
                (Some(state), Some(record)) => Ok((state, record)),
                _ => Err(refuse(ServerErrorCode::LoopNotFound, loop_id)),
            }
        }

        fn create(
            &self,
            kind: &str,
            id: &str,
            def_id: &str,
            context: serde_json::Value,
            now_ms: u64,
        ) -> Result<MachineInstance, String> {
            let (instance, _replayed) = self
                .store
                .instantiate_batch(
                    def_id,
                    Context::from_value(context)?,
                    &self.tenant,
                    &self.actor,
                    &request_key(kind, &self.tenant, id),
                    now_ms,
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
            now_ms: u64,
        ) -> Result<MachineInstance, String> {
            let outcome = self
                .store
                .send_event(
                    &instance.instance_id,
                    &event,
                    Some(instance.version),
                    now_ms as i64,
                )
                .map_err(|error| match error {
                    StatechartError::VersionConflict { .. } => {
                        refuse(ServerErrorCode::RevisionConflict, error)
                    }
                    other => other.to_string(),
                })?;
            if !outcome.outcome.fired {
                return Err(refuse(
                    ServerErrorCode::LoopNotClaimable,
                    format!("event {} did not fire", event.name),
                ));
            }
            Ok(outcome.instance)
        }

        /// The state of a loop instance, checked against the chart.
        fn state_of(instance: &MachineInstance) -> Result<String, String> {
            let active = instance
                .configuration
                .active
                .iter()
                .next()
                .ok_or_else(|| refuse(ServerErrorCode::LoopStateUnknown, "no active state"))?;
            if !loop_statechart_def()
                .states
                .iter()
                .any(|state| state.id == *active)
            {
                return Err(refuse(ServerErrorCode::LoopStateUnknown, active));
            }
            Ok(active.to_string())
        }

        fn view_of(state: &MachineInstance, record: &MachineInstance) -> Result<LoopView, String> {
            let mut view: LoopView = match record.context.get(LOOP_RECORD_KEY) {
                Some(value) => serde_json::from_value(value.clone())
                    .map_err(|error| format!("loop record is unreadable: {error}"))?,
                None => return Err("loop record is missing".to_string()),
            };
            view.instance.state = Self::state_of(state)?;
            Ok(view)
        }

        fn write(
            &self,
            record: &MachineInstance,
            view: &LoopView,
            now_ms: u64,
        ) -> Result<(), String> {
            let event =
                EventInput::with_payload("write", serde_json::json!({ LOOP_RECORD_KEY: view }));
            self.send(record, event, now_ms).map(|_| ())
        }

        fn transition(&self, view: &mut LoopView, event: &str, from: &str, to: &str, now_ms: u64) {
            view.transitions.push(LoopTransition {
                epoch: view.instance.lease.as_ref().map_or(0, |lease| lease.epoch),
                event: event.to_string(),
                from: from.to_string(),
                to: to.to_string(),
                at_ms: now_ms,
                actor: self.actor.clone(),
            });
        }

        fn claim(
            &self,
            loop_id: &str,
            objective_digest: &str,
            priority: u32,
            now_ms: u64,
        ) -> Result<LoopView, String> {
            if loop_id.is_empty() {
                return Err(invalid("loop_id is empty"));
            }
            let (state_instance, record_instance) = if self.find("loop", loop_id)?.is_none() {
                let view = LoopView {
                    instance: LoopInstance {
                        tenant_id: self.tenant.clone(),
                        loop_id: loop_id.to_string(),
                        objective_digest: objective_digest.to_string(),
                        state: loop_statechart_def().initial,
                        priority,
                        created_at_ms: now_ms,
                        lease: None,
                    },
                    gaps: Vec::new(),
                    offers: Vec::new(),
                    beacons: Vec::new(),
                    ledger: ImprovementLedger {
                        loop_id: loop_id.to_string(),
                        samples: Vec::new(),
                    },
                    patterns: Vec::new(),
                    ratchets: Vec::new(),
                    transitions: Vec::new(),
                };
                let state = self.create(
                    "loop",
                    loop_id,
                    &self.loop_def,
                    serde_json::json!({"tenant_id": self.tenant, "loop_id": loop_id}),
                    now_ms,
                )?;
                let record = self.create(
                    "loop_record",
                    loop_id,
                    &self.record_def,
                    serde_json::json!({ LOOP_RECORD_KEY: view }),
                    now_ms,
                )?;
                (state, record)
            } else {
                self.require(loop_id)?
            };
            let mut view = Self::view_of(&state_instance, &record_instance)?;
            let lease = claim_loop(
                view.instance.lease.as_ref(),
                &self.actor,
                now_ms,
                self.knobs.lease_ttl_s,
            )
            .map_err(kernel_refusal)?;
            let from = view.instance.state.clone();
            let mut instance = state_instance;
            if ACTIVE.contains(&from.as_str()) {
                // The lease expired under an in-flight loop: the sweep event
                // marks it re-claimable before the new holder takes it.
                instance = self.send(
                    &instance,
                    EventInput::with_payload("lease_lost", serde_json::json!({})),
                    now_ms,
                )?;
                let to = Self::state_of(&instance)?;
                self.transition(&mut view, "lease_lost", &from, &to, now_ms);
            } else if !matches!(from.as_str(), "submitted" | "orphaned") {
                let why = if FINAL_STATES.contains(&from.as_str()) {
                    "the loop has finished"
                } else {
                    "the loop is not waiting for a holder"
                };
                return Err(refuse(
                    ServerErrorCode::LoopNotClaimable,
                    format!("{loop_id} is {from}: {why}"),
                ));
            }
            let before = Self::state_of(&instance)?;
            let instance = self.send(
                &instance,
                EventInput::with_payload("claim", serde_json::json!({})),
                now_ms,
            )?;
            let after = Self::state_of(&instance)?;
            view.instance.lease = Some(lease);
            self.transition(&mut view, "claim", &before, &after, now_ms);
            view.instance.state = after;
            self.write(&record_instance, &view, now_ms)?;
            Ok(view)
        }

        fn release(&self, loop_id: &str, epoch: u64, now_ms: u64) -> Result<LoopView, String> {
            let (state_instance, record_instance) = self.require(loop_id)?;
            let mut view = Self::view_of(&state_instance, &record_instance)?;
            fence(&view, epoch)?;
            let from = view.instance.state.clone();
            let instance = self.send(
                &state_instance,
                EventInput::with_payload("lease_lost", serde_json::json!({})),
                now_ms,
            )?;
            let to = Self::state_of(&instance)?;
            // The lease keeps its epoch so the next claim raises it; it ends now.
            if let Some(lease) = view.instance.lease.as_mut() {
                lease.expires_at_ms = lease.expires_at_ms.min(now_ms);
            }
            self.transition(&mut view, "release", &from, &to, now_ms);
            view.instance.state = to;
            self.write(&record_instance, &view, now_ms)?;
            Ok(view)
        }

        /// Run `change` on the loop's record under the fenced epoch.
        fn mutate(
            &self,
            loop_id: &str,
            epoch: u64,
            now_ms: u64,
            change: impl FnOnce(&mut LoopView) -> Result<(), String>,
        ) -> Result<LoopView, String> {
            let (state_instance, record_instance) = self.require(loop_id)?;
            let mut view = Self::view_of(&state_instance, &record_instance)?;
            fence(&view, epoch)?;
            change(&mut view)?;
            self.write(&record_instance, &view, now_ms)?;
            Ok(view)
        }

        fn get(&self, loop_id: &str) -> Result<LoopView, String> {
            let (state_instance, record_instance) = self.require(loop_id)?;
            Self::view_of(&state_instance, &record_instance)
        }
    }

    /// The epoch fence: the stored epoch must match and the loop must be in
    /// a state a holder acts in (a released loop has no holder).
    fn fence(view: &LoopView, epoch: u64) -> Result<(), String> {
        check_epoch(view.instance.lease.as_ref(), epoch).map_err(kernel_refusal)?;
        if !ACTIVE.contains(&view.instance.state.as_str()) {
            return Err(refuse(
                ServerErrorCode::LoopLeaseStale,
                format!("the loop is {}: no holder", view.instance.state),
            ));
        }
        Ok(())
    }

    fn find_gap<'a>(view: &'a mut LoopView, gap_id: &str) -> Result<&'a mut Gap, String> {
        view.gaps
            .iter_mut()
            .find(|gap| gap.gap_id == gap_id)
            .ok_or_else(|| refuse(ServerErrorCode::GapNotFound, gap_id))
    }
}
