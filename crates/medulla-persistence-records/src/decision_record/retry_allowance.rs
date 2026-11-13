//! The durable retry allowance of a denied decision
//! (MEDULLA-DECISION-ENGINE-R113.3.1.1, the storage half of R113.3.2).
//!
//! A denied acquisition grants exactly one further decision. The allowance is
//! one row in the Agent Library owner table `decision_retry_allowance`, keyed
//! `(tenant, denied_decision_id)`, and its lifecycle is an explicit state
//! machine: the closed states [`DenialAllowanceState`], the closed events
//! [`DenialAllowanceEvent`], and ONE transition function,
//! [`denial_allowance_transition`], which is the only code that computes the
//! next state. The state is persisted as one field. `Consumed` is the
//! tombstone: it is retained forever and no event leaves it, so a restart or a
//! reopen never restores the allowance.
//!
//! Every write goes through the admitted pack write of the Agent Library
//! (replay identity, staged ledger batch, owner rows in one durable commit).

use medulla_agent_types::agent_library::AgentLibraryMutationContext;
use medulla_agent_types::agent_topology::capacity_commit::RetryAllowance;
use medulla_mutation_types::mutation_scope::DurabilityDomain;
use medulla_mutation_types::mutation_scope::MutationSurface;
use medulla_protocol_types::mutation_batch::MutationOperation;
use medulla_protocol_types::protocol::Method;
use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};
use serde::{Deserialize, Serialize};

use crate::agent_library::AgentLibraryStore;
use crate::connector_pack::admitted::{PackOwnerWrite, PackWriteEffects, PackWriteIdentity};

const RESULT_SCHEMA: &str = "decision-retry-allowance-result.v1";
const EXHAUSTED_CODE: &str = "RETRY_ALLOWANCE_EXHAUSTED";
const NOT_ISSUED_CODE: &str = "RETRY_ALLOWANCE_NOT_ISSUED";

/// The states of one denial allowance. `Issued` holds one decision;
/// `Consumed` holds none and is terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DenialAllowanceState {
    Issued,
    Consumed,
}

/// The one event of the allowance machine: the follow-up commit that names the
/// denied decision uses the allowance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DenialAllowanceEvent {
    Take,
}

impl MachineState for DenialAllowanceState {
    fn stored(self) -> &'static str {
        match self {
            Self::Issued => "issued",
            Self::Consumed => "consumed",
        }
    }
}

impl MachineEvent for DenialAllowanceEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Take => "Take",
        }
    }
}

/// `DenialAllowanceMachine`: `Issued` is the initial state a denial creates,
/// `Consumed` is terminal. The one edge is `Issued --Take--> Consumed`; every
/// other pair is refused with the typed `TRANSITION_REFUSED`.
pub static DENIAL_ALLOWANCE_MACHINE: MachineDef<
    DenialAllowanceState,
    DenialAllowanceEvent,
    &'static str,
> = MachineDef {
    name: "DenialAllowanceMachine",
    states: &[DenialAllowanceState::Issued, DenialAllowanceState::Consumed],
    initial: DenialAllowanceState::Issued,
    terminals: &[DenialAllowanceState::Consumed],
    events: &[DenialAllowanceEvent::Take],
    guards: &[],
    edges: &[Edge {
        id: "A01",
        from: DenialAllowanceState::Issued,
        event: DenialAllowanceEvent::Take,
        guard: None,
        to: DenialAllowanceState::Consumed,
    }],
};

impl DenialAllowanceState {
    /// The one function that produces a new allowance state from an event.
    pub fn apply(self, event: DenialAllowanceEvent) -> Result<Self, TransitionRefused> {
        transition(&DENIAL_ALLOWANCE_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }
}

/// What a caller asks of one allowance row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenialAllowanceRequest {
    /// A denial grants the one further decision (creates the initial state).
    Issue,
    /// A follow-up commit uses it (fires `Take`).
    Take,
}

