//! Decide-layer job rows, evaluation receipts and fit drafts (EH-062, EH-040).
//!
//! They live in the tenant-scoped Agent Library control owner, beside the
//! components a receipt qualifies, so the `DecisionHead` publish check reads a
//! receipt from the same authority that commits the head. One table with
//! `job:`, `receipt:`, `receipt-threshold:`, `receipt-time:`, `draft:`, `grounded-pin:`,
//! `evaluation-run:`, and `replay-cursor:` key families, every row
//! written once: a job runs to its terminal state inside its submit, so there
//! is nothing to update. The `receipt-time:` family indexes only non-synthetic
//! full-label receipts written after this index was introduced; older receipts
//! remain available through digest lookup and are never assigned a fake time.

use medulla_storage::{AgentLibraryOwner, DECISION_ARTIFACTS};
use medulla_transaction::AdmittedOwnerWrite;
use redb::ReadableTable;

use super::agent_library::AgentLibraryStore;

/// Key of a job row.
pub fn job_key(job_id: &str) -> String {
    format!("job:{job_id}")
}

/// Key of an evaluation receipt.
pub fn receipt_key(receipt_digest: &str) -> String {
    format!("receipt:{receipt_digest}")
}

/// Key of the immutable policy assessment committed with a full-label receipt.
pub fn receipt_threshold_key(receipt_digest: &str) -> String {
    format!("receipt-threshold:{receipt_digest}")
}

/// Stable lexical order by authoritative job submission time and digest.
pub fn receipt_time_key(submitted_at_ms: u64, receipt_digest: &str) -> String {
    format!("receipt-time:{submitted_at_ms:020}:{receipt_digest}")
}

/// Key of a committed statistical decision record.
pub fn record_key(record_id: &str) -> String {
    format!("record:{record_id}")
}

/// Key of one outcome evaluation of a committed record.
pub fn evaluation_key(record_id: &str, evaluation_id: &str) -> String {
    format!("evaluation:{record_id}:{evaluation_id}")
}

/// Key of a sealed replay evaluation run (EH-528).
pub fn evaluation_run_key(run_digest: &str) -> String {
    format!("evaluation-run:{run_digest}")
}

/// Key naming the sealed run that first reached an event-replay cursor
/// (MEDULLA-DECISION-ENGINE-R088.2.2); its value is that run's digest.
pub fn replay_cursor_key(cursor: &str) -> String {
    format!("replay-cursor:{cursor}")
}

/// Key of the digest a grounded-evaluation input set pinned for one revision:
/// `id` at `revision`. Written once with the first accepted evaluation that
/// binds it (MEDULLA-DECISION-ENGINE-R100.101.3).
pub fn grounded_pin_key(set_id: &str, revision: u32) -> String {
    format!("{}{revision:010}", grounded_pin_prefix(set_id))
}

/// Prefix of every stored revision of the grounded input set `set_id`.
pub fn grounded_pin_prefix(set_id: &str) -> String {
    format!("grounded-pin:{set_id}:")
}
/// Key of a fit draft.
pub fn draft_key(draft_sha256: &str) -> String {
    format!("draft:{draft_sha256}")
}

/// Key of the penalty receipt stored beside a penalised fit draft.
pub fn penalty_key(draft_sha256: &str) -> String {
    format!("draft-penalty:{draft_sha256}")
}

fn put_absent(
    owner: &AdmittedOwnerWrite<'_, AgentLibraryOwner>,
    tenant_id: &str,
    rows: &[(String, Vec<u8>)],
) -> Result<(), String> {
    let mut table = owner.open_table(DECISION_ARTIFACTS)?;
    for (key, bytes) in rows {
        let existing = table
            .get((tenant_id, key.as_str()))
            .map_err(|error| error.to_string())?
            .map(|row| row.value().to_vec());
        match existing {
            Some(stored) if &stored == bytes => {}
            Some(_) => {
                return Err(format!(
                    "IDEMPOTENCY_CONFLICT: decision artifact {key} already holds other bytes"
                ))
            }
            None => {
                table
                    .insert((tenant_id, key.as_str()), bytes.as_slice())
                    .map_err(|error| error.to_string())?;
            }
        }
    }
    Ok(())
}

impl AgentLibraryStore {
    /// Write `rows` for `tenant_id` in one owner transaction. A row that is
    /// already present with the same bytes is a replay; with other bytes the
    /// whole write is refused.
    pub fn put_decision_artifacts(
        &self,
        tenant_id: &str,
        rows: &[(String, Vec<u8>)],
    ) -> Result<(), String> {
        self.maintain_control_rows(tenant_id, "decision_artifacts", |owner| {
            put_absent(owner, tenant_id, rows)
        })
    }

