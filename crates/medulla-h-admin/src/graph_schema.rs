//! `Method::GraphSchema` and `Method::GraphSchemaList` (X9): keyed shapes and
//! ontology sources on one request graph.
//!
//! The write path is GATEWAY-routed, exactly like `IcvConfigure`: the arm runs
//! inside the graph commit kernel, so an attach is audited, emits CDC and is
//! ordered against every other write to that graph. The read is a separate
//! method because a read op inside a gateway-routed method would need a
//! runtime-conditional gateway plan.
//!
//! The schema-sources package owns composition, validation and the memoised
//! composed digest. Writes reach the ordinary graph mutation gateway, so schema
//! changes share the graph's persistence, audit, CDC, idempotency and Raft
//! ordering rather than maintaining a second control-plane commit path.

mod approved;
pub(crate) mod attach_pack;
mod edits;
#[cfg(test)]
mod finance_tests;
mod ontology_import;
pub mod ontology_diff;
mod ontology_diff_serve;
#[cfg(test)]
mod ontology_diff_tests;
#[cfg(all(test, feature = "shacl"))]
mod swarm_topology_tests;
#[cfg(test)]
mod terms_served_tests;
#[cfg(test)]
mod test_support;
#[cfg(all(test, feature = "shacl"))]
mod virtual_graph_tests;
#[cfg(test)]
mod world_model_tests;

use medulla_server_cdc::cdc::CdcStateExt;
use std::sync::Arc;
use tokio::sync::RwLock;

use medulla_graph_core::graph::GraphCore;

use medulla_protocol_types::protocol::Method;
use medulla_protocol_types::protocol::Response;
use medulla_protocol_types::protocol::ResultPayload;
use medulla_server_api::state::ServerState;
use medulla_server_mutation::mutation::MutationCtx;
use medulla_server_mutation::mutation::MutationPlan;

/// One exact semantic member selected from a committed ConnectorPack record.
/// Keeping URI and body together preserves the per-file RDF blank-node scope;
/// callers must never concatenate Turtle documents before this boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PackSchemaDocument {
    pub(crate) uri: String,
    pub(crate) body: Arc<str>,
}

/// Everything one pack head projects into its pack graph: the schema documents
/// of the reserved source, and the connector surface triples
/// (MEDULLA-TYPED-PACKS-R102.2) as one N-Triples document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PackProjectionDocuments {
    pub(crate) shapes: Vec<PackSchemaDocument>,
    pub(crate) ontologies: Vec<PackSchemaDocument>,
    pub(crate) crosswalks: Vec<PackSchemaDocument>,
    pub(crate) surface: Option<Arc<str>>,
}

