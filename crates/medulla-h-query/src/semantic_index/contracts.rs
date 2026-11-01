//! Conversions from semantic owner runtime values to the declared ingestion
//! result-contract DTOs. The owner crate keeps its receipt/status types private
//! to the runtime crate DAG; the wire contract mirrors their serialized fields.

use medulla_core::compute::semantic_ann_codes::SemanticMutationReceipt as RuntimeReceipt;
use medulla_core::compute::semantic_index_service::{
    SemanticSqlSourcePageAdmission as RuntimePageAdmission,
    SemanticSqlSourceReconciliationAdmission as RuntimeReconciliationAdmission,
};
use medulla_index_types::semantic_index::SemanticStageTransition;
use medulla_protocol_types::{
    result_contract::ingestion::SemanticMutationReceipt,
    result_contract::ingestion::SemanticOutboxStatus,
    result_contract::ingestion::SemanticSqlSourcePageAdmission,
    result_contract::ingestion::SemanticSqlSourceReconciliationAdmission,
};
use medulla_transaction::OutboxStatus;

pub(super) fn receipt(value: RuntimeReceipt) -> SemanticMutationReceipt {
    SemanticMutationReceipt {
        batch_id: value.batch_id,
        mutation_digest: value.mutation_digest,
        source_version: value.source_version,
        target_version: value.target_version,
        replayed: value.replayed,
    }
}

pub(super) fn page(value: RuntimePageAdmission) -> SemanticSqlSourcePageAdmission {
    SemanticSqlSourcePageAdmission {
        receipts: value.receipts.into_iter().map(receipt).collect(),
        next_cursor: value.next_cursor,
        complete: value.complete,
    }
}

pub(super) fn reconciliation(
    value: RuntimeReconciliationAdmission,
) -> SemanticSqlSourceReconciliationAdmission {
    SemanticSqlSourceReconciliationAdmission {
        receipts: value.receipts.into_iter().map(receipt).collect(),
        source_revision: value.source_revision,
        page_count: value.page_count,
        complete: value.complete,
        wakeup_consumed: value.wakeup_consumed,
        rows_seen: value.rows_seen,
        source_bytes_seen: value.source_bytes_seen,
    }
}

pub(super) fn outbox_status(value: OutboxStatus) -> SemanticOutboxStatus {
    SemanticOutboxStatus {
        status: medulla_server_api::outbox_operator::consumer_status(&value),
        consecutive_claims: value.consecutive_claims,
        total_claims: value.total_claims,
    }
}

pub(super) fn replay(
    value: Option<(SemanticStageTransition, RuntimeReceipt)>,
) -> Option<(SemanticStageTransition, SemanticMutationReceipt)> {
    value.map(|(transition, receipt_value)| (transition, receipt(receipt_value)))
}