    /// Overwrite one existing decision artifact: the log entry's retention
    /// state (`Compacted`/`Retired`) is the only row that changes after it is
    /// written, and its record digest is never recomputed.
    pub fn replace_decision_artifact(
        &self,
        tenant_id: &str,
        key: &str,
        bytes: Vec<u8>,
    ) -> Result<(), String> {
        self.maintain_control_rows(tenant_id, "decision_artifacts", |owner| {
            let mut table = owner.open_table(DECISION_ARTIFACTS)?;
            let present = table
                .get((tenant_id, key))
                .map_err(|error| error.to_string())?
                .is_some();
            if !present {
                return Err(format!("decision artifact {key} is absent"));
            }
            table
                .insert((tenant_id, key), bytes.as_slice())
                .map_err(|error| error.to_string())?;
            Ok(())
        })
    }

    /// Remove every decision artifact of `tenant_id` whose key is one of
    /// `keys` or starts with one of `prefixes`, in one owner transaction, and
    /// return how many rows went. Retention expiry (MEDULLA-DECISION-ENGINE-
    /// R066.4) is the only caller: an expired record leaves no joined row
    /// behind.
    pub fn remove_decision_artifacts(
        &self,
        tenant_id: &str,
        keys: &[String],
        prefixes: &[String],
    ) -> Result<usize, String> {
        let mut removed = 0;
        self.maintain_control_rows(tenant_id, "decision_artifacts", |owner| {
            let mut table = owner.open_table(DECISION_ARTIFACTS)?;
            removed = 0;
            let mut doomed: Vec<String> = Vec::new();
            for prefix in prefixes {
                for row in table
                    .range((tenant_id, prefix.as_str())..)
                    .map_err(|error| error.to_string())?
                {
                    let (key, _) = row.map_err(|error| error.to_string())?;
                    let (row_tenant, row_key) = key.value();
                    if row_tenant != tenant_id || !row_key.starts_with(prefix.as_str()) {
                        break;
                    }
                    doomed.push(row_key.to_string());
                }
            }
            doomed.extend(keys.iter().cloned());
            for key in doomed {
                if table
                    .remove((tenant_id, key.as_str()))
                    .map_err(|error| error.to_string())?
                    .is_some()
                {
                    removed += 1;
                }
            }
            Ok(())
        })?;
        Ok(removed)
    }

    /// Compare-and-set one decision artifact: write `bytes` only when the row
    /// still holds `expected` (`None` = absent). A pointer that moved since it
    /// was read is refused, never overwritten (the query adapter state,
    /// EH-396).
    pub fn swap_decision_artifact(
        &self,
        tenant_id: &str,
        key: &str,
        expected: Option<Vec<u8>>,
        bytes: Vec<u8>,
    ) -> Result<(), String> {
        self.maintain_control_rows(tenant_id, "decision_artifacts", |owner| {
            let mut table = owner.open_table(DECISION_ARTIFACTS)?;
            let current = table
                .get((tenant_id, key))
                .map_err(|error| error.to_string())?
                .map(|row| row.value().to_vec());
            if current != expected {
                return Err(format!(
                    "IDEMPOTENCY_CONFLICT: decision artifact {key} changed concurrently; read it again"
                ));
            }
            table
                .insert((tenant_id, key), bytes.as_slice())
                .map_err(|error| error.to_string())?;
            Ok(())
        })
    }

    /// Compare-and-set several decision artifacts in one owner transaction:
    /// every `(key, expected, bytes)` is written only when every row still holds
    /// its `expected` bytes (`None` = absent), else nothing is written. A review
    /// that moves a revision row and stores its receipt is one such write
    /// (MEDULLA-DECISION-ENGINE-R805.15).
    pub fn swap_decision_artifacts(
        &self,
        tenant_id: &str,
        swaps: &[(String, Option<Vec<u8>>, Vec<u8>)],
    ) -> Result<(), String> {
        self.maintain_control_rows(tenant_id, "decision_artifacts", |owner| {
            let mut table = owner.open_table(DECISION_ARTIFACTS)?;
            for (key, expected, _) in swaps {
                let current = table
                    .get((tenant_id, key.as_str()))
                    .map_err(|error| error.to_string())?
                    .map(|row| row.value().to_vec());
                if &current != expected {
                    return Err(format!(
                        "IDEMPOTENCY_CONFLICT: decision artifact {key} changed concurrently; read it again"
                    ));
                }
            }
            for (key, _, bytes) in swaps {
                table
                    .insert((tenant_id, key.as_str()), bytes.as_slice())
                    .map_err(|error| error.to_string())?;
            }
            Ok(())
        })
    }

