//! `Method::FeatureLedger`: the feature ledger (MEDULLA-DECISION-ENGINE-R707.3).
//!
//! This handler is the single writer of ledger state. A tenant's ledger is one
//! instance of the ledger chart in the `medulla-statechart` store
//! (`medulla_store::feature_ledger_statechart`); its instance id derives from
//! the verified tenant alone, so another tenant derives another instance and
//! finds none of these entries. Each entry's status is the explicit
//! `FeatureLedgerMachine`: `LedgerStatus::apply`, through `advance_ledger`, is
//! the only function that changes it, and an illegal event is the typed
//! `TRANSITION_REFUSED`. A stale `expected_revision` is
//! `LEDGER_REVISION_CONFLICT`, and a `Close` whose `closed_by` the repository
//! index does not know is `LEDGER_CLOSE_NO_CHANGE` (checked before the
//! compare-and-set).
//! Every refusal returns before the store write, and one operation is one
//! store write, so an operation over several entries changes all of them or none.
//!
//! A build without the `statechart` feature answers a typed refusal naming it.

use medulla_decision_statistical_types::decision::feature_match::FeatureLedgerOp;
use medulla_h_decide_base::decide::SharedState;
use medulla_protocol_types::protocol::Response;
use medulla_server_api::authority_context::VerifiedRequestContext;

/// Serve one feature ledger operation.
pub async fn handle_feature_ledger(
    state: &SharedState,
    req_id: u64,
    verified: &VerifiedRequestContext,
    op: FeatureLedgerOp,
) -> Response {
    #[cfg(feature = "statechart")]
    {
        let persist_dir = { state.read().await.persist_dir.clone() };
        // A close names the change that closes the feature; the repository
        // index answers whether it knows that change, before any state moves.
        let known = match &op {
            FeatureLedgerOp::Close { closed_by, .. } => {
                match change_known(state, verified, closed_by).await {
                    Ok(known) => Some(known),
                    Err(error) => return Response::err(req_id, error),
                }
            }
            _ => None,
        };
        let served = served::FeatureLedger::open(
            persist_dir.as_deref(),
            verified.tenant(),
            &verified.principal_persistence_id(),
        )
        .and_then(|ledger| ledger.serve(op, known));
        match served {
            Ok(payload) => Response::ok(req_id, payload),
            Err(error) => Response::err(req_id, error),
        }
    }
    #[cfg(not(feature = "statechart"))]
    {
        let _ = (state, verified, op);
        Response::err(req_id, "FeatureLedger requires the `statechart` feature")
    }
}

