//! The `FinanceReceipt` delivery machine and the wire types of its served door
//! (MEDULLA-FINANCE-PRIMITIVES-R011.5).
//!
//! A receipt is the idempotency and delivery record of one finance alert delivery,
//! schedule run or DCA proposal. Its state lives in `receiptState` and moves only
//! through [`FINANCE_RECEIPT_MACHINE`], the one edge table that
//! `state-machines-v1.ttl` repeats. A retry is the `Retry` edge: it raises
//! `attempt` by one in the same step, so no caller rewrites the counter.

use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};
use serde::{Deserialize, Serialize};

/// The most attempts one receipt may make; a `Retry` from attempt `N` is guarded by
/// `N < FINANCE_RECEIPT_MAX_ATTEMPTS`.
pub const FINANCE_RECEIPT_MAX_ATTEMPTS: u64 = 5;
/// The page size a `FinanceReceiptList` answers when it names no limit.
pub const FINANCE_RECEIPT_LIST_DEFAULT_LIMIT: u32 = 100;
/// The largest page a `FinanceReceiptList` may ask for.
pub const FINANCE_RECEIPT_LIST_MAX_LIMIT: u32 = 1000;
/// The guard of the `Retry` edges: the receipt has attempts left.
pub const FINANCE_RECEIPT_GUARD_UNDER_ATTEMPT_LIMIT: &str = "UnderAttemptLimit";

/// The delivery state of one receipt, stored as `receiptState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum FinanceReceiptState {
    /// The delivery is owed and nobody holds it. A new receipt starts here.
    Due,
    /// One worker holds the delivery.
    Claimed,
    /// The channel accepted the delivery; the acknowledgement is still owed.
    Delivered,
    /// The delivery is acknowledged. Terminal.
    Completed,
    /// The delivery window passed with nobody claiming it.
    Missed,
    /// The delivery failed; a retry may follow.
    Failed,
}

impl FinanceReceiptState {
    /// The state a stored `receiptState` text names; `None` for a text of no state.
    pub fn parse(text: &str) -> Option<Self> {
        FINANCE_RECEIPT_MACHINE
            .states
            .iter()
            .copied()
            .find(|state| state.stored() == text)
    }

    /// The one transition function of the machine. It answers the next state and the
    /// attempt count after `event`: `Retry` is the only event that raises `attempt`.
    pub fn step(
        self,
        attempt: u64,
        event: FinanceReceiptEvent,
    ) -> Result<(Self, u64), TransitionRefused> {
        let edge = transition(&FINANCE_RECEIPT_MACHINE, self, event, |guard| {
            guard != FINANCE_RECEIPT_GUARD_UNDER_ATTEMPT_LIMIT
                || attempt < FINANCE_RECEIPT_MAX_ATTEMPTS
        })?;
        let attempt = if event == FinanceReceiptEvent::Retry {
            attempt + 1
        } else {
            attempt
        };
        Ok((edge.to, attempt))
    }
}

/// An event of the receipt machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum FinanceReceiptEvent {
    /// A worker takes the delivery.
    Claim,
    /// The channel accepted the delivery.
    Deliver,
    /// The delivery is acknowledged.
    Complete,
    /// The delivery window passed.
    Miss,
    /// The delivery failed.
    Fail,
    /// Another attempt is made.
    Retry,
}

impl MachineState for FinanceReceiptState {
    fn stored(self) -> &'static str {
        match self {
            Self::Due => "due",
            Self::Claimed => "claimed",
            Self::Delivered => "delivered",
            Self::Completed => "completed",
            Self::Missed => "missed",
            Self::Failed => "failed",
        }
    }
}

impl MachineEvent for FinanceReceiptEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Claim => "Claim",
            Self::Deliver => "Deliver",
            Self::Complete => "Complete",
            Self::Miss => "Miss",
            Self::Fail => "Fail",
            Self::Retry => "Retry",
        }
    }
}

