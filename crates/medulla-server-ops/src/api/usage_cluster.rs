//! Data product proposals from recorded asset usage (MEDULLA-TYPED-PACKS-R805.7).
//!
//! The run reads the `asset_usage.reads` series (every read is one query of the asset, from the server's own dispatch and from source query logs) of one tenant and graph, turns each point
//! into an [`AssetUsage`] of its period, clusters them with
//! `medulla_core::data_product::propose_products` under the resolved knob
//! `data_product.cluster_min_principals`, and returns one proposal per cluster together with the
//! change request that carries it. The run reads no graph rows; glossary bindings come from the
//! caller of the job.

use std::collections::BTreeMap;
use std::sync::Arc;

use medulla_catalog_types::asset_usage::{AssetUsage, ASSET_USAGE_READS_SERIES};
use medulla_catalog_types::data_product::ProductProposal;
use medulla_catalog_types::stewardship::{
    ChangeRequest, StewardRole, StewardshipAssignment, StewardshipScope,
};
use medulla_core::data_product::propose_products;
use medulla_core::stewardship::{open_change_request, TargetScopes};
use medulla_server_api::state::ServerState;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// The verified target of one usage cluster run.
#[derive(Debug, Clone)]
pub struct UsageClusterRun {
    pub tenant: String,
    pub graph: String,
    /// `(asset, term)` bindings the caller read.
    pub term_bindings: Vec<(String, String)>,
}

/// One proposal and the open change request that carries it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProposalWithRequest {
    pub proposal: ProductProposal,
    pub request: ChangeRequest,
}

/// The change-request kind a product proposal opens.
pub const PRODUCT_PROPOSAL_KIND: &str = "data_product.proposal";

/// Open the change request of `proposal`: the owner candidate is asked to accept ownership, so
/// the candidate is the reviewer. The target is the proposed product's dataset list.
pub fn request_for(proposal: &ProductProposal) -> Result<ChangeRequest, String> {
    let target = format!("data_product:{}", proposal.datasets.join("+"));
    let digest = {
        let encoded = serde_json::to_vec(proposal).map_err(|error| error.to_string())?;
        format!(
            "sha256:{}",
            medulla_types::contract::Digest256::sha256(&encoded).to_hex()
        )
    };
    let owner = StewardshipAssignment {
        scope: StewardshipScope::Asset(target.clone()),
        role: StewardRole::Owner,
        principal: proposal.owner_candidate.clone(),
    };
    let scopes = TargetScopes {
        asset: Some(target.clone()),
        concept_scheme: None,
        domain: None,
    };
    open_change_request(&target, &digest, &[owner], &scopes)
        .map_err(|error| format!("CHANGE_REQUEST_INVALID: {error}"))
}

/// Cluster `usage` and open one change request per proposal. Pure over its inputs.
pub fn proposals_from_usage(
    usage: &[AssetUsage],
    bindings: &[(String, String)],
    min_principals: usize,
) -> Result<Vec<ProposalWithRequest>, String> {
    let mut terms: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (asset, term) in bindings {
        terms.entry(asset.clone()).or_default().push(term.clone());
    }
    propose_products(usage, &terms, min_principals)
        .into_iter()
        .map(|proposal| {
            let request = request_for(&proposal)?;
            Ok(ProposalWithRequest { proposal, request })
        })
        .collect()
}

/// Series name tags of one usage series: `(principal, asset)`.
fn usage_tags(series: &str) -> Option<(&str, &str)> {
    let rest = series
        .strip_prefix(ASSET_USAGE_READS_SERIES)?
        .strip_prefix("|principal=")?;
    rest.split_once("|asset=")
}

/// Run one usage cluster pass. Refusal: `USAGE_UNAVAILABLE` when the time-series store cannot be
/// read.
#[cfg(feature = "tsdb")]
pub async fn run_usage_cluster(
    state: &Arc<RwLock<ServerState>>,
    run: &UsageClusterRun,
) -> Result<Vec<ProposalWithRequest>, String> {
    use medulla_catalog_types::asset_usage::catalog_usage_period_s;
    use medulla_tsdb::store::SeriesKey;

    let store = state
        .read()
        .await
        .tsdb_store
        .clone()
        .ok_or_else(|| "USAGE_UNAVAILABLE: no time-series store".to_string())?;
    fn unavailable<E>(_: E) -> String {
        "USAGE_UNAVAILABLE: the usage store could not be read".to_string()
    }
    let catalog = store
        .scoped_catalog(&run.tenant, &run.graph)
        .map_err(unavailable)?;
    let period_ns = (catalog_usage_period_s() as i64).saturating_mul(1_000_000_000);
    let mut usage: BTreeMap<(String, String, i64), u64> = BTreeMap::new();
    for name in catalog.into_keys() {
        let Some((principal, asset)) = usage_tags(&name) else {
            continue;
        };
        let (principal, asset) = (principal.to_string(), asset.to_string());
        let key = SeriesKey::new(run.tenant.as_str(), run.graph.as_str(), name.clone());
        for point in store.scan_all_scoped(&key).map_err(unavailable)? {
            let count = point.values.first().copied().unwrap_or(0.0).max(0.0) as u64;
            let bucket = point.ts.div_euclid(period_ns.max(1));
            *usage
                .entry((asset.clone(), principal.clone(), bucket))
                .or_default() += count;
        }
    }
    let rows: Vec<AssetUsage> = usage
        .into_iter()
        .map(|((asset, principal, bucket), queries)| AssetUsage {
            asset,
            principal,
            period: bucket.to_string(),
            reads: queries,
            queries,
        })
        .collect();
    let min = medulla_server_api::knob_config::knobs().data_product_cluster_min_principals;
    proposals_from_usage(
        &rows,
        &run.term_bindings,
        usize::try_from(min).unwrap_or(usize::MAX),
    )
}

/// Without the time-series build no usage exists.
#[cfg(not(feature = "tsdb"))]
pub async fn run_usage_cluster(
    _state: &Arc<RwLock<ServerState>>,
    _run: &UsageClusterRun,
) -> Result<Vec<ProposalWithRequest>, String> {
    Err("USAGE_UNAVAILABLE: no time-series store".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(asset: &str, principal: &str, queries: u64) -> AssetUsage {
        AssetUsage {
            asset: asset.into(),
            principal: principal.into(),
            period: "1".into(),
            reads: queries,
            queries,
        }
    }

    // spec: MEDULLA-TYPED-PACKS-R805.7
    #[test]
    fn a_cluster_opens_an_open_change_request_for_its_owner_candidate() {
        let mut usage = Vec::new();
        for principal in ["a", "b", "c"] {
            usage.push(point("x", principal, 1));
            usage.push(point("y", principal, 2));
        }
        let found = proposals_from_usage(&usage, &[("x".into(), "t".into())], 3).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].proposal.terms, vec!["t"]);
        assert_eq!(found[0].request.reviewers, vec!["a"]);
        assert_eq!(
            found[0].request.state,
            medulla_types::change_request_machine::ChangeRequestState::Open
        );
    }
}
