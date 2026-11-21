//! The owner digest as stored queries (MEDULLA-BUSINESS-OPS-R003.1, R003.2).
//!
//! The three definitions are written once here. The consumer puts them with
//! `Method::StoredQueryPut` at tenant onboarding and calls each by name with
//! `Method::StoredQueryCall`; the engine adds no served method and reads no clock, so the
//! as-of is always a parameter.

use std::collections::BTreeMap;

use medulla_identity_types::identity::ShareScope;

use crate::stored_query::{RefreshPolicy, StoredParamType, StoredQuery, StoredQueryLanguage};

/// The stored query that lists the approval requests awaiting a role the person holds.
pub const DIGEST_PENDING_APPROVALS: &str = "digest.pending_approvals";
/// The stored query that lists open obligations due before a date.
pub const DIGEST_OVERDUE: &str = "digest.overdue";
/// The stored query that lists the nodes recorded in a transaction-time window.
pub const DIGEST_CHANGED_SINCE: &str = "digest.changed_since";

/// `digest.pending_approvals`: the `:ApprovalRequest` nodes awaiting a role the person holds
/// through a `:RoleAssignment`, largest amount first.
const PENDING_APPROVALS_TEXT: &str = "MATCH (:Person) WHERE id = $person \
|> TRAVERSE <-[:assigneePerson]- \
|> TRAVERSE -[:assignedRole]-> \
|> TRAVERSE <-[:requestRequiredRole]- \
|> WHERE requestStatus = 'pending' \
|> ORDER BY requestAmountMinor DESC \
|> RETURN requestActionKind, requestAmountMinor, valid_from";

/// `digest.overdue`: the `:ObligationInstance` nodes (tax returns included) still open or
/// overdue and due before `$as_of_date`.
const OVERDUE_TEXT: &str = "MATCH (:ObligationInstance) \
WHERE obligationStatus IN ('open', 'overdue') AND instanceDueDate < $as_of_date \
|> ORDER BY instanceDueDate \
|> RETURN instanceDueDate, obligationStatus";

/// `digest.changed_since`: the business nodes recorded by transaction time in `[from, to)`.
const CHANGED_SINCE_TEXT: &str = "MATCH () WHERE label IN ('ApprovalRequest', 'GateDecision', \
'ObligationInstance', 'TaxReturn', 'Order', 'OrderLine', 'Payment', 'Refund', 'Payout', 'Fee', \
'PurchaseOrder', 'PurchaseOrderLine', 'GoodsReceipt', 'StockLevel', 'DeployRecord', 'Incident') \
AND tx_from >= $from AND tx_from < $to \
|> ORDER BY label, tx_from \
|> RETURN label, tx_from, valid_from";

fn digest_query(
    name: &str,
    text: &str,
    params: &[(&str, StoredParamType)],
) -> StoredQuery {
    StoredQuery {
        name: name.to_string(),
        version: 1,
        language: StoredQueryLanguage::Uql,
        text: text.to_string(),
        params: params
            .iter()
            .map(|(param, ty)| ((*param).to_string(), *ty))
            .collect::<BTreeMap<_, _>>(),
        materialize: false,
        refresh: RefreshPolicy::OnChange,
        // The engine stamps `author` on put and ignores whatever a request carries.
        author: String::new(),
        share: ShareScope::Private,
    }
}

/// The three digest definitions: pending approvals, overdue obligations and changed in a
/// window, in that order.
pub fn digest_queries() -> [StoredQuery; 3] {
    [
        digest_query(
            DIGEST_PENDING_APPROVALS,
            PENDING_APPROVALS_TEXT,
            &[("person", StoredParamType::String)],
        ),
        digest_query(
            DIGEST_OVERDUE,
            OVERDUE_TEXT,
            &[("as_of_date", StoredParamType::String)],
        ),
        digest_query(
            DIGEST_CHANGED_SINCE,
            CHANGED_SINCE_TEXT,
            &[("from", StoredParamType::Int), ("to", StoredParamType::Int)],
        ),
    ]
}
