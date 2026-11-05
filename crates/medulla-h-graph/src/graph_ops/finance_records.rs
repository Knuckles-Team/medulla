//! Served finance-v1 records (MEDULLA-FINANCE-PRIMITIVES-R004.3.2).
//!
//! `FinanceRecordPut` is gateway-routed (`Route::FinanceRecords`): the declared
//! schema version is admitted first (`admit_finance_record_version`), then the
//! kind policy (`FINANCE_RECORD_POLICY`) refuses a derived kind, the record's
//! tenant must be the verified request tenant, the record must conform to its
//! `<Kind>Shape` in `finance-v1.shapes.ttl`, and the writer's expected head must
//! match the stored one. The admitted record then commits as the one `AddNode`
//! upsert through the same coalescable gateway `Method::AddNode` uses, so the
//! durable log, audit and CDC see an ordinary node write.
//!
//! `FinanceRecordGet` is a terminal read over the RLS-projected core: the stored
//! record node, refused with `ACCESS_DENIED` when it belongs to another tenant.

use super::*;

use super::terminal::GraphOpsContext;

/// The node property naming the opaque verified tenant scope of the writer.
const TENANT_SCOPE_KEY: &str = "finance_tenant_scope";
/// The node property holding the record head (a per-record revision counter).
#[cfg(all(feature = "finance", feature = "shacl"))]
const HEAD_KEY: &str = "finance_head";

#[cfg(all(feature = "finance", feature = "shacl"))]
const FINANCE_SHAPES: &str = include_str!("../../../medulla-core/ontology/finance-v1.shapes.ttl");

#[cfg(all(feature = "finance", feature = "shacl"))]
const TURTLE_PREFIXES: &str = "@prefix : <http://knuckles.team/kg#> .\n\
     @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n";

/// `INVALID_ARGUMENT`/`READ_ONLY`/`CONFLICT`/`ACCESS_DENIED` carry the declared
/// class code; the finance refusal code leads the detail.
#[cfg(all(feature = "finance", feature = "shacl"))]
fn refuse(class: &str, code: &str, detail: impl std::fmt::Display) -> String {
    format!("{class}: {code}: {detail}")
}

#[cfg(all(feature = "finance", feature = "shacl"))]
fn is_local_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Each field value must be exactly one Turtle object term, so a value can never
/// smuggle extra triples into the validated document.
#[cfg(all(feature = "finance", feature = "shacl"))]
fn single_object_term(value: &str) -> bool {
    let probe = format!("{TURTLE_PREFIXES}<urn:medulla:probe> <urn:medulla:probe> {value} .\n");
    matches!(medulla_rdf::mapping::parse_turtle(&probe), Ok(triples) if triples.len() == 1)
}

/// Validate a record against its `<Kind>Shape`.
#[cfg(all(feature = "finance", feature = "shacl"))]
pub(super) fn check_record_shape(
    kind: medulla_types::wire_finance::FinanceRecordKind,
    record: &medulla_types::wire_finance::FinanceRecord,
) -> Result<(), String> {
    use medulla_types::wire_finance::finance_record_refusal::SHAPE_INVALID;
    if !is_local_name(&record.record_id) {
        return Err(refuse(
            "INVALID_ARGUMENT",
            SHAPE_INVALID,
            "record_id must be a non-empty [A-Za-z0-9_] name",
        ));
    }
    let mut data = format!(
        "{TURTLE_PREFIXES}<urn:medulla:finance:{}> a :{}",
        record.record_id,
        kind.class_name()
    );
    for (name, value) in &record.fields {
        if !is_local_name(name) || !single_object_term(value) {
            return Err(refuse(
                "INVALID_ARGUMENT",
                SHAPE_INVALID,
                format!("field '{name}' is not one finance property term"),
            ));
        }
        data.push_str(&format!(" ;\n    :{name} {value}"));
    }
    data.push_str(" .\n");
    let report = medulla_shacl::validate_turtle(FINANCE_SHAPES, &data)
        .map_err(|error| refuse("INVALID_ARGUMENT", SHAPE_INVALID, error))?;
    if report.conforms {
        Ok(())
    } else {
        Err(refuse(
            "INVALID_ARGUMENT",
            SHAPE_INVALID,
            format!(
                "{} violates {}Shape ({} result(s))",
                record.record_id,
                kind.class_name(),
                report.results.len()
            ),
        ))
    }
}

#[cfg(all(feature = "finance", feature = "shacl"))]
fn stored_props(
    core: &GraphCore,
    node_id: &str,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    core.get_node_properties(node_id)
        .and_then(|blob| medulla_types::msgpack::decode_property_object(&blob).ok())
}

