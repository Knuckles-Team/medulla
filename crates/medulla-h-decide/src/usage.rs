//! `Method::Usage`: the served per-tenant usage ledger
//! (MEDULLA-DECISION-ENGINE-R801.8).
//!
//! `Append` and `PutPrice` write under the tenant and principal of the verified request
//! context and need the service-only scope `usage:write`, so a human
//! principal is refused; `Read` (R801.7) needs `usage:read` and only ever reads
//! the verified tenant. `Append` writes one batch through
//! `AgentLibraryStore::append_usage`; `PutPrice` validates the catalog and
//! stores its entries as one revision in one transaction through
//! `AgentLibraryStore::put_prices`. Every refusal is
//! answered with its stable R801.4 code before anything is written.

use medulla_decision_policy_types::usage::UsageOp;
use medulla_protocol_types::protocol::Response;
use medulla_server_api::authority_context::VerifiedRequestContext;

use medulla_h_decide_base::decide::SharedState;

/// Append usage records or store a price catalog for the verified tenant.
pub async fn handle_usage(
    state: &SharedState,
    req_id: u64,
    verified: &VerifiedRequestContext,
    op: UsageOp,
) -> Response {
    #[cfg(feature = "redb")]
    {
        match served::serve(state, verified, op).await {
            Ok(payload) => Response::ok(req_id, payload),
            Err(error) => Response::err(req_id, error),
        }
    }
    #[cfg(not(feature = "redb"))]
    {
        let _ = (state, verified, op);
        Response::err(req_id, "Usage requires the `redb` feature")
    }
}

#[cfg(feature = "redb")]
mod served {
    use super::{UsageOp, VerifiedRequestContext};
    use medulla_h_decide_base::decide::SharedState;
    use medulla_persistence_records::agent_library::AgentLibraryStateExt;
    use medulla_persistence_records::usage_ledger::UsageRefusal;
    use medulla_persistence_records::usage_ledger::{DAY_MS, USAGE_READ_PAGE};
    use medulla_protocol_types::protocol::ResultPayload;
    use medulla_protocol_types::result_contract::coordination::{
        UsageAppend, UsagePutPrice, UsageRead,
    };
    use {
        medulla_decision_policy_types::usage::PriceCatalog,
        medulla_decision_policy_types::usage::UsageAppended,
        medulla_decision_policy_types::usage::UsagePriceStored,
        medulla_decision_policy_types::usage::UsageReadResult,
        medulla_decision_policy_types::usage::UsageReadView,
        medulla_decision_policy_types::usage::UsageRecord,
        medulla_decision_policy_types::usage::USAGE_SCHEMA_VERSION,
    };

    pub(super) async fn serve(
        state: &SharedState,
        verified: &VerifiedRequestContext,
        op: UsageOp,
    ) -> Result<ResultPayload, String> {
        let scope = op.required_scope();
        if !verified.allows_action(scope) {
            return Err(format!("ACCESS_DENIED: Usage needs the scope {scope}"));
        }
        let store = state.write().await.ensure_agent_library()?;
        let tenant = verified.tenant();
        let refused = |refusal: UsageRefusal| refusal.to_string();
        match op {
            UsageOp::Append { records } => {
                let principal = verified.principal_persistence_id();
                let records: Vec<UsageRecord> = records
                    .into_iter()
                    .map(|record| UsageRecord {
                        principal: principal.clone(),
                        ..record
                    })
                    .collect();
                let catalog = PriceCatalog {
                    aliases: std::collections::BTreeMap::new(),
                    entries: store.price_entries(tenant).map_err(refused)?,
                };
                let now_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
                let receipt = store
                    .append_usage(tenant, &principal, &catalog, &records, now_ms)
                    .map_err(refused)?;
                ResultPayload::of::<UsageAppend>(UsageAppended {
                    appended: receipt.appended as u64,
                    batch_digest: receipt.batch_digest,
                })
            }
            UsageOp::Read {
                from_ms,
                to_ms,
                model_key,
                view,
                group_by,
                cursor,
            } => {
                if from_ms > to_ms {
                    return Err(format!(
                        "PARAMETER_INVALID: from_ms {from_ms} is after to_ms {to_ms}"
                    ));
                }
                let result = match view {
                    UsageReadView::Records => {
                        let (records, next_cursor) = store
                            .usage_records(
                                tenant,
                                from_ms,
                                to_ms,
                                model_key.as_deref(),
                                cursor.as_deref(),
                                USAGE_READ_PAGE,
                            )
                            .map_err(refused)?;
                        UsageReadResult::Records {
                            records,
                            next_cursor,
                        }
                    }
                    UsageReadView::Rollup => {
                        let catalog = PriceCatalog {
                            aliases: std::collections::BTreeMap::new(),
                            entries: store.price_entries(tenant).map_err(refused)?,
                        };
                        let wanted = model_key.as_deref().map(|raw| {
                            medulla_persistence_records::usage_ledger::normalize_model_key(
                                &catalog, raw,
                            )
                        });
                        let mut days = store
                            .usage_rollup(tenant, &catalog, from_ms / DAY_MS, to_ms / DAY_MS)
                            .map_err(refused)?
                            .days;
                        if let Some(wanted) = wanted {
                            days.retain(|day| day.model_key == wanted);
                        }
                        let groups = match group_by {
                            Some(group_by) => store
                                .usage_groups(tenant, &catalog, from_ms, to_ms, group_by)
                                .map_err(refused)?,
                            None => Vec::new(),
                        };
                        let index_watermark_ms = store.usage_watermark(tenant).map_err(refused)?;
                        UsageReadResult::Rollup {
                            days,
                            groups,
                            index_watermark_ms,
                        }
                    }
                };
                ResultPayload::of::<UsageRead>(result)
            }
            UsageOp::PutPrice { catalog } => {
                let mut entries = catalog.entries;
                if let Some(entry) = entries
                    .iter()
                    .find(|e| e.schema_version != USAGE_SCHEMA_VERSION || e.model_key.is_empty())
                {
                    return Err(format!(
                        "PARAMETER_INVALID: price entry for {:?} needs schema_version \
                         {USAGE_SCHEMA_VERSION} and a model key",
                        entry.model_key
                    ));
                }
                if let Some(entry) = entries.iter().find(|e| {
                    !(e.currency.is_empty()
                        || e.currency.len() == 3
                            && e.currency.bytes().all(|b| b.is_ascii_uppercase()))
                }) {
                    return Err(format!(
                        "PARAMETER_INVALID: price currency {:?} for {:?} is not an ISO 4217 code",
                        entry.currency, entry.model_key
                    ));
                }
                entries.sort_by(|a, b| {
                    (a.model_key.as_str(), a.valid_from_ms)
                        .cmp(&(b.model_key.as_str(), b.valid_from_ms))
                });
                store.put_prices(tenant, &entries).map_err(refused)?;
                ResultPayload::of::<UsagePutPrice>(UsagePriceStored {
                    entries: entries.len() as u64,
                })
            }
        }
    }
}

#[cfg(all(test, feature = "decide"))]
mod tests;
