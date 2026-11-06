//! Served schema boundary writes (MEDULLA-FEDERATED-QUERY-R724).
//!
//! `SchemaProjections::Put` is gateway-routed (`Route::SchemaProjections`): the
//! record's bounds are checked, every required scope must be a registered scope,
//! and `mapping_ref` must name a `VirtualMapping` node of the request graph. The
//! admitted record commits as the one `AddNode` upsert of node
//! `SchemaBoundary:<tenant scope>:<boundary_id>` through the coalescable
//! gateway, so the durable log, audit and CDC see an ordinary node write. A
//! refused put writes nothing. `List` is a read served by the query surface and
//! never reaches this module.

use super::*;
use medulla_catalog_types::schema_boundary::{
    SchemaBoundary, SchemaProjectionsOp, SCHEMA_BOUNDARY_LABEL, SCHEMA_BOUNDARY_TENANT_KEY,
};
use medulla_protocol_types::result_contract::graph::SchemaProjectionsPut;
use medulla_types::contract::ServerErrorCode;

/// The node classes a `mapping_ref` may resolve to.
const MAPPING_CLASSES: &[&str] = &[
    "VirtualMapping",
    "vg:VirtualMapping",
    "http://knuckles.team/kg/virtual-graph#VirtualMapping",
];

fn refuse(code: ServerErrorCode, detail: impl std::fmt::Display) -> String {
    format!("{}: {detail}", code.as_str())
}

fn check_scopes(boundary: &SchemaBoundary) -> Result<(), String> {
    match boundary
        .required_scopes
        .iter()
        .find(|scope| medulla_capabilities::scopes::scope_entry(scope).is_none())
    {
        Some(scope) => Err(refuse(
            ServerErrorCode::SchemaBoundaryInvalid,
            format!("required scope '{scope}' is not a registered scope"),
        )),
        None => Ok(()),
    }
}

fn check_mapping(core: &GraphCore, mapping_ref: &str) -> Result<(), String> {
    let class = core
        .get_node_properties(mapping_ref)
        .and_then(|blob| medulla_types::msgpack::decode_property_object(&blob).ok())
        .and_then(|props| {
            props
                .get("type")
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        });
    match class {
        Some(class) if MAPPING_CLASSES.contains(&class.as_str()) => Ok(()),
        _ => Err(refuse(
            ServerErrorCode::BoundaryMappingUnknown,
            format!("{mapping_ref} names no VirtualMapping of this graph"),
        )),
    }
}

/// Admit one boundary put and return the `AddNode` upsert it commits as.
pub(super) fn admit_boundary(
    core: &GraphCore,
    tenant_scope: &str,
    boundary: &SchemaBoundary,
) -> Result<Method, String> {
    boundary.check_shape().map_err(|error| {
        refuse(
            ServerErrorCode::SchemaBoundaryInvalid,
            format!("{error:?} out of bounds"),
        )
    })?;
    check_scopes(boundary)?;
    check_mapping(core, &boundary.mapping_ref)?;
    let props = serde_json::json!({
        "type": SCHEMA_BOUNDARY_LABEL,
        SCHEMA_BOUNDARY_TENANT_KEY: tenant_scope,
        "boundary": boundary,
    });
    let properties_msgpack = rmp_serde::to_vec_named(&props)
        .map_err(|error| format!("INVALID_ARGUMENT: schema boundary encoding failed: {error}"))?;
    Ok(Method::AddNode {
        node_id: SchemaBoundary::node_id(tenant_scope, &boundary.boundary_id),
        properties_msgpack,
    })
}

/// The `Route::SchemaProjections` entry: admit the put, then commit it as the
/// `AddNode` upsert through the coalescable gateway.
pub(super) async fn handle_boundary_put(
    ctx: &MutationCtx<'_>,
    method: &Method,
) -> Option<Response> {
    let Method::SchemaProjections { op } = method else {
        return None;
    };
    let SchemaProjectionsOp::Put { boundary } = op.as_ref() else {
        return Some(Response::err(
            ctx.req_id,
            "INVALID_ARGUMENT: SchemaProjections list is served by the query surface",
        ));
    };
    let write = match admit_boundary(ctx.core, ctx.tenant_scope, boundary) {
        Ok(write) => write,
        Err(error) => return Some(Response::err(ctx.req_id, error)),
    };
    let plan = MutationPlan::for_method(&write);
    let owned = write.clone();
    let stored = boundary.clone();
    Some(
        super::gateway::commit_gateway_coalescable(ctx, &plan, &write, move |core| {
            medulla_server_mutation::mutation::apply_coalescable_write(core, &owned)?;
            ResultPayload::of::<SchemaProjectionsPut>(stored)
        })
        .await,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn core_with_mapping(id: &str, class: &str) -> GraphCore {
        let core = GraphCore::new();
        let props = rmp_serde::to_vec_named(&serde_json::json!({ "type": class })).unwrap();
        core.add_node(id.to_string(), props);
        core
    }

    fn boundary(scope: &str, mapping_ref: &str) -> SchemaBoundary {
        SchemaBoundary {
            boundary_id: "sales".into(),
            required_scopes: vec![scope.into()],
            tables: vec!["orders".into()],
            mapping_ref: mapping_ref.into(),
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R724
    #[test]
    fn put_admission_refuses_unknown_mapping_and_unregistered_scope() {
        let core = core_with_mapping("urn:mapping", "vg:VirtualMapping");
        let cases: &[(SchemaBoundary, Option<&str>)] = &[
            (boundary("kg:read", "urn:mapping"), None),
            (
                boundary("kg:read", "urn:absent"),
                Some("BOUNDARY_MAPPING_UNKNOWN"),
            ),
            (
                boundary("no:such-scope", "urn:mapping"),
                Some("SCHEMA_BOUNDARY_INVALID"),
            ),
        ];
        for (record, refusal) in cases {
            let outcome = admit_boundary(&core, "tenant-a", record);
            match refusal {
                None => assert!(matches!(outcome, Ok(Method::AddNode { ref node_id, .. })
                    if node_id == "SchemaBoundary:tenant-a:sales")),
                Some(code) => assert!(outcome.unwrap_err().starts_with(code)),
            }
        }
    }
}