/// Whether the repository index holds the change `closed_by` names, in any
/// graph the caller may read. A graph or row the caller may not see answers
/// like an absent one (MEDULLA-DECISION-ENGINE-R707.4).
#[cfg(feature = "statechart")]
async fn change_known(
    state: &SharedState,
    verified: &VerifiedRequestContext,
    closed_by: &str,
) -> Result<bool, String> {
    let seam = medulla_server_ops::api::ingest::ingest_seam(state)
        .await
        .ok_or_else(|| "FeatureLedger Close requires the installed ingest seam".to_string())?;
    let graphs: Vec<String> = state
        .read()
        .await
        .registry
        .list()
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    for graph in graphs {
        // A graph the caller may not read is not part of what the caller knows.
        let Ok(view) = medulla_h_decide_base::decide::stat_vectors::graph_vectors(
            state,
            verified.agent_id(),
            &graph,
        )
        .await
        else {
            continue;
        };
        if seam.change_known(&|id| view.visible_properties(id), closed_by) {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(feature = "statechart")]
pub(super) mod served {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use medulla_protocol_types::protocol::ResultPayload;
    use medulla_protocol_types::result_contract::coordination::{
        FeatureLedgerClose, FeatureLedgerGet, FeatureLedgerOpen, FeatureLedgerPlan,
    };
    use medulla_statechart::{
        instance_id_for_request, Context, EventInput, MachineInstance, StatechartError,
        StatechartStore,
    };
    use medulla_store::feature_ledger_statechart::{feature_ledger_statechart_def, RECORD_EVENT};
    use {
        medulla_decision_statistical_types::decision::feature_match::FeatureLedgerOp,
        medulla_decision_statistical_types::decision::feature_match::FeatureLedgerView,
        medulla_decision_statistical_types::decision::feature_match::LedgerEntry,
        medulla_decision_statistical_types::decision::feature_match::LedgerEvent,
        medulla_decision_statistical_types::decision::feature_match::LedgerStatus,
        medulla_decision_statistical_types::decision::feature_match::PlanProposal,
        medulla_decision_statistical_types::decision::feature_match::FEATURE_LEDGER_MACHINE,
    };
    use {
        medulla_decision_statistical_types::decision::feature_match::LEDGER_CLOSE_NO_CHANGE,
        medulla_decision_statistical_types::decision::feature_match::LEDGER_REVISION_CONFLICT,
    };
    use {
        medulla_decision_stats_records::decision::feature_match::advance_ledger,
        medulla_decision_stats_records::decision::feature_match::LedgerChangeError,
    };

    use super::super::claims::served::{field, invalid, request_key, ClaimStore};

    type Entries = BTreeMap<String, LedgerEntry>;

    /// The feature ledger of one tenant.
    pub(in crate::decide) struct FeatureLedger {
        store: Arc<StatechartStore>,
        def_id: String,
        tenant: String,
        actor: String,
    }

    impl FeatureLedger {
        /// Open the shared statechart store and register the ledger chart.
        pub(in crate::decide) fn open(
            persist_dir: Option<&str>,
            tenant: &str,
            actor: &str,
        ) -> Result<Self, String> {
            let store = medulla_h_jobs::statechart::statechart_store(persist_dir)?;
            let def_id = store
                .define(&feature_ledger_statechart_def())
                .map_err(|error| error.to_string())?;
            Ok(Self {
                store,
                def_id,
                tenant: tenant.to_string(),
                actor: actor.to_string(),
            })
        }

        fn key(&self) -> String {
            request_key("feature_ledger", &self.tenant, "ledger")
        }

        /// The ledger instance if one was ever written. Reading never creates it.
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

        fn load(&self) -> Result<(Option<MachineInstance>, Entries), String> {
            let existing = self.existing()?;
            let entries = match &existing {
                Some(instance) => field(&instance.context, "entries")?,
                None => Entries::new(),
            };
            Ok((existing, entries))
        }

        fn create(&self) -> Result<MachineInstance, String> {
            let (instance, _replayed) = self
                .store
                .instantiate_batch(
                    &self.def_id,
                    Context::from_value(serde_json::json!({
                        "tenant_id": self.tenant,
                        "entries": {},
                    }))?,
                    &self.tenant,
                    &self.actor,
                    &self.key(),
                    ClaimStore::now_ms(),
                )
                .map_err(|error| error.to_string())?;
            Ok(instance)
        }

        /// Store `entries` as the tenant's whole entry map: one write, guarded
        /// by the version of the instance it was read from.
        fn record(
            &self,
            existing: Option<MachineInstance>,
            entries: &Entries,
        ) -> Result<(), String> {
            let instance = match existing {
                Some(instance) => instance,
                None => self.create()?,
            };
            let event =
                EventInput::with_payload(RECORD_EVENT, serde_json::json!({ "entries": entries }));
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
                        format!("{}: {error}", LEDGER_REVISION_CONFLICT.as_str())
                    }
                    other => other.to_string(),
                })?;
            if !outcome.outcome.fired {
                return Err("feature ledger event did not fire".to_string());
            }
            Ok(())
        }

        /// Run one operation to its typed result.
        pub(in crate::decide) fn serve(
            &self,
            op: FeatureLedgerOp,
            change_known: Option<bool>,
        ) -> Result<ResultPayload, String> {
            match op {
                FeatureLedgerOp::Open {
                    feature_id,
                    match_ids,
                } => {
                    ResultPayload::of::<FeatureLedgerOpen>(self.open_entry(feature_id, match_ids)?)
                }
                FeatureLedgerOp::Plan {
                    plan,
                    expected_revision,
                } => ResultPayload::of::<FeatureLedgerPlan>(self.plan(plan, expected_revision)?),
                FeatureLedgerOp::Close {
                    feature_id,
                    closed_by,
                    expected_revision,
                } => ResultPayload::of::<FeatureLedgerClose>(self.close(
                    &feature_id,
                    closed_by,
                    expected_revision,
                    change_known == Some(true),
                )?),
                FeatureLedgerOp::Get { feature_ids } => {
                    ResultPayload::of::<FeatureLedgerGet>(self.get(&feature_ids)?)
                }
            }
        }

        fn open_entry(
            &self,
            feature_id: String,
            match_ids: Vec<String>,
        ) -> Result<FeatureLedgerView, String> {
            if feature_id.is_empty() {
                return Err(invalid("feature_id is empty"));
            }
            let (existing, mut entries) = self.load()?;
            if entries.contains_key(&feature_id) {
                return Err(invalid(&format!(
                    "feature {feature_id} is already in the ledger"
                )));
            }
            let entry = LedgerEntry {
                feature_id: feature_id.clone(),
                status: FEATURE_LEDGER_MACHINE.initial,
                closed_by: None,
                revision: 0,
                match_ids,
                plan_id: None,
            };
            entries.insert(feature_id, entry.clone());
            self.record(existing, &entries)?;
            Ok(FeatureLedgerView {
                entries: vec![entry],
            })
        }

        fn plan(
            &self,
            plan: PlanProposal,
            expected_revision: u64,
        ) -> Result<FeatureLedgerView, String> {
            if plan.plan_id.is_empty() || plan.feature_ids.is_empty() {
                return Err(invalid("a plan names an id and at least one feature"));
            }
            let (existing, mut entries) = self.load()?;
            let mut changed = Vec::new();
            for feature_id in &plan.feature_ids {
                let entry = entries.get(feature_id).ok_or_else(|| {
                    invalid(&format!("feature {feature_id} is not in the ledger"))
                })?;
                let mut next = advance_ledger(entry, expected_revision, LedgerEvent::Plan)
                    .map_err(change_refusal)?;
                next.plan_id = Some(plan.plan_id.clone());
                changed.push(next);
            }
            for entry in &changed {
                entries.insert(entry.feature_id.clone(), entry.clone());
            }
            self.record(existing, &entries)?;
            changed.sort_by(|a, b| a.feature_id.cmp(&b.feature_id));
            changed.dedup_by(|a, b| a.feature_id == b.feature_id);
            Ok(FeatureLedgerView { entries: changed })
        }

        fn close(
            &self,
            feature_id: &str,
            closed_by: String,
            expected_revision: u64,
            change_known: bool,
        ) -> Result<FeatureLedgerView, String> {
            if closed_by.is_empty() {
                return Err(invalid("closed_by names no change"));
            }
            let (existing, mut entries) = self.load()?;
            let entry = entries
                .get(feature_id)
                .ok_or_else(|| invalid(&format!("feature {feature_id} is not in the ledger")))?;
            if !change_known {
                return Err(format!(
                    "{}: '{closed_by}' names no change the repository index knows",
                    LEDGER_CLOSE_NO_CHANGE.as_str()
                ));
            }
            let mut next = advance_ledger(entry, expected_revision, LedgerEvent::Close)
                .map_err(change_refusal)?;
            debug_assert_eq!(next.status, LedgerStatus::Closed);
            next.closed_by = Some(closed_by);
            entries.insert(feature_id.to_string(), next.clone());
            self.record(existing, &entries)?;
            Ok(FeatureLedgerView {
                entries: vec![next],
            })
        }

        fn get(&self, feature_ids: &[String]) -> Result<FeatureLedgerView, String> {
            let (_, entries) = self.load()?;
            let found: Vec<LedgerEntry> = if feature_ids.is_empty() {
                entries.into_values().collect()
            } else {
                feature_ids
                    .iter()
                    .filter_map(|id| entries.get(id).cloned())
                    .collect()
            };
            Ok(FeatureLedgerView { entries: found })
        }
    }

    fn change_refusal(error: LedgerChangeError) -> String {
        match error {
            LedgerChangeError::RevisionConflict { expected, found } => format!(
                "{}: the entry is at revision {found}, not {expected}",
                LEDGER_REVISION_CONFLICT.as_str()
            ),
            LedgerChangeError::Transition(refused) => refused.to_string(),
        }
    }
}