/// `FinanceReceiptMachine`: the closed edge table. Every other pair of state and event
/// is refused `TRANSITION_REFUSED`.
pub static FINANCE_RECEIPT_MACHINE: MachineDef<
    FinanceReceiptState,
    FinanceReceiptEvent,
    &'static str,
> = MachineDef {
    name: "FinanceReceiptMachine",
    states: &[
        FinanceReceiptState::Due,
        FinanceReceiptState::Claimed,
        FinanceReceiptState::Delivered,
        FinanceReceiptState::Completed,
        FinanceReceiptState::Missed,
        FinanceReceiptState::Failed,
    ],
    initial: FinanceReceiptState::Due,
    terminals: &[FinanceReceiptState::Completed],
    events: &[
        FinanceReceiptEvent::Claim,
        FinanceReceiptEvent::Deliver,
        FinanceReceiptEvent::Complete,
        FinanceReceiptEvent::Miss,
        FinanceReceiptEvent::Fail,
        FinanceReceiptEvent::Retry,
    ],
    guards: &[FINANCE_RECEIPT_GUARD_UNDER_ATTEMPT_LIMIT],
    edges: &[
        Edge {
            id: "F01",
            from: FinanceReceiptState::Due,
            event: FinanceReceiptEvent::Claim,
            guard: None,
            to: FinanceReceiptState::Claimed,
        },
        Edge {
            id: "F02",
            from: FinanceReceiptState::Claimed,
            event: FinanceReceiptEvent::Deliver,
            guard: None,
            to: FinanceReceiptState::Delivered,
        },
        Edge {
            id: "F03",
            from: FinanceReceiptState::Delivered,
            event: FinanceReceiptEvent::Complete,
            guard: None,
            to: FinanceReceiptState::Completed,
        },
        Edge {
            id: "F04",
            from: FinanceReceiptState::Due,
            event: FinanceReceiptEvent::Miss,
            guard: None,
            to: FinanceReceiptState::Missed,
        },
        Edge {
            id: "F05",
            from: FinanceReceiptState::Claimed,
            event: FinanceReceiptEvent::Fail,
            guard: None,
            to: FinanceReceiptState::Failed,
        },
        Edge {
            id: "F06",
            from: FinanceReceiptState::Delivered,
            event: FinanceReceiptEvent::Fail,
            guard: None,
            to: FinanceReceiptState::Failed,
        },
        Edge {
            id: "F07",
            from: FinanceReceiptState::Failed,
            event: FinanceReceiptEvent::Retry,
            guard: Some(FINANCE_RECEIPT_GUARD_UNDER_ATTEMPT_LIMIT),
            to: FinanceReceiptState::Due,
        },
        Edge {
            id: "F08",
            from: FinanceReceiptState::Missed,
            event: FinanceReceiptEvent::Retry,
            guard: Some(FINANCE_RECEIPT_GUARD_UNDER_ATTEMPT_LIMIT),
            to: FinanceReceiptState::Due,
        },
    ],
};

/// One receipt as the served door answers it. Every field is the stored property of the
/// same name in snake case; the stored forms are `receiptKind`, `receiptState`,
/// `tenantKey`, `idempotencyKey`, `receiptEventId`, `receiptMode`, `receiptChannel`,
/// `receiptAttempt` and `recordedAt`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FinanceReceiptView {
    /// The graph node id of the receipt.
    pub receipt_id: String,
    /// `alert_delivery`, `schedule_run` or `dca_proposal`.
    pub receipt_kind: String,
    /// The delivery state.
    pub receipt_state: FinanceReceiptState,
    /// The opaque tenant key the receipt belongs to.
    pub tenant_key: String,
    /// The idempotency key of the work the receipt records.
    pub idempotency_key: String,
    /// The id of the event the delivery reports.
    pub event_id: String,
    /// The delivery mode, an opaque non-empty text owned by the caller.
    pub mode: String,
    /// The delivery channel, an opaque non-empty text owned by the caller.
    pub channel: String,
    /// The attempt this receipt is on, starting at 1.
    pub attempt: u64,
    /// When the receipt was recorded (an `xsd:dateTime` text).
    pub recorded_at: String,
}