    /// Every decision artifact of `tenant_id` whose key starts with `prefix`,
    /// in key order. Refuses rather than truncates past `limit`.
    pub fn decision_artifacts_with_prefix(
        &self,
        tenant_id: &str,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<(String, Vec<u8>)>, String> {
        self.scan_decision_artifacts(tenant_id, prefix, None, limit, true)
    }

    /// Read at most `limit` tenant rows after an exclusive key cursor. The
    /// caller supplies a fixed artifact-family prefix and owns page semantics.
    pub fn decision_artifacts_page(
        &self,
        tenant_id: &str,
        prefix: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<(String, Vec<u8>)>, String> {
        self.scan_decision_artifacts(tenant_id, prefix, after, limit, false)
    }

    fn scan_decision_artifacts(
        &self,
        tenant_id: &str,
        prefix: &str,
        after: Option<&str>,
        limit: usize,
        reject_truncation: bool,
    ) -> Result<Vec<(String, Vec<u8>)>, String> {
        let read = self.read()?;
        let table = read.open_owner_table(DECISION_ARTIFACTS)?;
        let mut out = Vec::new();
        for row in table
            .range((tenant_id, after.unwrap_or(prefix))..)
            .map_err(|error| error.to_string())?
        {
            let (key, value) = row.map_err(|error| error.to_string())?;
            let (row_tenant, row_key) = key.value();
            if row_tenant != tenant_id || !row_key.starts_with(prefix) {
                break;
            }
            if after == Some(row_key) {
                continue;
            }
            if reject_truncation && out.len() == limit {
                return Err(format!(
                    "DECISION_LOG_TOO_LARGE: more than {limit} rows under {prefix}"
                ));
            }
            out.push((row_key.to_string(), value.value().to_vec()));
            if !reject_truncation && out.len() == limit {
                break;
            }
        }
        Ok(out)
    }

    /// Read one decision artifact of `tenant_id`.
    pub fn decision_artifact(&self, tenant_id: &str, key: &str) -> Result<Option<Vec<u8>>, String> {
        let read = self.read()?;
        let table = read.open_owner_table(DECISION_ARTIFACTS)?;
        Ok(table
            .get((tenant_id, key))
            .map_err(|error| error.to_string())?
            .map(|row| row.value().to_vec()))
    }
}

/// Key family of retrieval outcomes (`retrieval:<record id>`), written once per
/// committed retrieval-plan record by the decision log's outcome join.
pub const RETRIEVAL_OUTCOME_PREFIX: &str = "retrieval:";

/// Rows read per page while scanning retrieval outcomes.
const OUTCOME_PAGE_ROWS: usize = 1_000;

impl AgentLibraryStore {
    /// The distinct span ids that retrieval outcomes of `tenant_id` recorded as
    /// cited at or after `since_ms` (MEDULLA-DECISION-ENGINE-R079.5). Ids only:
    /// the reader joins them to content classes and counts, never reads content.
    pub fn cited_span_ids_since(
        &self,
        tenant_id: &str,
        since_ms: u64,
    ) -> Result<std::collections::BTreeSet<String>, String> {
        let mut cited = std::collections::BTreeSet::new();
        let mut after: Option<String> = None;
        loop {
            let page = self.decision_artifacts_page(
                tenant_id,
                RETRIEVAL_OUTCOME_PREFIX,
                after.as_deref(),
                OUTCOME_PAGE_ROWS,
            )?;
            let Some((last, _)) = page.last() else {
                return Ok(cited);
            };
            after = Some(last.clone());
            let full = page.len() == OUTCOME_PAGE_ROWS;
            for (_, bytes) in &page {
                let stored: medulla_decision_types::decision::statistical::retrieval::StoredRetrievalOutcome =
                    decode_artifact(bytes, "retrieval outcome")?;
                if stored.recorded_at_ms >= since_ms {
                    cited.extend(stored.outcome.cited.as_slice().iter().cloned());
                }
            }
            if !full {
                return Ok(cited);
            }
        }
    }
}

/// Decode one stored decision artifact.
pub fn decode_artifact<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
    noun: &str,
) -> Result<T, String> {
    medulla_types::msgpack::decode_bounded(
        bytes,
        medulla_types::msgpack::MsgpackLimits::new(16 * 1024 * 1024, 1_000_000, 64),
    )
    .map_err(|_| format!("CORRUPT_DECISION_ARTIFACT: invalid {noun}"))
}

/// Encode one decision artifact for storage.
pub fn encode_artifact<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, String> {
    rmp_serde::to_vec_named(value).map_err(|error| error.to_string())
}

/// The store key of one governed pointer row (EH-396, EH-397): `pointer:<key>`
/// where `<key>` is `champion:<kind>:<ref>` for a champion.
pub fn pointer_artifact_key(key: &str) -> String {
    format!("pointer:{key}")
}

/// A governed pointer's stored bytes (for the compare-and-set) and its state;
/// an absent pointer is the empty state of `key`. The one pointer read: the
/// decide handlers and the program job executor both call it
/// (MEDULLA-DECISION-ENGINE-R805.8).
pub fn read_pointer(
    store: &AgentLibraryStore,
    tenant: &str,
    key: &str,
) -> Result<
    (
        Option<Vec<u8>>,
        medulla_decision_policy_types::decision::statistical::retrieval_pointer::PointerState,
    ),
    String,
> {
    let Some(bytes) = store.decision_artifact(tenant, &pointer_artifact_key(key))? else {
        let state =
            medulla_decision_policy_types::decision::statistical::retrieval_pointer::PointerState {
                key: key.to_string(),
                ..Default::default()
            };
        return Ok((None, state));
    };
    let state = decode_artifact(&bytes, "pointer state")?;
    Ok((Some(bytes), state))
}

/// Key of a fitted rerank calibration curve (curve and receipt) by digest.
pub fn calibration_key(curve_digest: &str) -> String {
    format!("calibration:{curve_digest}")
}

/// The fitted calibration curve stored under `tenant` by digest.
pub fn read_calibration(
    store: &AgentLibraryStore,
    tenant: &str,
    curve_digest: &str,
) -> Result<
    Option<medulla_decision_types::decision::statistical::retrieval_calibration::CalibrationFitted>,
    String,
> {
    store
        .decision_artifact(tenant, &calibration_key(curve_digest))?
        .map(|bytes| decode_artifact(&bytes, "calibration curve"))
        .transpose()
}

/// The champion calibration (curve, receipt and digests) of `scorer_version`
/// under `tenant`: the
/// target of its governed pointer, or `None` when no curve is active. The one
/// read the rerank stage and the fit's qualification share.
pub fn champion_calibration(
    store: &AgentLibraryStore,
    tenant: &str,
    scorer_version: &str,
) -> Result<
    Option<medulla_decision_types::decision::statistical::retrieval_calibration::CalibrationFitted>,
    String,
> {
    use medulla_decision_types::decision::statistical::retrieval_calibration::champion_pointer_key;
    let (_, state) = read_pointer(store, tenant, &champion_pointer_key(scorer_version))?;
    let Some(digest) = state.target() else {
        return Ok(None);
    };
    read_calibration(store, tenant, digest)
}

/// Promotion by publish (EH-040): a `DecisionHead` revision is admitted only
/// when the receipt it names exists in this tenant, PASSED, and qualified
/// exactly these bytes (`head_digest` equals the revision's content digest).
/// Receipts are written once and never removed, so this check cannot race the
/// publish it guards.
pub fn require_head_receipt(
    store: &AgentLibraryStore,
    request: &medulla_agent_types::agent_component::AgentComponentPublishRequest,
) -> Result<(), String> {
    use medulla_decision_statistical_types::decision::statistical::errors::StatisticalErrorCode::EvaluationReceiptMismatch;
    if request.component.kind
        != medulla_agent_types::agent_component::AgentComponentKind::DecisionHead
    {
        return Ok(());
    }
    let Some(digest) = request.evaluation_receipt_digest.as_deref() else {
        return Err(
            EvaluationReceiptMismatch.refusal("a decision head names no evaluation receipt")
        );
    };
    let bytes = store
        .decision_artifact(&request.context.tenant_id, &receipt_key(digest))?
        .ok_or_else(|| {
            EvaluationReceiptMismatch.refusal("no evaluation receipt with that digest")
        })?;
    let receipt: medulla_decision_types::decision::jobs::DecisionEvalReceipt =
        decode_artifact(&bytes, "evaluation receipt")?;
    if !receipt.passed {
        return Err(EvaluationReceiptMismatch.refusal(format!(
            "the receipt failed its promotion gates: {}",
            receipt.failed_gates.as_slice().join(", ")
        )));
    }
    if receipt.head_digest != request.component.content_digest {
        return Err(EvaluationReceiptMismatch.refusal("the receipt qualified a different head"));
    }
    Ok(())
}