/// Commit the semantic projection of one ConnectorPack head.
///
/// This is the server-level companion to [`apply_pack_source`].  ConnectorPack
/// owns its head/receipt CAS; GraphSchema owns physical graph identity, durable
/// graph lifecycle, the mutation gateway, and the reserved schema source.  A
/// newer head with no semantic documents installs an empty Pack-origin
/// tombstone, preserving the record high-water mark so stale schema cannot be
/// resurrected.
pub(crate) async fn commit_pack_projection(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    verified: &medulla_server_api::authority_context::VerifiedRequestContext,
    connector: &medulla_types::contract::ResourceId,
    record_id: &str,
    documents: PackProjectionDocuments,
    idempotency_key: &str,
) -> Result<medulla_catalog_types::graph_schema::GraphSchemaCommitted, String> {
    let graph_name = medulla_catalog_types::connector_pack::ids::pack_projection_graph_name(
        verified.tenant(),
        connector,
    )?;
    #[cfg(feature = "raft")]
    if state.read().await.multi_raft.is_some() {
        return Err(
            "CONNECTOR_PACK_PROJECTION_CLUSTER_UNAVAILABLE: ConnectorPack control state is not replicated"
                .to_string(),
        );
    }
    medulla_server_api::api::reentry::ensure_internal_global_graph(
        state,
        req_id,
        verified,
        &graph_name,
        idempotency_key,
    )
    .await?;
    let authority = medulla_server_api::carrier::CarrierAuthority::from_verified(verified)?;
    let (entry, isolation, persistence, materialization_manifest, write_coalescer) = {
        let state = state.read().await;
        let entry = state
            .registry
            .get(&graph_name)
            .cloned()
            .ok_or_else(|| format!("pack projection graph '{graph_name}' is not resident"))?;
        let materialization_manifest = state.registry.materialization_handle(&graph_name);
        (
            entry,
            state.isolation.clone(),
            state.persistence.clone(),
            materialization_manifest,
            state.routed_write_coalescer.clone(),
        )
    };
    if materialization_manifest
        .as_ref()
        .is_some_and(|manifest| manifest.read().map_or(true, |manifest| !manifest.valid))
    {
        return Err(
            "PACK_PROJECTION_GRAPH_NOT_MATERIALIZED: projection graph is incomplete".into(),
        );
    }
    prevalidate_pack_projection(&entry.core, connector, record_id, &documents).await?;
    #[cfg(feature = "streaming")]
    let cdc = state.read().await.cdc_hub().cloned();
    let method = Method::GraphSchema {
        op: Box::new(
            medulla_catalog_types::graph_schema::GraphSchemaOp::AttachPack {
                connector: connector.clone(),
                if_composed_digest: None,
            },
        ),
    };
    let plan = MutationPlan::for_method(&method);
    let result_graph_name = graph_name.clone();
    let ctx = MutationCtx {
        anonymous: state.read().await.local_anonymous,
        req_id,
        caller: Some(verified.agent_id()),
        attempt_nonce: verified.attempt_nonce(),
        idempotency_key,
        tenant_scope: authority.tenant_scope(),
        graph_name: &graph_name,
        graph_type: entry.graph_type,
        owner: entry.owner.as_deref(),
        isolation: &isolation,
        core: &entry.core,
        persistence: persistence.as_ref(),
        #[cfg(feature = "streaming")]
        cdc: cdc.as_ref(),
        materialization_manifest: materialization_manifest.as_ref(),
        write_coalescer: Some(&write_coalescer),
    };
    let connector = connector.clone();
    let record_id = record_id.to_string();
    let response =
        medulla_server_ops::api::ingest::graph::commit_gateway(&ctx, &plan, &method, move |core| {
            let committed = apply_pack_source(
                core,
                &result_graph_name,
                &connector,
                &record_id,
                PackSourceDocuments {
                    shapes: documents.shapes,
                    ontologies: documents.ontologies,
                    crosswalks: documents.crosswalks,
                },
                None,
            )?;
            write_pack_surface(core, &result_graph_name, documents.surface.as_deref())?;
            ResultPayload::of::<medulla_protocol_types::result_contract::reasoning::GraphSchema>(
                committed,
            )
        })
        .await;
    if let Some(refusal) = response.refusal_text() {
        return Err(refusal);
    }
    match response.result {
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(&bytes)
            .map_err(|error| format!("GraphSchema projection result decode failed: {error}")),
        _ => Err("GraphSchema projection returned an undeclared result encoding".to_string()),
    }
}

/// Write one pack head's surface triples into its graph, inside the gateway.
#[cfg(all(feature = "redb", feature = "blob", feature = "shacl"))]
fn write_pack_surface(core: &GraphCore, graph: &str, surface: Option<&str>) -> Result<(), String> {
    crate::admin::connector_pack::reproject::surface::write_surface(core, graph, surface)
}

/// A build without the pack projection substrates has no surface to write.
#[cfg(not(all(feature = "redb", feature = "blob", feature = "shacl")))]
fn write_pack_surface(
    _core: &GraphCore,
    _graph: &str,
    _surface: Option<&str>,
) -> Result<(), String> {
    Ok(())
}