/// Why an allowance event was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryAllowanceError {
    /// The allowance was already consumed. No event restores it.
    Exhausted,
    /// No allowance was issued under this tenant and denied decision.
    NotIssued,
    /// The store refused or failed; the text names the cause.
    Storage(String),
}

impl RetryAllowanceError {
    /// The stable code of a typed refusal.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::Exhausted => Some(EXHAUSTED_CODE),
            Self::NotIssued => Some(NOT_ISSUED_CODE),
            Self::Storage(_) => None,
        }
    }

    fn refusal(&self) -> String {
        match self {
            Self::Exhausted => format!("{EXHAUSTED_CODE}: the denial allowance was already used"),
            Self::NotIssued => {
                format!("{NOT_ISSUED_CODE}: no allowance was issued for this denied decision")
            }
            Self::Storage(text) => text.clone(),
        }
    }

    /// Recover the typed refusal from the text a write carried out.
    fn classify(text: String) -> Self {
        if text.starts_with(EXHAUSTED_CODE) {
            Self::Exhausted
        } else if text.starts_with(NOT_ISSUED_CODE) {
            Self::NotIssued
        } else {
            Self::Storage(text)
        }
    }
}

impl std::fmt::Display for RetryAllowanceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.refusal())
    }
}

/// Resolve one request against the stored state: the single place that turns a
/// request into the next state. `current` is `None` before the first `Issue`.
///
/// Legal: `(none, Issue) -> Issued`, `(Issued, Issue) -> Issued` (idempotent),
/// `(Issued, Take) -> Consumed` through [`DenialAllowanceState::apply`].
/// Illegal: `(none, Take)` is `NotIssued`; `(Consumed, Issue)` and
/// `(Consumed, Take)` are `Exhausted`, because no event leaves the tombstone.
pub fn denial_allowance_transition(
    current: Option<DenialAllowanceState>,
    request: DenialAllowanceRequest,
) -> Result<DenialAllowanceState, RetryAllowanceError> {
    match (current, request) {
        (None | Some(DenialAllowanceState::Issued), DenialAllowanceRequest::Issue) => {
            Ok(DenialAllowanceState::Issued)
        }
        (None, DenialAllowanceRequest::Take) => Err(RetryAllowanceError::NotIssued),
        (Some(DenialAllowanceState::Consumed), DenialAllowanceRequest::Issue) => {
            Err(RetryAllowanceError::Exhausted)
        }
        (Some(state), DenialAllowanceRequest::Take) => {
            let next = state
                .apply(DenialAllowanceEvent::Take)
                .map_err(|_| RetryAllowanceError::Exhausted)?;
            // The checked model transition is the consumer of the one
            // remaining decision; it agrees with the machine edge.
            let mut model = RetryAllowance {
                further_decisions: 1,
            };
            match model.consume() {
                Ok(()) => Ok(next),
                Err(_) => Err(RetryAllowanceError::Exhausted),
            }
        }
    }
}

/// The persisted row: the state is the only field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DenialAllowanceRow {
    state: DenialAllowanceState,
}

/// The typed result an allowance write records in the ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DenialAllowanceReceipt {
    pub tenant_id: String,
    pub denied_decision_id: String,
    pub state: DenialAllowanceState,
}

fn decode_row(bytes: &[u8]) -> Result<DenialAllowanceRow, String> {
    serde_json::from_slice(bytes)
        .map_err(|error| format!("stored retry allowance does not decode: {error}"))
}

fn write_event(
    write: &PackOwnerWrite<'_>,
    tenant_id: &str,
    denied_decision_id: &str,
    event: DenialAllowanceRequest,
) -> Result<DenialAllowanceReceipt, String> {
    let key = (tenant_id, denied_decision_id);
    let current = {
        let table = write.open_table(medulla_storage::DECISION_RETRY_ALLOWANCES)?;
        let row = table
            .get(key)
            .map_err(|error| error.to_string())?
            .map(|row| decode_row(row.value()))
            .transpose()?;
        row.map(|row| row.state)
    };
    let next = denial_allowance_transition(current, event).map_err(|error| error.refusal())?;
    if current != Some(next) {
        let bytes = serde_json::to_vec(&DenialAllowanceRow { state: next })
            .map_err(|error| error.to_string())?;
        write
            .open_table(medulla_storage::DECISION_RETRY_ALLOWANCES)?
            .insert(key, bytes.as_slice())
            .map_err(|error| error.to_string())?;
    }
    Ok(DenialAllowanceReceipt {
        tenant_id: tenant_id.to_string(),
        denied_decision_id: denied_decision_id.to_string(),
        state: next,
    })
}

