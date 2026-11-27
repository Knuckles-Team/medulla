//! Open one delivery per dispatched slice (MEDULLA-SDLC-R019.3) and shadow an unprompted
//! slice in `development` mode (MEDULLA-SDLC-R019.4).
//!
//! The engine starts no run: the caller passes the run trace id it minted. [`open_delivery`]
//! is a pure decision; the served arm reads `existing` and the project mode, takes the clock
//! from the kernel, and writes the returned delivery in one batch.

use medulla_source_types::delivery::{
    AssignmentState, Delivery, DeliveryAssignment, DeliveryMandate, MandateStrength,
    DELIVERY_SCHEMA_VERSION,
};
use medulla_source_types::lifecycle::{sha256_hex, LifecycleRefusal, ProjectMode, ProposalOrigin};

/// The inputs of one delivery open.
pub struct OpenDelivery<'a> {
    /// The tenant the delivery belongs to.
    pub tenant: &'a str,
    /// The repository the delivery changes.
    pub repo: &'a str,
    /// The root of the slice the delivery carries out.
    pub slice_root: &'a str,
    /// The spec rows the slice covers; empty is refused.
    pub scope_rows: &'a [String],
    /// The decision that dispatched the slice.
    pub dispatch_decision_id: &'a str,
    /// The policy version the dispatch was decided under.
    pub policy_version: &'a str,
    /// Who requested the delivery.
    pub actor: &'a str,
    /// Who or what proposed the slice.
    pub origin: ProposalOrigin,
    /// The project mode in force; it decides whether an unprompted slice starts a run.
    pub mode: ProjectMode,
    /// The trace id of the run the assignment records.
    pub run_trace_id: &'a str,
    /// The time of the open, in milliseconds since the Unix epoch.
    pub now_ms: i64,
}

/// The first 32 hex digits of the SHA-256 of `text`.
fn hash(text: &str) -> String {
    sha256_hex(text.as_bytes())[..32].to_owned()
}

/// The id of the delivery of one dispatch of one slice: the idempotency key is
/// `{tenant}\n{dispatch_decision_id}\n{slice_root}`.
pub fn delivery_id(tenant: &str, dispatch_decision_id: &str, slice_root: &str) -> String {
    format!(
        "delivery:{}",
        hash(&format!("{tenant}\n{dispatch_decision_id}\n{slice_root}"))
    )
}

/// Open the delivery of one slice. A second call with the same key returns `existing`
/// unchanged: the mandate is immutable, and a wider scope needs a new slice root and so a new
/// key. A slice with no rows refuses `DELIVERY_SCOPE_EMPTY`; a `reference` codebase refuses
/// `MODE_REFERENCE_NO_PROPOSALS`. Both are checked before any id is derived.
pub fn open_delivery(
    input: &OpenDelivery<'_>,
    existing: Option<Delivery>,
) -> Result<Delivery, LifecycleRefusal> {
    if input.scope_rows.is_empty() {
        return Err(LifecycleRefusal::DeliveryScopeEmpty(
            input.slice_root.to_owned(),
        ));
    }
    if input.mode == ProjectMode::Reference {
        return Err(LifecycleRefusal::ModeReferenceNoProposals(
            input.repo.to_owned(),
        ));
    }
    if let Some(delivery) = existing {
        return Ok(delivery);
    }
    let id = delivery_id(input.tenant, input.dispatch_decision_id, input.slice_root);
    let (strength, basis) = match input.origin {
        ProposalOrigin::Requested => (
            MandateStrength::Explicit,
            format!("requested by {}", input.actor),
        ),
        ProposalOrigin::Unprompted => (
            MandateStrength::Inferred,
            format!(
                "inferred from dispatch decision {}",
                input.dispatch_decision_id
            ),
        ),
    };
    let mut scope_rows = input.scope_rows.to_vec();
    scope_rows.sort();
    scope_rows.dedup();
    let assignment = DeliveryAssignment {
        schema_version: DELIVERY_SCHEMA_VERSION,
        id: format!(
            "assignment:{}",
            hash(&format!("{id}\n{}", input.run_trace_id))
        ),
        run_trace_id: input.run_trace_id.to_owned(),
        state: AssignmentState::Pending,
        report: None,
        started_at: None,
        ended_at: None,
        ended_reason: None,
        recovery: None,
    };
    Ok(Delivery {
        schema_version: DELIVERY_SCHEMA_VERSION,
        id,
        tenant: input.tenant.to_owned(),
        repo: input.repo.to_owned(),
        slice_root: input.slice_root.to_owned(),
        scope_rows: scope_rows.clone(),
        mandate: DeliveryMandate {
            schema_version: DELIVERY_SCHEMA_VERSION,
            strength,
            basis,
            policy_version: input.policy_version.to_owned(),
            actor: input.actor.to_owned(),
            scope_rows,
            project_mode: input.mode,
        },
        seq: 0,
        proposals: Vec::new(),
        follows: Vec::new(),
        assignments: vec![assignment],
        inputs: Vec::new(),
        attention: None,
        ended: None,
        created_at: input.now_ms,
    })
}

use medulla_source_types::delivery::WouldDispatch;

/// The decision of one slice: a run, or a recorded `would_dispatch`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DispatchOutcome {
    /// The slice started a run; the delivery is open.
    Dispatched(Box<Delivery>),
    /// Development mode shadowed the slice: nothing started and the would-be dispatch is recorded.
    WouldDispatch(WouldDispatch),
}

/// Dispatch a slice or shadow it. In mode `development` an unprompted slice starts no run:
/// the engine records a `would_dispatch` decision with the slice, rows and evidence digests
/// (the knob `sdlc.delivery.shadow_unprompted_in_development` is Fixed at true). In `evolving`
/// the same slice dispatches, and a person's request dispatches in both modes. A `reference`
/// codebase refuses, as [`open_delivery`] does.
pub fn shadow_or_dispatch(
    input: &OpenDelivery<'_>,
    existing: Option<Delivery>,
    evidence_digests: &[String],
) -> Result<DispatchOutcome, LifecycleRefusal> {
    if input.scope_rows.is_empty() {
        return Err(LifecycleRefusal::DeliveryScopeEmpty(
            input.slice_root.to_owned(),
        ));
    }
    if input.mode == ProjectMode::Development && input.origin == ProposalOrigin::Unprompted {
        let mut rows = input.scope_rows.to_vec();
        rows.sort();
        rows.dedup();
        let mut digests = evidence_digests.to_vec();
        digests.sort();
        digests.dedup();
        return Ok(DispatchOutcome::WouldDispatch(WouldDispatch {
            slice_root: input.slice_root.to_owned(),
            rows,
            evidence_digests: digests,
            dispatch_decision_id: input.dispatch_decision_id.to_owned(),
            mode: input.mode,
        }));
    }
    open_delivery(input, existing).map(|delivery| DispatchOutcome::Dispatched(Box::new(delivery)))
}