impl FinanceReceiptView {
    /// The view of the node `receipt_id` whose stored properties are `props`; `None` when
    /// the node is not a complete receipt.
    pub fn from_properties(
        receipt_id: &str,
        props: &serde_json::Map<String, serde_json::Value>,
    ) -> Option<Self> {
        let text = |key: &str| props.get(key).and_then(|v| v.as_str()).map(str::to_owned);
        Some(Self {
            receipt_id: receipt_id.to_owned(),
            receipt_kind: text("receiptKind")?,
            receipt_state: FinanceReceiptState::parse(&text("receiptState")?)?,
            tenant_key: text("tenantKey")?,
            idempotency_key: text("idempotencyKey")?,
            event_id: text("receiptEventId")?,
            mode: text("receiptMode")?,
            channel: text("receiptChannel")?,
            attempt: props.get("receiptAttempt").and_then(|v| v.as_u64())?,
            recorded_at: text("recordedAt")?,
        })
    }
}

/// What `FinanceReceiptList` answers: the receipts of the caller's tenant under the asked
/// tenant key, oldest `recorded_at` first (ties by `receipt_id`), at most `limit` of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FinanceReceiptPage {
    pub receipts: Vec<FinanceReceiptView>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-FINANCE-PRIMITIVES-R011.5
    #[test]
    fn a_retry_raises_the_attempt_and_the_other_events_keep_it() {
        use FinanceReceiptEvent::*;
        use FinanceReceiptState::*;
        assert_eq!(Due.step(1, Claim).unwrap(), (Claimed, 1));
        assert_eq!(Claimed.step(1, Deliver).unwrap(), (Delivered, 1));
        assert_eq!(Delivered.step(1, Complete).unwrap(), (Completed, 1));
        assert_eq!(Claimed.step(1, Fail).unwrap(), (Failed, 1));
        assert_eq!(Failed.step(1, Retry).unwrap(), (Due, 2));
        assert_eq!(Due.step(2, Miss).unwrap(), (Missed, 2));
        assert_eq!(Missed.step(2, Retry).unwrap(), (Due, 3));
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R011.5
    #[test]
    fn the_retry_guard_stops_at_the_attempt_limit() {
        let refused = FinanceReceiptState::Failed
            .step(FINANCE_RECEIPT_MAX_ATTEMPTS, FinanceReceiptEvent::Retry)
            .unwrap_err();
        assert_eq!(
            refused.reason,
            medulla_types::transition_refused::TransitionRefusalReason::GuardFailed
        );
        assert_eq!(refused.machine, "FinanceReceiptMachine");
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R011.5
    #[test]
    fn every_pair_without_an_edge_is_refused_and_completed_is_terminal() {
        let machine = &FINANCE_RECEIPT_MACHINE;
        for &state in machine.states {
            for &event in machine.events {
                let has_edge = machine
                    .edges
                    .iter()
                    .any(|edge| edge.from == state && edge.event == event);
                assert_eq!(state.step(1, event).is_ok(), has_edge, "{state:?} {event:?}");
            }
        }
        for &event in machine.events {
            let refused = FinanceReceiptState::Completed.step(1, event).unwrap_err();
            assert_eq!(
                refused.reason,
                medulla_types::transition_refused::TransitionRefusalReason::TerminalState
            );
        }
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R011.5
    #[test]
    fn a_stored_state_text_round_trips() {
        for &state in FINANCE_RECEIPT_MACHINE.states {
            assert_eq!(FinanceReceiptState::parse(state.stored()), Some(state));
        }
        assert_eq!(FinanceReceiptState::parse("claim"), None);
    }
}