/// Attach, replace or detach one keyed schema source, inside the gateway.
///
/// The op match is exhaustive and has no catch-all: `AttachPack` resolves a
/// connector's pack head and belongs to a different package from the operator
/// attach, so it must stay separately routed.
pub(crate) async fn handle_gateway(
    state: &Arc<RwLock<ServerState>>,
    tenant_id: &str,
    ctx: &MutationCtx<'_>,
    plan: &MutationPlan,
    method: &Method,
    op: &medulla_catalog_types::graph_schema::GraphSchemaOp,
) -> Response {
    use medulla_catalog_types::graph_schema::GraphSchemaOp;

    match op {
        GraphSchemaOp::Attach { .. } | GraphSchemaOp::Detach { .. } => {
            let op = op.clone();
            let graph_name = ctx.graph_name.to_string();
            medulla_server_ops::api::ingest::graph::commit_gateway(ctx, plan, method, move |core| {
                apply(core, &graph_name, &op)
            })
            .await
        }
        GraphSchemaOp::AttachPack { connector, .. } => {
            handle_attach_pack_gateway(
                state,
                tenant_id,
                ctx,
                plan,
                method,
                connector,
                op.expected_composed_digest(),
            )
            .await
        }
        GraphSchemaOp::AttachApproved { .. } => {
            approved::handle_gateway(tenant_id, ctx, plan, method, op).await
        }
        GraphSchemaOp::ValidateRepair { .. } => {
            approved::handle_validate(ctx, plan, method, op).await
        }
        GraphSchemaOp::DiffOntology { .. } => {
            handle_diff_gateway(tenant_id, ctx, plan, method, op).await
        }
        GraphSchemaOp::ImportOntology { .. }
        | GraphSchemaOp::WithdrawOntology { .. }
        | GraphSchemaOp::RevertEdit { .. } => {
            handle_ontology_gateway(tenant_id, ctx, plan, method, op).await
        }
    }
}

/// R804.3: serve `DiffOntology`. It reads the staged graph and returns the
/// diff and impact; it installs and writes nothing.
async fn handle_diff_gateway(
    tenant_id: &str,
    ctx: &MutationCtx<'_>,
    plan: &MutationPlan,
    method: &Method,
    op: &medulla_catalog_types::graph_schema::GraphSchemaOp,
) -> Response {
    let op = op.clone();
    let tenant = tenant_id.to_string();
    let graph_name = ctx.graph_name.to_string();
    medulla_server_ops::api::ingest::graph::commit_gateway(ctx, plan, method, move |core| {
        ontology_diff_serve::diff_ontology(core, &graph_name, &tenant, &op).and_then(
            ResultPayload::of::<medulla_protocol_types::result_contract::reasoning::GraphSchema>,
        )
    })
    .await
}

/// R103.3: route `ImportOntology` / `WithdrawOntology` through the gateway with
/// the authenticated request principal as the importer.
async fn handle_ontology_gateway(
    tenant_id: &str,
    ctx: &MutationCtx<'_>,
    plan: &MutationPlan,
    method: &Method,
    op: &medulla_catalog_types::graph_schema::GraphSchemaOp,
) -> Response {
    use medulla_catalog_types::graph_schema::GraphSchemaOp;

    let authority = ontology_import::OntologyAuthority {
        importer: ctx.caller.unwrap_or_default().to_string(),
        tenant: tenant_id.to_string(),
        at_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| {
                u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
            }),
    };
    let op = op.clone();
    let graph_name = ctx.graph_name.to_string();
    medulla_server_ops::api::ingest::graph::commit_gateway(ctx, plan, method, move |core| {
        let committed = match &op {
            GraphSchemaOp::WithdrawOntology { .. } => {
                ontology_import::withdraw_ontology(core, &graph_name, &op, &authority)
            }
            GraphSchemaOp::RevertEdit { subject_iri } => {
                ontology_import::revert_edit(core, &graph_name, subject_iri)
            }
            _ => ontology_import::import_ontology(core, &graph_name, &op, &authority),
        };
        committed.and_then(
            ResultPayload::of::<medulla_protocol_types::result_contract::reasoning::GraphSchema>,
        )
    })
    .await
}