impl AgentLibraryStore {
    fn commit_allowance_event(
        &self,
        context: &AgentLibraryMutationContext,
        denied_decision_id: &str,
        event: DenialAllowanceRequest,
    ) -> Result<DenialAllowanceReceipt, RetryAllowanceError> {
        medulla_agent_types::agent_library::validate_key(&context.tenant_id, denied_decision_id)
            .map_err(RetryAllowanceError::Storage)?;
        let (purpose, kind, event_type) = match event {
            DenialAllowanceRequest::Issue => (
                "decision-retry-allowance:issue",
                "decision-retry-allowance-issue",
                "decision_retry_allowance_issue",
            ),
            DenialAllowanceRequest::Take => (
                "decision-retry-allowance:take",
                "decision-retry-allowance-take",
                "decision_retry_allowance_take",
            ),
        };
        let identity = PackWriteIdentity {
            purpose,
            kind,
            slug: kind,
            subject: denied_decision_id,
            revision: 0,
            discriminator: None,
            result_schema: RESULT_SCHEMA,
            noun: "decision retry allowance",
        };
        let effects = PackWriteEffects {
            operations: vec![MutationOperation {
                ordinal: 0,
                surface: MutationSurface::Lifecycle,
                domain: DurabilityDomain::ControlPlane,
                method: Method::ApplyMutation {
                    event_type: event_type.into(),
                    query: denied_decision_id.to_string(),
                },
            }],
            outbox: Vec::new(),
        };
        let tenant = context.tenant_id.clone();
        self.commit_pack_write(context, identity, effects, |write, _staged| {
            write_event(write, &tenant, denied_decision_id, event)
        })
        .map_err(RetryAllowanceError::classify)
    }

    /// Issue the one further decision a denial grants. Writes `Issued` once and
    /// is idempotent for the same key; refuses `Exhausted` when the allowance
    /// was already consumed, because a tombstone is never reset.
    pub fn issue_denial_allowance(
        &self,
        context: &AgentLibraryMutationContext,
        denied_decision_id: &str,
    ) -> Result<DenialAllowanceReceipt, RetryAllowanceError> {
        self.commit_allowance_event(context, denied_decision_id, DenialAllowanceRequest::Issue)
    }

    /// Use the allowance: `Issued` becomes `Consumed` in one write. A second
    /// take refuses `Exhausted`; a key that was never issued, and any other
    /// tenant's key, refuse `NotIssued`.
    pub fn take_denial_allowance(
        &self,
        context: &AgentLibraryMutationContext,
        denied_decision_id: &str,
    ) -> Result<(), RetryAllowanceError> {
        self.commit_allowance_event(context, denied_decision_id, DenialAllowanceRequest::Take)
            .map(drop)
    }

    /// The current state of one allowance, if any was issued.
    pub fn denial_allowance_state(
        &self,
        tenant_id: &str,
        denied_decision_id: &str,
    ) -> Result<Option<DenialAllowanceState>, RetryAllowanceError> {
        let read = self.read().map_err(RetryAllowanceError::Storage)?;
        let table = read
            .open_owner_table(medulla_storage::DECISION_RETRY_ALLOWANCES)
            .map_err(RetryAllowanceError::Storage)?;
        let row = table
            .get((tenant_id, denied_decision_id))
            .map_err(|error| RetryAllowanceError::Storage(error.to_string()))?
            .map(|row| decode_row(row.value()))
            .transpose()
            .map_err(RetryAllowanceError::Storage)?;
        Ok(row.map(|row| row.state))
    }
}