/// Admit one `FinanceRecordPut` and return the `AddNode` upsert it commits as,
/// with the new head.
#[cfg(all(feature = "finance", feature = "shacl"))]
pub(super) fn admit_put(
    core: &GraphCore,
    tenant_id: &str,
    tenant_scope: &str,
    method: &Method,
) -> Result<(Method, u64), String> {
    use medulla_types::wire_finance::{
        admit_finance_record_version, finance_record_refusal as code, FinanceRecordPolicy,
    };
    let Method::FinanceRecordPut {
        kind,
        schema_version,
        record,
        expected_head,
    } = method
    else {
        unreachable!("admit_put is only called for FinanceRecordPut");
    };
    admit_finance_record_version(schema_version)
        .map_err(|refusal| refuse("INVALID_ARGUMENT", code::VERSION_UNKNOWN, refusal.reason))?;
    if kind.policy() == FinanceRecordPolicy::Derived {
        return Err(refuse(
            "READ_ONLY",
            code::DERIVED_READONLY,
            format!("{} records are derived by the engine", kind.class_name()),
        ));
    }
    if record.tenant != tenant_id {
        return Err(format!(
            "ACCESS_DENIED: a {} record for another tenant cannot be written",
            kind.class_name()
        ));
    }
    check_record_shape(*kind, record)?;
    let node_id = kind.node_id(&record.record_id);
    let stored = stored_props(core, &node_id);
    if let Some(props) = &stored {
        if props.get(TENANT_SCOPE_KEY).and_then(|v| v.as_str()) != Some(tenant_scope) {
            return Err(format!(
                "ACCESS_DENIED: {node_id} belongs to another tenant"
            ));
        }
    }
    let current_head = stored
        .as_ref()
        .and_then(|props| props.get(HEAD_KEY))
        .and_then(|v| v.as_u64());
    if current_head != *expected_head {
        return Err(refuse(
            "CONFLICT",
            code::HEAD_CONFLICT,
            format!("{node_id} head is {current_head:?}, the writer expected {expected_head:?}"),
        ));
    }
    let head = current_head.unwrap_or(0) + 1;
    let props = serde_json::json!({
        "type": kind.class_name(),
        "finance_kind": kind.class_name(),
        "record_id": record.record_id,
        "schema_version": schema_version,
        "tenant": record.tenant,
        TENANT_SCOPE_KEY: tenant_scope,
        HEAD_KEY: head,
        "fields": record.fields,
    });
    let properties_msgpack = rmp_serde::to_vec_named(&props)
        .map_err(|error| format!("INVALID_ARGUMENT: finance record encoding failed: {error}"))?;
    Ok((
        Method::AddNode {
            node_id,
            properties_msgpack,
        },
        head,
    ))
}

/// The `Route::FinanceRecords` entry: admit the put, then commit it as the
/// `AddNode` upsert through the coalescable gateway.
pub(super) async fn handle_record(
    ctx: &MutationCtx<'_>,
    tenant_id: &str,
    method: &Method,
) -> Option<Response> {
    if !matches!(method, Method::FinanceRecordPut { .. }) {
        return None;
    }
    #[cfg(all(feature = "finance", feature = "shacl"))]
    {
        let (upsert, head) = match admit_put(ctx.core, tenant_id, ctx.tenant_scope, method) {
            Ok(admitted) => admitted,
            Err(error) => return Some(Response::err(ctx.req_id, error)),
        };
        let plan = MutationPlan::for_method(&upsert);
        let owned = upsert.clone();
        let response =
            super::gateway::commit_gateway_coalescable(ctx, &plan, &upsert, move |core| {
                medulla_server_mutation::mutation::apply_coalescable_write(core, &owned)
                    .map(|_| ResultPayload::Count(head))
            })
            .await;
        Some(response)
    }
    #[cfg(not(all(feature = "finance", feature = "shacl")))]
    {
        let _ = tenant_id;
        Some(Response::err(
            ctx.req_id,
            "METHOD_NOT_YET_SERVED: FinanceRecordPut needs the finance and shacl features",
        ))
    }
}

/// `FinanceRecordGet` over the RLS-projected core (`terminal.rs`).
pub(super) async fn handle_record_get(
    ctx: GraphOpsContext<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    let Method::FinanceRecordGet { kind, record_id } = method else {
        return ControlFlow::Continue(method);
    };
    let node_id = kind.node_id(&record_id);
    let props = ctx.core.get_node_properties(&node_id);
    if let Some(blob) = &props {
        let owner_scope = medulla_types::msgpack::decode_property_object(blob)
            .ok()
            .and_then(|props| {
                props
                    .get(TENANT_SCOPE_KEY)
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
            });
        let caller_scope = ctx
            .read_authority
            .carrier()
            .map(|carrier| carrier.tenant_scope().to_owned());
        if owner_scope.is_none() || owner_scope != caller_scope {
            return ControlFlow::Break(Response::err(
                ctx.req_id,
                format!("ACCESS_DENIED: {node_id} belongs to another tenant"),
            ));
        }
    }
    ControlFlow::Break(Response::ok(
        ctx.req_id,
        ResultPayload::of_encoded_or_null::<
            medulla_protocol_types::result_contract::graph::FinanceRecordGet,
        >(props),
    ))
}