async fn handle_attach_pack_gateway(
    state: &Arc<RwLock<ServerState>>,
    tenant_id: &str,
    ctx: &MutationCtx<'_>,
    plan: &MutationPlan,
    method: &Method,
    connector: &medulla_types::contract::ResourceId,
    expected_digest: Option<&str>,
) -> Response {
    #[cfg(feature = "raft")]
    if state.read().await.multi_raft.is_some() {
        return Response::err(
            ctx.req_id,
            "ATTACH_PACK_CLUSTER_UNAVAILABLE: ConnectorPack control state is not replicated",
        );
    }
    let resolved = match attach_pack::resolve(state, tenant_id, connector).await {
        Ok(source) => source,
        Err(error) => return Response::err(ctx.req_id, error),
    };
    let connector = connector.clone();
    let expected_digest = expected_digest.map(str::to_owned);
    let graph_name = ctx.graph_name.to_string();
    medulla_server_ops::api::ingest::graph::commit_gateway(ctx, plan, method, move |core| {
        apply_pack_source(
            core,
            &graph_name,
            &connector,
            &resolved.record_id,
            PackSourceDocuments {
                shapes: resolved.shapes,
                ontologies: resolved.ontologies,
                crosswalks: resolved.crosswalks,
            },
            expected_digest.as_deref(),
        )
        .and_then(
            ResultPayload::of::<medulla_protocol_types::result_contract::reasoning::GraphSchema>,
        )
    })
    .await
}

/// The engine-owned documents one pack record attaches.
pub(crate) struct PackSourceDocuments {
    pub(crate) shapes: Vec<PackSchemaDocument>,
    pub(crate) ontologies: Vec<PackSchemaDocument>,
    pub(crate) crosswalks: Vec<PackSchemaDocument>,
}

/// Install one ConnectorPack semantic projection under its reserved source.
///
/// This is the sole internal seam shared by `GraphSchema.AttachPack` and the
/// ConnectorPack reproject worker.  Callers must invoke it inside the graph
/// mutation gateway on the gateway's staged [`GraphCore`].  The seam accepts
/// only connector identity, immutable record identity, and engine-owned bytes;
/// it constructs the reserved key and provenance itself.  Consequently neither
/// a wire caller nor a pack worker can forge a core/admin/ingestion origin.
pub(crate) fn apply_pack_source(
    core: &GraphCore,
    graph_name: &str,
    connector: &medulla_types::contract::ResourceId,
    record_id: &str,
    documents: PackSourceDocuments,
    if_composed_digest: Option<&str>,
) -> Result<medulla_catalog_types::graph_schema::GraphSchemaCommitted, String> {
    use medulla_catalog_types::graph_schema::GraphSchemaCommitted;
    let (sources, changed, records) = pack_candidate_sources(
        &core.schema_sources(),
        connector,
        record_id,
        documents,
        if_composed_digest,
    )?;
    medulla_server_api::schema_compose::validate_entering_schema(&sources)?;
    revalidate_pack_crosswalks(&sources, &records)?;
    if changed {
        core.install_schema_sources(Arc::new(sources.clone()));
    }
    Ok(GraphSchemaCommitted {
        schema_version: medulla_catalog_types::graph_schema::GRAPH_SCHEMA_RESULT_SCHEMA_VERSION,
        graph: graph_name.to_string(),
        composed_digest: sources.composed_digest().to_hex(),
        graph_version: core.version().saturating_add(u64::from(changed)),
        changed,
        edit_head: edits::head(core),
        ontology_diff: None,
    })
}

