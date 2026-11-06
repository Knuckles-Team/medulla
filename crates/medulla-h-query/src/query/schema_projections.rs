//! `Method::SchemaProjections` list (MEDULLA-FEDERATED-QUERY-R724): one entry per
//! authorization boundary of the verified tenant that the caller's verified
//! token scopes cover.
//!
//! The read runs over the caller's row-level-security projection of the request
//! graph. Only `SchemaBoundary` nodes stamped with the verified tenant scope are
//! candidates, so another tenant's boundaries are never seen; a boundary whose
//! `required_scopes` the token does not hold is dropped. The tenant's total
//! boundary count is returned only to a token holding `audit:operator`. The
//! `Put` op is a write and commits through the mutation gateway.

use super::*;
use medulla_catalog_types::schema_boundary::{
    SchemaBoundary, SchemaProjections, SchemaProjectionsOp, BOUNDARY_COUNT_SCOPE,
    SCHEMA_BOUNDARY_LABEL, SCHEMA_BOUNDARY_TENANT_KEY,
};
use medulla_protocol_types::result_contract::graph::SchemaProjectionsList;

/// The most boundary nodes one read scans in a graph.
const MAX_BOUNDARY_NODES: usize = 65_536;

/// Decode one stored node into its boundary when it belongs to `tenant_scope`.
fn tenant_boundary(blob: &[u8], tenant_scope: &str) -> Option<SchemaBoundary> {
    let props = medulla_types::msgpack::decode_property_object(blob).ok()?;
    let owner = props.get(SCHEMA_BOUNDARY_TENANT_KEY)?.as_str()?;
    if owner != tenant_scope {
        return None;
    }
    serde_json::from_value(props.get("boundary")?.clone()).ok()
}

/// Project the tenant's boundaries onto what `sorted_scopes` covers.
pub(crate) fn project_boundaries(
    core: &GraphCore,
    tenant_scope: &str,
    sorted_scopes: &[String],
) -> SchemaProjections {
    let all: Vec<SchemaBoundary> = core
        .get_nodes_by_label(SCHEMA_BOUNDARY_LABEL, MAX_BOUNDARY_NODES)
        .iter()
        .filter_map(|(_, blob)| tenant_boundary(blob, tenant_scope))
        .collect();
    let operator = sorted_scopes
        .binary_search_by(|scope| scope.as_str().cmp(BOUNDARY_COUNT_SCOPE))
        .is_ok();
    let boundary_count = operator.then_some(all.len() as u64);
    let mut entries: Vec<SchemaBoundary> = all
        .into_iter()
        .filter(|boundary| boundary.covered_by(sorted_scopes))
        .collect();
    entries.sort_by(|a, b| a.boundary_id.cmp(&b.boundary_id));
    SchemaProjections {
        entries,
        boundary_count,
    }
}

/// Serve one `SchemaProjections` op; a `Put` is handed back to the gateway.
pub(crate) fn handle_schema_projections(
    ctx: &QueryHandlerCtx<'_>,
    op: Box<SchemaProjectionsOp>,
) -> Result<Response, Method> {
    if op.is_mutation() {
        return Err(Method::SchemaProjections { op });
    }
    let Some((authority, carrier)) = ctx
        .read_authority
        .and_then(|authority| authority.carrier().map(|carrier| (authority, carrier)))
    else {
        return Ok(Response::err(
            ctx.req_id,
            "ACCESS_DENIED: schema projections need a verified caller",
        ));
    };
    let projected = authority.project_core(ctx.core);
    let projections = project_boundaries(&projected, carrier.tenant_scope(), carrier.scopes());
    Ok(Response::ok(
        ctx.req_id,
        ResultPayload::of::<SchemaProjectionsList>(projections),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(core: &GraphCore, tenant: &str, id: &str, scope: &str) {
        let boundary = SchemaBoundary {
            boundary_id: id.into(),
            required_scopes: vec![scope.into()],
            tables: vec![format!("{id}_table")],
            mapping_ref: "urn:mapping".into(),
        };
        let props = serde_json::json!({
            "type": SCHEMA_BOUNDARY_LABEL,
            SCHEMA_BOUNDARY_TENANT_KEY: tenant,
            "boundary": boundary,
        });
        core.add_node(
            SchemaBoundary::node_id(tenant, id),
            rmp_serde::to_vec_named(&props).unwrap(),
        );
    }

    fn scopes(names: &[&str]) -> Vec<String> {
        let mut out: Vec<String> = names.iter().map(|s| s.to_string()).collect();
        out.sort();
        out
    }

    // spec: MEDULLA-FEDERATED-QUERY-R724
    #[test]
    fn projection_filters_by_scopes_and_tenant() {
        let core = GraphCore::new();
        put(&core, "tenant-a", "sales", "sales:read");
        put(&core, "tenant-a", "hr", "hr:read");
        put(&core, "tenant-b", "other", "sales:read");
        let cases: &[(&str, &[&str], &[&str], Option<u64>)] = &[
            ("tenant-a", &["sales:read"], &["sales"], None),
            (
                "tenant-a",
                &["sales:read", "audit:operator"],
                &["sales"],
                Some(2),
            ),
            ("tenant-b", &["hr:read"], &[], None),
        ];
        for (tenant, held, expected, count) in cases {
            let out = project_boundaries(&core, tenant, &scopes(held));
            let ids: Vec<&str> = out.entries.iter().map(|b| b.boundary_id.as_str()).collect();
            assert_eq!(ids, *expected);
            assert_eq!(out.boundary_count, *count);
        }
    }
}
