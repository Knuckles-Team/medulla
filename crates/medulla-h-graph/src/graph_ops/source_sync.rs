//! Served source sync (MEDULLA-SRC-AUTH-R012.6, R012.7, R012.8).
//!
//! `SourceSync` is gateway-routed and admin-scoped (`source:sync`). The host
//! owns the connector call: it opened, filled and completed one foreign read for
//! the connection and class (`plan_digest = sha256(connection 0x00 class)`), and
//! the engine never calls out. The handler reads the authority rows of the
//! composed schema sources, refuses a sync the rows or the adapter table do not
//! allow before it touches the read, takes the completed rows exactly once,
//! plans the whole sync as a pure function (`plan_sync`) over the caller's
//! visible graph and commits the inbound changes and the sync state as ONE
//! `BatchUpdate` through the staged gateway. Outbound changes are never written
//! here: they come back in the report as drafts, and the router queues each as
//! a write-back proposal (`queue_outbound`, R012.5).

use super::*;

use medulla_server_api::carrier::CarrierAuthority;
use medulla_types::contract::ServerErrorCode;

/// The `vg:`-less IRI base of a core class label.
#[cfg(all(feature = "rdf", feature = "federation"))]
const CORE_CLASS_BASE: &str = "https://knuckles-team.github.io/ontology/core#";

#[cfg(all(feature = "rdf", feature = "federation"))]
fn refusal(code: ServerErrorCode, detail: impl std::fmt::Display) -> String {
    format!("{}: {detail}", code.as_str())
}

/// The route entry from the mutation gateway: `visible` is the caller's
/// projected graph (the sync plans against what the caller may see); the batch
/// commits on the authoritative core of `ctx`.
#[cfg(all(feature = "rdf", feature = "federation"))]
pub(super) async fn handle_source_sync(
    state: &Arc<RwLock<ServerState>>,
    ctx: &MutationCtx<'_>,
    visible: &Arc<GraphCore>,
    carrier: Option<&CarrierAuthority>,
    method: &Method,
) -> Response {
    let Method::SourceSync {
        connection_iri,
        class_iri,
        foreign_execution,
        scope,
    } = method
    else {
        return Response::err(ctx.req_id, "INVALID_ARGUMENT: not a SourceSync");
    };
    let Some(carrier) = carrier else {
        return Response::err(
            ctx.req_id,
            "ACCESS_DENIED: SourceSync requires a verified tenant carrier",
        );
    };
    match plan_and_batch(
        state,
        ctx,
        visible,
        carrier,
        connection_iri,
        class_iri,
        foreign_execution,
        scope.as_deref(),
    )
    .await
    {
        Ok((write, operations, report)) => {
            if operations.is_empty() {
                return match ResultPayload::of::<
                    medulla_protocol_types::result_contract::graph::SourceSync,
                >(report)
                {
                    Ok(payload) => Response::ok(ctx.req_id, payload),
                    Err(error) => Response::err(ctx.req_id, error),
                };
            }
            let plan = MutationPlan::for_method(&write);
            medulla_server_ops::api::ingest::graph::commit_gateway(
                ctx,
                &plan,
                &write,
                move |core| {
                    medulla_graph_algos::algorithms::batch_update(core, &operations).and_then(
                        |_| {
                            ResultPayload::of::<
                                medulla_protocol_types::result_contract::graph::SourceSync,
                            >(report)
                        },
                    )
                },
            )
            .await
        }
        Err(error) => Response::err(ctx.req_id, error),
    }
}

#[cfg(not(all(feature = "rdf", feature = "federation")))]
pub(super) async fn handle_source_sync(
    _state: &Arc<RwLock<ServerState>>,
    ctx: &MutationCtx<'_>,
    _visible: &Arc<GraphCore>,
    _carrier: Option<&CarrierAuthority>,
    _method: &Method,
) -> Response {
    Response::err(
        ctx.req_id,
        "METHOD_NOT_YET_SERVED: SourceSync needs the rdf and federation features",
    )
}

/// Authority checks, the one-shot row take and the pure plan. Returns the
/// `BatchUpdate` method, its encoded operations and the report; every refusal
/// happens before the rows are taken, so a refused sync leaves the read open.
#[cfg(all(feature = "rdf", feature = "federation"))]
#[allow(clippy::too_many_arguments)]
async fn plan_and_batch(
    state: &Arc<RwLock<ServerState>>,
    ctx: &MutationCtx<'_>,
    visible: &Arc<GraphCore>,
    carrier: &CarrierAuthority,
    connection_iri: &str,
    class_iri: &str,
    execution: &medulla_catalog_types::foreign_read::ForeignReadExecutionRef,
    scope: Option<&str>,
) -> Result<
    (
        Method,
        Vec<u8>,
        medulla_source_types::source_sync::SyncReport,
    ),
    String,