/// Decide the entering-schema check for a pack projection on the BLOCKING pool,
/// before the commit kernel runs (BUG-009).
///
/// Composition (the terminology tableau) and the full-ABox tableau are CPU work
/// that grows with the pack's ontology. Run inline inside the commit closure they
/// held a tokio worker for minutes on a large pack, and every request queued
/// behind that worker or the state lock stalled. The verdict lands on the
/// content-keyed compiled entry (and its ABox memo), so the authoritative check
/// inside [`apply_pack_source`] is then a cache hit. A refusal here is the
/// refusal the commit would give for the same bytes; if the graph's sources
/// change in between, the in-commit check recomputes, so correctness never
/// depends on this warm-up.
async fn prevalidate_pack_projection(
    core: &Arc<GraphCore>,
    connector: &medulla_types::contract::ResourceId,
    record_id: &str,
    documents: &PackProjectionDocuments,
) -> Result<(), String> {
    let (candidate, _, _) = pack_candidate_sources(
        &core.schema_sources(),
        connector,
        record_id,
        PackSourceDocuments {
            shapes: documents.shapes.clone(),
            ontologies: documents.ontologies.clone(),
            crosswalks: documents.crosswalks.clone(),
        },
        None,
    )?;
    medulla_server_api::api::dispatch_support::blocking_task(
        "pack projection schema validation",
        move || medulla_server_api::schema_compose::validate_entering_schema(&candidate).map(drop),
    )
    .await
}

/// The graph's schema sources with `connector`'s pack source attached, whether
/// the attach changed them, and the pack's crosswalk records. Pure: shared by
/// [`apply_pack_source`] and the off-runtime pre-validation in
/// [`commit_pack_projection`], so both validate byte-identical sources under one
/// `composed_digest`.
fn pack_candidate_sources(
    current: &medulla_graph_core::graph::GraphSchemaSources,
    connector: &medulla_types::contract::ResourceId,
    record_id: &str,
    documents: PackSourceDocuments,
    if_composed_digest: Option<&str>,
) -> Result<
    (
        medulla_graph_core::graph::GraphSchemaSources,
        bool,
        Vec<medulla_catalog_types::domain_pack::VocabularyCrosswalk>,
    ),
    String,
> {
    let PackSourceDocuments {
        shapes,
        ontologies,
        crosswalks,
    } = documents;

    if record_id.is_empty() {
        return Err("ATTACH_PACK_SCHEMA_INVALID: pack record id is empty".to_string());
    }
    if let Some(expected) = if_composed_digest {
        medulla_types::contract::Digest256::parse(expected)
            .map_err(|error| format!("invalid if_composed_digest: {error}"))?;
    }
    let mut sources = current.clone();
    if let Some(expected) = if_composed_digest {
        let actual = sources.composed_digest().to_hex();
        if expected != actual {
            return Err(format!(
                "COMPOSED_DIGEST_MISMATCH: expected {expected}, actual {actual}"
            ));
        }
    }
    let shapes_ttl = compose_pack_documents("SHAPES", shapes)?;
    let ontology_ttl = compose_pack_documents("ONTOLOGY", ontologies)?;
    let records = parse_pack_crosswalks(&crosswalks)?;
    let source = medulla_graph_core::graph::GraphSchemaSource::new(
        medulla_graph_core::graph::SchemaSourceOrigin::Pack {
            connector: connector.as_str().to_string(),
            record_id: record_id.to_string(),
        },
        shapes_ttl,
        ontology_ttl,
        0,
    )?
    .with_crosswalks(records.clone());
    let source_id = format!("pack:{}", connector.as_str());
    let changed = sources.attach_dynamic(source_id, source)?;
    Ok((sources, changed, records))
}

/// The crosswalk records of a pack head, in canonical order. A body that is not
/// a crosswalk record refuses activation (MEDULLA-TYPED-PACKS-R071.1.3).
fn parse_pack_crosswalks(
    documents: &[PackSchemaDocument],
) -> Result<Vec<medulla_catalog_types::domain_pack::VocabularyCrosswalk>, String> {
    let mut records = Vec::with_capacity(documents.len());
    for document in documents {
        let record =
            crate::admin::connector_pack::import::crosswalk::parse_record(document.body.as_bytes())
                .map_err(|error| {
                    format!(
                        "{}: crosswalk member '{}': {error}",
                        medulla_types::contract::ServerErrorCode::MalformedBody.as_str(),
                        document.uri
                    )
                })?;
        records.push(record);
    }
    records.sort_by(|left, right| {
        (
            &left.source_class_iri,
            &left.target_class_iri,
            &left.content_digest,
        )
            .cmp(&(
                &right.source_class_iri,
                &right.target_class_iri,
                &right.content_digest,
            ))
    });
    Ok(records)
}

