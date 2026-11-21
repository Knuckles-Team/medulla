//! `Method::StandardsDrift` (MEDULLA-UNIFIED-DATA-PLANE-R701.3): the verified tenant's assets
//! scored against its standards, grouped into capability cohorts, with consolidation
//! proposals.
//!
//! The read runs over the caller's row-level-security projection of the request graph. Only
//! `Standard` and `StandardsAsset` nodes stamped with the verified tenant are candidates, so
//! another tenant's nodes are never scored. Rule evaluation is `drift_score`'s fact
//! membership; there is no second evaluator. Every recommendation is a proposal: the handler
//! writes nothing and queues no change.

use super::*;
use medulla_protocol_types::result_contract::ingestion::StandardsDrift;
use medulla_source_types::standards_drift::{
    drift_score, form_cohorts, recommend_consolidation, Standard, StandardsAsset,
    StandardsDriftResult, StandardsRefusal, MAX_STANDARDS_NODES, STANDARDS_ASSET_BODY_KEY,
    STANDARDS_ASSET_LABEL, STANDARDS_MIN_COHORT_SIZE, STANDARDS_OVERLAP_MIN, STANDARDS_TENANT_KEY,
    STANDARD_BODY_KEY, STANDARD_LABEL, STANDARDS_NODE_INVALID, STANDARD_NOT_FOUND,
};

/// The tenant's nodes of `label`, decoded from `body_key` and checked by `valid`. A node of
/// another tenant (or one whose properties do not decode, so it names no tenant) is not a
/// candidate; a candidate whose body does not decode or fails `valid` refuses the call.
fn tenant_nodes<T: serde::de::DeserializeOwned>(
    core: &GraphCore,
    label: &str,
    body_key: &str,
    tenant: &str,
    valid: impl Fn(&T) -> bool,
) -> Result<Vec<T>, String> {
    let mut out = Vec::new();
    for (node_id, blob) in core.get_nodes_by_label(label, MAX_STANDARDS_NODES) {
        let Ok(props) = medulla_types::msgpack::decode_property_object(&blob) else {
            continue;
        };
        if props.get(STANDARDS_TENANT_KEY).and_then(|v| v.as_str()) != Some(tenant) {
            continue;
        }
        let decoded = props
            .get(body_key)
            .and_then(|body| serde_json::from_value::<T>(body.clone()).ok())
            .filter(|body| valid(body));
        match decoded {
            Some(body) => out.push(body),
            None => return Err(format!("{STANDARDS_NODE_INVALID}: {node_id}")),
        }
    }
    Ok(out)
}

/// Score the verified tenant's assets; the pure core of [`handle_standards_drift`].
pub(crate) fn standards_drift(
    core: &GraphCore,
    tenant: &str,
    scope: Option<&str>,
    standard: Option<&str>,
) -> Result<StandardsDriftResult, String> {
    let mut standards: Vec<Standard> =
        tenant_nodes(core, STANDARD_LABEL, STANDARD_BODY_KEY, tenant, |_| true)?;
    let mut assets: Vec<StandardsAsset> = tenant_nodes(
        core,
        STANDARDS_ASSET_LABEL,
        STANDARDS_ASSET_BODY_KEY,
        tenant,
        |asset: &StandardsAsset| asset.tenant == tenant,
    )?;
    standards.sort_by(|a, b| a.id.cmp(&b.id));
    assets.sort_by(|a, b| a.id.cmp(&b.id));
    if let Some(id) = standard {
        standards.retain(|candidate| candidate.id == id);
        if standards.is_empty() {
            return Err(format!("{STANDARD_NOT_FOUND}: {id}"));
        }
    }
    if let Some(class) = scope {
        standards.retain(|candidate| candidate.capability_class == class);
        assets.retain(|asset| asset.capability_classes.contains(class));
    }

    let scores = assets
        .iter()
        .map(|asset| drift_score(asset, &standards))
        .collect();
    let cohorts = form_cohorts(&assets);
    let min_cohort = usize::try_from(STANDARDS_MIN_COHORT_SIZE.value()).unwrap_or(usize::MAX);
    let overlap_min = STANDARDS_OVERLAP_MIN.value();
    let mut recommendations = Vec::new();
    for cohort in cohorts.iter().filter(|c| c.assets.len() >= min_cohort) {
        let members: Vec<&StandardsAsset> = assets
            .iter()
            .filter(|asset| cohort.assets.contains(&asset.id))
            .collect();
        match recommend_consolidation(&cohort.capability_class, &members) {
            Ok(recommendation) if recommendation.overlap >= overlap_min => {
                recommendations.push(recommendation)
            }
            // A cohort with too few assets (or spanning tenants, which one tenant's read
            // cannot produce) is left out, never an error.
            Ok(_) | Err(StandardsRefusal::TooFewAssets | StandardsRefusal::CrossTenant { .. }) => {}
        }
    }
    Ok(StandardsDriftResult {
        scores,
        cohorts,
        recommendations,
    })
}