> {
    use medulla_server_ops::api::query_txn::ForeignStateExt;
    use medulla_virtual_graph::source_selection::sync_authorities;
    use medulla_virtual_graph::source_sync::{
        plan_sync, sync_plan_digest, ConflictRule, GraphNodes, RecordRole, SyncAdapter,
    };

    let sources = ctx.core.schema_sources();
    let authorities = sync_authorities(&sources)?;
    let for_class: Vec<_> = authorities
        .iter()
        .filter(|row| row.class == class_iri)
        .collect();
    if !for_class
        .iter()
        .any(|row| row.role == RecordRole::SystemOfRecord)
    {
        return Err(refusal(
            ServerErrorCode::SyncNoSystemOfRecord,
            format!("no system_of_record row covers {class_iri}"),
        ));
    }
    let secondary = for_class
        .iter()
        .find(|row| row.role == RecordRole::FederatedSecondary && row.connection == connection_iri)
        .ok_or_else(|| {
            refusal(
                ServerErrorCode::SyncSourceNotSecondary,
                format!("{connection_iri} is not a federated secondary for {class_iri}"),
            )
        })?;
    let (Some(direction), Some(rule_literal)) = (secondary.direction, &secondary.conflict_rule)
    else {
        return Err(format!(
            "INVALID_ARGUMENT: authority row {} lacks vg:syncDirection or vg:conflictRule",
            secondary.row
        ));
    };
    let rule = ConflictRule::parse(rule_literal).ok_or_else(|| {
        format!(
            "INVALID_ARGUMENT: authority row {} has unknown vg:conflictRule",
            secondary.row
        )
    })?;
    let adapter = resolve_adapter(class_iri, scope)?.ok_or_else(|| {
        refusal(
            ServerErrorCode::SyncAdapterMissing,
            format!("no sync adapter serves {class_iri}"),
        )
    })?;

    let digest = sync_plan_digest(connection_iri, class_iri);
    let sessions = state.read().await.foreign_reads();
    let (_source_name, rows) = sessions.take_sync_rows(carrier, *execution, digest)?;
    let max_records = medulla_server_api::knob_config::knobs().sync_max_records as usize;
    if rows.len() > max_records {
        return Err(format!(
            "INVALID_ARGUMENT: the source read holds {} records, more than sync.max_records {max_records}",
            rows.len()
        ));
    }
    let records: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|row| {
            let mut record = serde_json::Map::new();
            record.insert("id".to_string(), serde_json::Value::String(row.id));
            record.extend(row.columns);
            serde_json::Value::Object(record)
        })
        .collect();

    let nodes: GraphNodes = visible
        .get_nodes()
        .into_iter()
        .filter_map(
            |(id, bytes)| match medulla_types::msgpack::decode_property_value(&bytes) {
                Ok(serde_json::Value::Object(map)) => Some((id, map)),
                _ => None,
            },
        )
        .collect();
    let now_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
    let adapter_ref: &dyn SyncAdapter = adapter.as_ref();
    let mut plan = plan_sync(
        adapter_ref,
        connection_iri,
        class_iri,
        rule,
        rule_literal,
        &records,
        &nodes,
        now_ms,
    )?;
    // A direction the authority row does not allow is refused whole: nothing
    // is written and nothing is proposed.
    if plan.report.applied > 0 && !direction.allows_inbound() {
        return Err(refusal(
            ServerErrorCode::SyncDirectionRefused,
            format!("{connection_iri} is outbound-only for {class_iri}, yet the source changed"),
        ));
    }
    if plan.report.proposed > 0 && !direction.allows_outbound() {
        return Err(refusal(
            ServerErrorCode::SyncDirectionRefused,
            format!("{connection_iri} is inbound-only for {class_iri}, yet the graph changed"),
        ));
    }
    let operations: Vec<serde_json::Value> = std::mem::take(&mut plan.batch)
        .into_iter()
        .map(|mutation| {
            serde_json::json!({
                "op": "upsert_node",
                "id": mutation.node_id,
                "properties": mutation.properties,
            })
        })
        .collect();
    let operations_msgpack = if operations.is_empty() {
        Vec::new()
    } else {
        rmp_serde::to_vec_named(&operations)
            .map_err(|error| format!("INVALID_ARGUMENT: sync encoding failed: {error}"))?
    };
    let write = Method::BatchUpdate {
        operations_msgpack: operations_msgpack.clone(),
    };
    Ok((write, operations_msgpack, plan.report))
}

/// One entry of the adapter table: the adapters of one family of classes.
#[cfg(all(feature = "rdf", feature = "federation"))]
type AdapterBuild =
    fn(
        class_iri: &str,
        scope: Option<&str>,
    ) -> Result<Option<Box<dyn medulla_virtual_graph::source_sync::SyncAdapter>>, String>;

/// The adapter table, a const slice (R012.6): MEDULLA-CQ-ORCH-R705.7 adds the
/// architecture entry; MEDULLA-CQ-ORCH-R703.3 and MEDULLA-TYPED-PACKS-R110.5
/// add theirs beside it. The first entry whose build answers `Some` serves the
/// class.
#[cfg(all(feature = "rdf", feature = "federation"))]
const ADAPTERS: &[AdapterBuild] = &[architecture_adapter];

#[cfg(all(feature = "rdf", feature = "federation"))]
fn resolve_adapter(
    class_iri: &str,
    scope: Option<&str>,
) -> Result<Option<Box<dyn medulla_virtual_graph::source_sync::SyncAdapter>>, String> {
    for build in ADAPTERS {
        if let Some(adapter) = build(class_iri, scope)? {
            return Ok(Some(adapter));
        }
    }
    Ok(None)
}

/// The architecture adapter serves every class that resolves an
/// `:archimateType` through the ontology; the scope is the model identifier.
#[cfg(all(feature = "rdf", feature = "federation"))]
fn architecture_adapter(
    class_iri: &str,
    scope: Option<&str>,
) -> Result<Option<Box<dyn medulla_virtual_graph::source_sync::SyncAdapter>>, String> {
    let Some(label) = class_iri.strip_prefix(CORE_CLASS_BASE) else {
        return Ok(None);
    };
    if medulla_rdf::archimate_types::archimate_type_map()
        .element_type_of(label)
        .is_none()
    {
        return Ok(None);
    }
    let model_id = scope.filter(|model| !model.is_empty()).ok_or_else(|| {
        "INVALID_ARGUMENT: an architecture sync names its model in scope".to_string()
    })?;
    Ok(Some(Box::new(
        medulla_virtual_graph::architecture_sync::ArchitectureSyncAdapter {
            model_id: model_id.to_string(),
        },
    )))
}