/// Re-validate the head's crosswalks against the composed schema before they
/// become active, with the same rules the import applied.
fn revalidate_pack_crosswalks(
    sources: &medulla_graph_core::graph::GraphSchemaSources,
    records: &[medulla_catalog_types::domain_pack::VocabularyCrosswalk],
) -> Result<(), String> {
    use crate::admin::connector_pack::import::{crosswalk, vocabulary};

    if records.is_empty() {
        return Ok(());
    }
    let pinned = vocabulary::pin_composed(
        &medulla_graph_core::graph::GraphSchemaSources::default(),
        sources,
    )?;
    crosswalk::validate_set(records, &pinned).map_err(|refusal| refusal.activation_error())
}

/// Parse each pack member independently, scope its blank nodes by immutable
/// member URI, then render a deterministic N-Triples/Turtle union. Sorting the
/// URI first makes pack archive order irrelevant; parsing before union is what
/// prevents two local `_:b0` labels from becoming one RDF node.
pub(crate) fn compose_pack_documents(
    kind: &str,
    mut documents: Vec<PackSchemaDocument>,
) -> Result<Option<Arc<str>>, String> {
    if documents.is_empty() {
        return Ok(None);
    }
    documents.sort_by(|left, right| left.uri.as_bytes().cmp(right.uri.as_bytes()));
    let mut previous_uri: Option<&str> = None;
    let mut rendered = std::collections::BTreeSet::new();
    for document in &documents {
        if document.uri.is_empty() || document.uri.chars().any(char::is_control) {
            return Err(format!(
                "ATTACH_PACK_SCHEMA_INVALID: {kind} member URI is invalid"
            ));
        }
        if previous_uri == Some(document.uri.as_str()) {
            return Err(format!(
                "ATTACH_PACK_SCHEMA_AMBIGUOUS: duplicate {kind} member URI '{}'",
                document.uri
            ));
        }
        previous_uri = Some(document.uri.as_str());
        if document.body.len() > medulla_catalog_types::graph_schema::MAX_SCHEMA_DOCUMENT_BYTES {
            return Err(format!(
                "ATTACH_PACK_SCHEMA_TOO_LARGE: {kind} member '{}' exceeds {} bytes",
                document.uri,
                medulla_catalog_types::graph_schema::MAX_SCHEMA_DOCUMENT_BYTES
            ));
        }
        let scope = medulla_types::contract::Digest256::sha256(document.uri.as_bytes()).to_hex();
        let triples = medulla_rdf::mapping::parse_turtle_for_composition(&document.body).map_err(
            |error| {
                format!(
                    "ATTACH_PACK_SCHEMA_INVALID: {kind} member '{}': {error}",
                    document.uri
                )
            },
        )?;
        for triple in triples {
            let triple = medulla_server_api::schema_compose::scope_blank_nodes(triple, &scope)
                .map_err(|error| {
                    format!(
                        "ATTACH_PACK_SCHEMA_INVALID: {kind} member '{}': {error}",
                        document.uri
                    )
                })?;
            // `Triple`'s Display is the N-Triples statement WITHOUT its terminating
            // ` .`; the projection is re-parsed as Turtle, so every line needs it.
            rendered.insert(format!("{triple} ."));
        }
    }
    let mut body = rendered.into_iter().collect::<Vec<_>>().join("\n");
    body.push('\n');
    if body.len() > medulla_catalog_types::graph_schema::MAX_SCHEMA_DOCUMENT_BYTES {
        return Err(format!(
            "ATTACH_PACK_SCHEMA_TOO_LARGE: composed {kind} projection exceeds {} bytes",
            medulla_catalog_types::graph_schema::MAX_SCHEMA_DOCUMENT_BYTES
        ));
    }
    Ok(Some(Arc::from(body)))
}

