//! Finance record reads served from the graph (MEDULLA-FINANCE-PRIMITIVES-R904).

use crate::{DurabilityDomain, Stability, TxnParticipation};

use super::{make_policy, spec, PolicyFlags, PolicyRow, PYTHON};

#[rustfmt::skip]
pub(crate) const ROWS: &[PolicyRow] = &[
    ("WalletKeyTrustGet", spec(make_policy(false, DurabilityDomain::None, "finance:records", PolicyFlags { idempotent: true, audited: false, emits_cdc: false }, TxnParticipation::Snapshot), PYTHON, Stability::Stable), ""),
    ("FinanceAccountProject", spec(make_policy(false, DurabilityDomain::None, "finance:records", PolicyFlags { idempotent: true, audited: false, emits_cdc: false }, TxnParticipation::Snapshot), PYTHON, Stability::Stable), ""),
    ("FinancePerformance", spec(make_policy(false, DurabilityDomain::None, "finance:records", PolicyFlags { idempotent: true, audited: false, emits_cdc: false }, TxnParticipation::Snapshot), PYTHON, Stability::Stable), ""),
    ("RegimeForecast", spec(make_policy(false, DurabilityDomain::None, "finance:records", PolicyFlags { idempotent: true, audited: false, emits_cdc: false }, TxnParticipation::Snapshot), PYTHON, Stability::Stable), ""),
    ("RegimeSizingInput", spec(make_policy(false, DurabilityDomain::None, "finance:records", PolicyFlags { idempotent: true, audited: false, emits_cdc: false }, TxnParticipation::Snapshot), PYTHON, Stability::Stable), ""),
    ("FinanceOrderReceiptGet", spec(make_policy(false, DurabilityDomain::None, "finance:records", PolicyFlags { idempotent: true, audited: false, emits_cdc: false }, TxnParticipation::Snapshot), PYTHON, Stability::Stable), "MEDULLA-FINANCE-PRIMITIVES-R015.1.4: the sealed OrderAdmissionReceipt of the promoted revision of one proposal, read from the verified tenant's own graph rows; an unknown, unpromoted or foreign proposal refuses ORDER_RECEIPT_NOT_FOUND; writes nothing"),
    ("FinanceOrderLimitCheck", spec(make_policy(false, DurabilityDomain::None, "finance:propose-order", PolicyFlags { idempotent: true, audited: false, emits_cdc: false }, TxnParticipation::Snapshot), PYTHON, Stability::Stable), ""),
];
