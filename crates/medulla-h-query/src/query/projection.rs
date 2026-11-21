//! `Method::ProjectionRead` (MEDULLA-UNIFIED-DATA-PLANE-R026.4.1): one table of a
//! graph-backed relational projection, read through an approved, versioned mapping.
//!
//! A mapping revision is one `ProjectionMapping` node of the caller's tenant
//! (`projection_mapping:<tenant>:<mapping id>:<revision>`). The read resolves the named
//! revision, refuses it when it is not approved (the typed approval state of
//! `medulla_plan::virtual_graph_mapping`) or when a newer revision replaced it, then scans
//! the nodes of the table's label that the caller may see, builds typed rows, filters,
//! orders by primary key and digests them. The answer binds the mapping revision and the
//! graph revision (`GraphCore::version`) the rows were read at. The handler never writes.

use medulla_plan::virtual_graph_mapping::{MappingApproval, VirtualGraphMapping};
use medulla_protocol_types::result_contract::query as results;
use medulla_server_api::read_scope::ReadCaller;
use medulla_types::contract::ServerErrorCode;
use medulla_types::projection_read::{
    evaluate, projection_mapping_node_id, projection_mapping_prefix, row_of, ProjectionMapping,
    ProjectionReadRequest, ProjectionReadResponse, PROJECTION_MAPPING_LABEL,
};
use serde_json::Value;

use super::*;

/// One refusal, already rendered as `<CODE>: <detail>`.
struct Refused(String);

fn refuse(code: ServerErrorCode, detail: impl std::fmt::Display) -> Refused {
    Refused(format!("{}: {detail}", code.as_str()))
}

fn props_of(core: &GraphCore, node: &str) -> Option<serde_json::Map<String, Value>> {
    let blob = core.get_node_properties(node)?;
    medulla_types::msgpack::decode_property_object(&blob).ok()
}

fn label_of(props: &serde_json::Map<String, Value>) -> Option<&str> {
    ["node_type", "type"]
        .iter()
        .find_map(|key| props.get(*key).and_then(Value::as_str))
}

fn approval_state(text: &str) -> MappingApproval {
    match text {
        "approved" => MappingApproval::Approved,
        "model-suggested" => MappingApproval::ModelSuggested,
        _ => MappingApproval::Proposed,
    }
}

/// The verified tenant of the read, or a denial when the request is not the caller's own.
fn verified_tenant(
    ctx: &QueryHandlerCtx<'_>,
    request: &ProjectionReadRequest,
) -> Result<String, Refused> {
    let caller = ReadCaller::from_authority(ctx.read_authority, ctx.graph_name);
    let tenant = caller.principal.tenant;
    if caller.tenant.is_none() {
        return Err(refuse(
            ServerErrorCode::ProjectionAccessDenied,
            "the read carries no verified tenant",
        ));
    }
    if ctx.read_authority.is_none()
        || request.tenant != tenant
        || request.actor != ctx.caller
        || request.graph != ctx.graph_name
    {
        return Err(refuse(
            ServerErrorCode::ProjectionAccessDenied,
            "tenant, actor and graph must be the verified caller's own",
        ));
    }
    Ok(tenant)
}

/// Resolve the named mapping revision: visible, approved and not replaced.
fn resolve_mapping(
    ctx: &QueryHandlerCtx<'_>,
    tenant: &str,
    request: &ProjectionReadRequest,
) -> Result<ProjectionMapping, Refused> {
    let node = projection_mapping_node_id(tenant, &request.mapping_id, request.mapping_revision);
    let visible = ctx
        .read_authority
        .is_some_and(|authority| authority.node_visible(ctx.core, &node));
    let stored = if visible {
        props_of(ctx.core, &node)
            .filter(|props| label_of(props) == Some(PROJECTION_MAPPING_LABEL))
            .and_then(|props| {
                serde_json::from_value::<ProjectionMapping>(Value::Object(props)).ok()
            })
    } else {
        None
    };
    let Some(mapping) = stored else {
        return Err(refuse(
            ServerErrorCode::ProjectionMappingUnapproved,
            format!(
                "mapping `{}` revision {} is not an approved mapping of this tenant",
                request.mapping_id, request.mapping_revision
            ),
        ));
    };
    VirtualGraphMapping {
        name: format!("{}@{}", mapping.mapping_id, mapping.revision),
        approval_state: approval_state(&mapping.approval_state),
        r2rml: String::new(),
    }
    .queryable_name()
    .map_err(|error| refuse(ServerErrorCode::ProjectionMappingUnapproved, error))?;
    let prefix = projection_mapping_prefix(tenant, &request.mapping_id);
    let newest = ctx
        .core
        .node_ids()
        .iter()
        .filter_map(|id| id.strip_prefix(&prefix)?.parse::<u64>().ok())
        .max()
        .unwrap_or(mapping.revision);
    if newest > mapping.revision {
        return Err(refuse(
            ServerErrorCode::ProjectionMappingStale,
            format!(
                "mapping `{}` revision {} was replaced by revision {newest}",
                request.mapping_id, mapping.revision
            ),
        ));
    }
    Ok(mapping)
}

fn read(
    ctx: &QueryHandlerCtx<'_>,
    request: ProjectionReadRequest,
) -> Result<ProjectionReadResponse, Refused> {
    let tenant = verified_tenant(ctx, &request)?;
    let mapping = resolve_mapping(ctx, &tenant, &request)?;
    let invalid = |detail: String| Refused(format!("INVALID_ARGUMENT: {detail}"));
    if !request.query_digest.is_empty() && request.query_digest != request.computed_query_digest() {
        return Err(invalid("query_digest does not match the request".into()));
    }
    let table = mapping
        .tables
        .iter()
        .find(|table| table.name == request.table)
        .ok_or_else(|| invalid(format!("mapping names no table `{}`", request.table)))?;
    // The graph revision is read before the rows: a commit between the two is in the rows
    // and the next read reports a later revision, never an earlier one.
    let graph_revision = ctx.core.version();
    let authority = ctx.read_authority;
    let mut rows = Vec::new();
    for id in ctx.core.node_ids() {
        let Some(props) = props_of(ctx.core, &id) else {
            continue;
        };
        if label_of(&props) != Some(table.label.as_str()) {
            continue;
        }
        if props
            .get("_tenant")
            .and_then(Value::as_str)
            .is_some_and(|owner| owner != tenant)
        {
            continue;
        }
        if !authority.is_some_and(|a| a.node_visible(ctx.core, &id)) {
            continue;
        }
        rows.push(row_of(table, &props));
    }
    let (column_types, rows, result_digest) =
        evaluate(table, &request, rows).map_err(|error| invalid(error.to_string()))?;
    Ok(ProjectionReadResponse {
        mapping_revision: mapping.revision,
        graph_revision,
        column_types,
        rows,
        result_digest,
    })
}

/// `ProjectionRead`: read one projected table through an approved mapping revision.
pub(crate) fn handle_projection_read(
    ctx: &QueryHandlerCtx<'_>,
    request: ProjectionReadRequest,
) -> Response {
    match read(ctx, request)
        .map_err(|refusal| refusal.0)
        .and_then(ResultPayload::of::<results::ProjectionRead>)
    {
        Ok(payload) => Response::ok(ctx.req_id, payload),
        Err(error) => Response::err(ctx.req_id, error),
    }
}