fn apply(
    core: &GraphCore,
    graph_name: &str,
    op: &medulla_catalog_types::graph_schema::GraphSchemaOp,
) -> Result<ResultPayload, String> {
    use medulla_catalog_types::{graph_schema::GraphSchemaCommitted, graph_schema::GraphSchemaOp};

    op.validate()?;
    let before = edits::source_digest(core, op.source_token());
    let mut sources = (*core.schema_sources()).clone();
    check_expected_digest(&sources, op.expected_composed_digest())?;
    let changed = match op {
        GraphSchemaOp::Attach { source_id, .. } => {
            sources.attach_dynamic(source_id.clone(), admin_source(op)?)?
        }
        GraphSchemaOp::AttachApproved { source_id, .. } => {
            let source = approved::admitted_source(core, &sources, op)?;
            sources.attach_dynamic(source_id.clone(), source)?
        }
        GraphSchemaOp::Detach { source_id, .. } => sources.detach_dynamic(source_id),
        GraphSchemaOp::AttachPack { .. }
        | GraphSchemaOp::ValidateRepair { .. }
        | GraphSchemaOp::ImportOntology { .. }
        | GraphSchemaOp::WithdrawOntology { .. }
        | GraphSchemaOp::DiffOntology { .. }
        | GraphSchemaOp::RevertEdit { .. } => {
            unreachable!(
                "attach-pack, validate-repair and runtime ontologies have their own routes"
            )
        }
    };
    if matches!(op, GraphSchemaOp::Detach { .. }) {
        medulla_server_api::schema_compose::validate_and_compose(&sources)?;
    } else {
        medulla_server_api::schema_compose::validate_entering_schema(&sources)?;
    }
    let composed_digest = sources.composed_digest().to_hex();
    if changed {
        core.install_schema_sources(Arc::new(sources));
    }
    let edit_head = edits::after_attach(core, op, before, changed)?;
    ResultPayload::of::<medulla_protocol_types::result_contract::reasoning::GraphSchema>(
        GraphSchemaCommitted {
            schema_version: medulla_catalog_types::graph_schema::GRAPH_SCHEMA_RESULT_SCHEMA_VERSION,
            graph: graph_name.to_string(),
            composed_digest,
            graph_version: core.version().saturating_add(u64::from(changed)),
            changed,
            edit_head,
            ontology_diff: None,
        },
    )
}

/// The compare-and-set fence: the composed digest the caller last saw.
fn check_expected_digest(
    sources: &medulla_graph_core::graph::GraphSchemaSources,
    expected: Option<&str>,
) -> Result<(), String> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let actual = sources.composed_digest().to_hex();
    if expected == actual {
        Ok(())
    } else {
        Err(format!(
            "COMPOSED_DIGEST_MISMATCH: expected {expected}, actual {actual}"
        ))
    }
}

/// The admin-origin source a validated generic `Attach` installs.
fn admin_source(
    op: &medulla_catalog_types::graph_schema::GraphSchemaOp,
) -> Result<medulla_graph_core::graph::GraphSchemaSource, String> {
    let medulla_catalog_types::graph_schema::GraphSchemaOp::Attach {
        source_id,
        shapes_ttl,
        ontology_ttl,
        ..
    } = op
    else {
        return Err("admin schema source needs a generic attach".to_string());
    };
    medulla_graph_core::graph::GraphSchemaSource::new(
        medulla_graph_core::graph::SchemaSourceOrigin::Admin {
            name: source_id
                .strip_prefix("admin:")
                .expect("validated generic schema ids use the admin namespace")
                .to_string(),
        },
        shapes_ttl.as_deref().map(Arc::from),
        ontology_ttl.as_deref().map(Arc::from),
        0,
    )
}

#[cfg(test)]
mod tests;
