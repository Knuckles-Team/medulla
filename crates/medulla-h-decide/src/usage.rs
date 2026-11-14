//! `Method::Usage`: the served per-tenant usage ledger
//! (MEDULLA-DECISION-ENGINE-R801.8).
//!
//! Both ops write under the tenant and principal of the verified request
//! context and need the service-only scope `usage:write`, so a human
//! principal is refused. `Append` writes one batch through
//! `AgentLibraryStore::append_usage`; `PutPrice` validates the catalog and
//! stores its entries as one revision in one transaction through
//! `AgentLibraryStore::put_prices`. Every refusal is
//! answered with its stable R801.4 code before anything is written.

use medulla_protocol_types::protocol::Response;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_types::usage::UsageOp;

use crate::decide::SharedState;

/// The scope both ops need (service-only, MEDULLA-DECISION-ENGINE-R801.3).
pub const USAGE_WRITE_SCOPE: &str = "usage:write";

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
    use super::{UsageOp, VerifiedRequestContext, USAGE_WRITE_SCOPE};
    use crate::decide::SharedState;
    use medulla_persistence_records::agent_library::AgentLibraryStateExt;
    use medulla_persistence_records::usage_ledger::UsageRefusal;
    use medulla_protocol_types::protocol::ResultPayload;
    use medulla_protocol_types::result_contract::coordination::{UsageAppend, UsagePutPrice};
    use medulla_types::usage::{
        PriceCatalog, UsageAppended, UsagePriceStored, UsageRecord, USAGE_SCHEMA_VERSION,
    };

    pub(super) async fn serve(
        state: &SharedState,
        verified: &VerifiedRequestContext,
        op: UsageOp,
    ) -> Result<ResultPayload, String> {
        if !verified.allows_action(USAGE_WRITE_SCOPE) {
            return Err(format!(
                "ACCESS_DENIED: Usage needs the service-only scope {USAGE_WRITE_SCOPE}"
            ));
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