/// Serve one `StandardsDrift` request over the caller's projection of the request graph.
pub(crate) fn handle_standards_drift(
    ctx: &QueryHandlerCtx<'_>,
    scope: Option<String>,
    standard: Option<String>,
) -> Response {
    let Some((authority, carrier)) = ctx
        .read_authority
        .and_then(|authority| authority.carrier().map(|carrier| (authority, carrier)))
    else {
        return Response::err(
            ctx.req_id,
            "ACCESS_DENIED: standards drift needs a verified caller",
        );
    };
    let projected = authority.project_core(ctx.core);
    match standards_drift(
        &projected,
        carrier.tenant_scope(),
        scope.as_deref(),
        standard.as_deref(),
    ) {
        Ok(result) => Response::ok(ctx.req_id, ResultPayload::of::<StandardsDrift>(result)),
        Err(message) => Response::err(ctx.req_id, message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_source_types::standards_drift::{DriftValue, StandardRule};
    use std::collections::BTreeSet;

    fn put_standard(core: &GraphCore, tenant: &str, standard: &Standard) {
        let props = serde_json::json!({
            "type": STANDARD_LABEL,
            STANDARDS_TENANT_KEY: tenant,
            STANDARD_BODY_KEY: standard,
        });
        core.add_node(
            Standard::node_id(tenant, &standard.id),
            rmp_serde::to_vec_named(&props).unwrap(),
        );
    }

    fn put_asset(core: &GraphCore, tenant: &str, node_tenant: &str, id: &str, facts: &[&str]) {
        let asset = StandardsAsset {
            id: id.to_string(),
            tenant: tenant.to_string(),
            capability_classes: BTreeSet::from(["database".to_string()]),
            facts: facts.iter().map(|f| f.to_string()).collect(),
            cost: 10,
        };
        let props = serde_json::json!({
            "type": STANDARDS_ASSET_LABEL,
            STANDARDS_TENANT_KEY: node_tenant,
            STANDARDS_ASSET_BODY_KEY: asset,
        });
        core.add_node(
            StandardsAsset::node_id(node_tenant, id),
            rmp_serde::to_vec_named(&props).unwrap(),
        );
    }

    fn baseline() -> Standard {
        Standard {
            id: "db-baseline".to_string(),
            capability_class: "database".to_string(),
            rules: vec![StandardRule {
                id: "tls".to_string(),
                required_fact: "tls".to_string(),
                weight: 1.0,
            }],
        }
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R701.3
    #[test]
    fn tenant_nodes_are_scored_and_recommendations_stay_proposals() {
        let core = GraphCore::new();
        put_standard(&core, "tenant-a", &baseline());
        put_asset(&core, "tenant-a", "tenant-a", "orders-db", &["tls", "backup"]);
        put_asset(&core, "tenant-a", "tenant-a", "billing-db", &["tls", "backup"]);
        put_asset(&core, "tenant-b", "tenant-b", "other-db", &["tls", "backup"]);

        let result = standards_drift(&core, "tenant-a", None, None).unwrap();
        let scored: Vec<&str> = result.scores.iter().map(|s| s.asset.as_str()).collect();
        assert_eq!(scored, ["billing-db", "orders-db"]);
        assert!(result
            .scores
            .iter()
            .all(|s| matches!(s.score, DriftValue::Scored { score, .. } if score == 1.0)));
        assert_eq!(result.recommendations.len(), 1);
        let recommendation = &result.recommendations[0];
        assert!(recommendation.proposal_only);
        assert_eq!(recommendation.assets, ["billing-db", "orders-db"]);
        assert_eq!(recommendation.tenant, "tenant-a");

        // A filter narrows both the assets and the standards read.
        let none = standards_drift(&core, "tenant-a", Some("network"), None).unwrap();
        assert!(none.scores.is_empty() && none.recommendations.is_empty());
        // An unknown standard id refuses and names the id.
        let refusal = standards_drift(&core, "tenant-a", None, Some("absent")).unwrap_err();
        assert_eq!(refusal, "STANDARD_NOT_FOUND: absent");
        // Another tenant's standard is not a standard of this tenant.
        let refusal = standards_drift(&core, "tenant-b", None, Some("db-baseline")).unwrap_err();
        assert!(refusal.starts_with("STANDARD_NOT_FOUND"), "{refusal}");
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R701.3
    #[test]
    fn a_node_that_does_not_decode_or_names_another_tenant_refuses() {
        let core = GraphCore::new();
        put_asset(&core, "tenant-b", "tenant-a", "stolen-db", &["tls"]);
        let refusal = standards_drift(&core, "tenant-a", None, None).unwrap_err();
        assert_eq!(
            refusal,
            format!(
                "STANDARDS_NODE_INVALID: {}",
                StandardsAsset::node_id("tenant-a", "stolen-db")
            )
        );
        let core = GraphCore::new();
        let props = serde_json::json!({
            "type": STANDARD_LABEL,
            STANDARDS_TENANT_KEY: "tenant-a",
            STANDARD_BODY_KEY: {"id": 1},
        });
        core.add_node(
            Standard::node_id("tenant-a", "broken"),
            rmp_serde::to_vec_named(&props).unwrap(),
        );
        let refusal = standards_drift(&core, "tenant-a", None, None).unwrap_err();
        assert!(refusal.starts_with("STANDARDS_NODE_INVALID"), "{refusal}");
        // The same node under another tenant is not this tenant's candidate.
        assert!(standards_drift(&core, "tenant-b", None, None).is_ok());
    }
}
