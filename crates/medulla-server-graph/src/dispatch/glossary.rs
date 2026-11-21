//! `Method::Glossary`: the served business glossary (MEDULLA-TYPED-PACKS-R801.3).
//!
//! Terms are `cat:CatalogGlossaryTerm` graph nodes in SKOS concept schemes. The
//! handler reads the verified tenant and principal from the request carrier
//! (never from the body), decides every refusal before its first write, and
//! commits through the ordinary graph gateway (`dispatch_graph_op`). Four ops:
//!
//! * `PutScheme` validates the batch (`medulla_core::glossary::PutScheme`) and
//!   upserts the scheme node and every term node as one batch.
//! * `PutTerm` upserts one term. A put never moves a status: a new term starts in
//!   `draft` and a stored term keeps its stored status.
//! * `Transition` applies one `GlossaryTermMachine` event to the stored term as a
//!   compare-and-set, recording the verified principal and the change request.
//! * `Bind` links a term to an asset through `:definesAsset`
//!   (`medulla_core::glossary::bind`, which refuses a deprecated term).
//!
//! Reads are UQL, SPARQL and search over the same nodes; no read op exists.

use std::sync::Arc;

use medulla_catalog_types::glossary::{
    GlossaryOp, GlossaryReceipt, GlossaryTerm, GlossaryTermEntry, GlossaryTermStatus,
    CAT_GLOSSARY_TERM, SKOS_IN_SCHEME,
};
use medulla_core::glossary::{bind, PutScheme};
use medulla_protocol_types::protocol::{Method, Response};
use medulla_protocol_types::result_contract::ingestion::{
    GlossaryBind, GlossaryPutScheme, GlossaryPutTerm, GlossaryTransition,
};
use medulla_server_api::state::ServerState;
use tokio::sync::RwLock;

use super::graph_client::{
    encode, read_node, took_effect, write, write_batch, GraphTarget, Properties, TENANT_PROPERTY,
};
use super::shared::typed_response;

/// The class of a concept scheme node.
const SCHEME_CLASS: &str = "http://www.w3.org/2004/02/skos/core#ConceptScheme";
/// The property holding the serialized term.
const RECORD_PROPERTY: &str = "record";

/// Serve one `Glossary` op.
pub async fn dispatch_glossary(
    state: &Arc<RwLock<ServerState>>,
    target: GraphTarget<'_>,
    op: GlossaryOp,
) -> Response {
    let req_id = target.req_id;
    let result = serve(state, &target, &op).await;
    match op {
        GlossaryOp::PutScheme { .. } => typed_response::<GlossaryPutScheme>(req_id, result),
        GlossaryOp::PutTerm { .. } => typed_response::<GlossaryPutTerm>(req_id, result),
        GlossaryOp::Transition { .. } => typed_response::<GlossaryTransition>(req_id, result),
        GlossaryOp::Bind { .. } => typed_response::<GlossaryBind>(req_id, result),
    }
}

fn invalid(detail: impl std::fmt::Display) -> String {
    format!("INVALID_ARGUMENT: {detail}")
}

async fn serve(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    op: &GlossaryOp,
) -> Result<GlossaryReceipt, String> {
    let tenant = target.verified.tenant();
    if tenant.trim().is_empty() {
        return Err(medulla_types::contract::ServerErrorCode::TenantScopeRequired
            .as_str()
            .to_string());
    }
    match op {
        GlossaryOp::PutScheme { scheme, terms } => put_scheme(state, target, scheme, terms).await,
        GlossaryOp::PutTerm { iri, term } => put_term(state, target, iri, term).await,
        GlossaryOp::Transition {
            iri,
            event,
            change_request,
        } => transition(state, target, iri, *event, change_request.as_deref()).await,
        GlossaryOp::Bind { term, asset } => bind_term(state, target, term, asset).await,
    }
}

/// The stored term of `iri` in the caller's tenant, with its node properties.
async fn stored_term(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    iri: &str,
) -> Result<Option<(GlossaryTerm, Properties)>, String> {
    let Some(properties) = read_node(state, target, iri).await? else {
        return Ok(None);
    };
    if properties.get(TENANT_PROPERTY).and_then(serde_json::Value::as_str)
        != Some(target.verified.tenant())
    {
        return Err(invalid(format!("term '{iri}' belongs to another tenant")));
    }
    let Some(body) = properties
        .get(RECORD_PROPERTY)
        .and_then(serde_json::Value::as_str)
    else {
        return Err(invalid(format!("node '{iri}' is not a glossary term")));
    };
    let term: GlossaryTerm = serde_json::from_str(body)
        .map_err(|error| invalid(format!("the stored term '{iri}' is malformed: {error}")))?;
    Ok(Some((term, properties)))
}

fn term_properties(
    tenant: &str,
    iri: &str,
    term: &GlossaryTerm,
) -> Result<Properties, String> {
    use serde_json::json;
    let record = serde_json::to_string(term)
        .map_err(|error| invalid(format!("term encoding failed: {error}")))?;
    let mut properties = Properties::new();
    properties.insert("type".into(), CAT_GLOSSARY_TERM.into());
    properties.insert("iri".into(), iri.into());
    properties.insert(SKOS_IN_SCHEME.into(), term.scheme.clone().into());
    properties.insert("scheme".into(), term.scheme.clone().into());
    properties.insert("pref_label".into(), term.pref_label.clone().into());
    properties.insert("alt_labels".into(), json!(term.alt_labels));
    properties.insert("definition".into(), term.definition.clone().into());
    properties.insert("broader".into(), json!(term.broader));
    properties.insert("related".into(), json!(term.related));
    properties.insert("status".into(), status_name(term.status).into());
    if let Some(replacement) = &term.replaced_by {
        properties.insert("replaced_by".into(), replacement.clone().into());
    }
    properties.insert(RECORD_PROPERTY.into(), record.into());
    properties.insert(TENANT_PROPERTY.into(), tenant.into());
    Ok(properties)
}

fn status_name(status: GlossaryTermStatus) -> &'static str {
    use medulla_types::transition_refused::MachineState;
    status.stored()
}

/// A term as it is first stored: it must start in `draft`; a stored term keeps its
/// stored status, so a put never skips the review lifecycle.
async fn admitted_term(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    iri: &str,
    term: &GlossaryTerm,
) -> Result<GlossaryTerm, String> {
    let mut term = term.clone();
    match stored_term(state, target, iri).await? {
        Some((stored, _)) => {
            term.status = stored.status;
            term.replaced_by = stored.replaced_by;
        }
        None if term.status != GlossaryTermStatus::Draft => {
            return Err(invalid(format!(
                "term '{iri}' must start in draft; only a transition moves its status"
            )));
        }
        None => {}
    }
    Ok(term)
}

async fn put_scheme(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    scheme: &str,
    entries: &[GlossaryTermEntry],
) -> Result<GlossaryReceipt, String> {
    let batch = PutScheme {
        scheme: scheme.to_string(),
        terms: entries
            .iter()
            .map(|entry| (entry.iri.clone(), entry.term.clone()))
            .collect(),
    };
    batch.validate().map_err(invalid)?;
    let tenant = target.verified.tenant();
    let mut scheme_properties = Properties::new();
    scheme_properties.insert("type".into(), SCHEME_CLASS.into());
    scheme_properties.insert("iri".into(), scheme.into());
    scheme_properties.insert(TENANT_PROPERTY.into(), tenant.into());
    let mut operations = vec![serde_json::json!({
        "op": "upsert_node", "id": scheme, "properties": scheme_properties,
    })];
    for entry in entries {
        let term = admitted_term(state, target, &entry.iri, &entry.term).await?;
        let properties = term_properties(tenant, &entry.iri, &term)?;
        operations.push(serde_json::json!({
            "op": "upsert_node", "id": entry.iri, "properties": properties,
        }));
    }
    let written = operations.len() as u64;
    write_batch(state, target, &operations).await?;
    Ok(GlossaryReceipt {
        iri: scheme.to_string(),
        status: None,
        written,
    })
}

async fn put_term(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    iri: &str,
    term: &GlossaryTerm,
) -> Result<GlossaryReceipt, String> {
    let single = PutScheme {
        scheme: term.scheme.clone(),
        terms: vec![(iri.to_string(), term.clone())],
    };
    single.validate().map_err(invalid)?;
    let term = admitted_term(state, target, iri, term).await?;
    let properties = term_properties(target.verified.tenant(), iri, &term)?;
    write(
        state,
        target,
        Method::AddNode {
            node_id: iri.to_string(),
            properties_msgpack: encode(&properties)?,
        },
    )
    .await?;
    Ok(GlossaryReceipt {
        iri: iri.to_string(),
        status: Some(term.status),
        written: 1,
    })
}

async fn transition(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    iri: &str,
    event: medulla_catalog_types::glossary::GlossaryTransition,
    change_request: Option<&str>,
) -> Result<GlossaryReceipt, String> {
    let (mut term, _) = stored_term(state, target, iri)
        .await?
        .ok_or_else(|| invalid(format!("term '{iri}' does not exist")))?;
    let from = term.status;
    let to = from
        .apply(event.event())
        .map_err(|refused| invalid(format!("term '{iri}': {refused:?}")))?;
    term.status = to;
    let record = serde_json::to_string(&term)
        .map_err(|error| invalid(format!("term encoding failed: {error}")))?;
    let mut conditions = Properties::new();
    conditions.insert("status".into(), status_name(from).into());
    conditions.insert(TENANT_PROPERTY.into(), target.verified.tenant().into());
    let mut updates = Properties::new();
    updates.insert("status".into(), status_name(to).into());
    updates.insert(RECORD_PROPERTY.into(), record.into());
    updates.insert("status_actor".into(), target.verified.agent_id().into());
    if let Some(request) = change_request {
        updates.insert("status_change_request".into(), request.into());
    }
    let committed = write(
        state,
        target,
        Method::CompareAndSetNodeFields {
            node_id: iri.to_string(),
            conditions_msgpack: encode(&conditions)?,
            updates_msgpack: encode(&updates)?,
        },
    )
    .await?;
    if !took_effect(&committed) {
        return Err(invalid(format!(
            "term '{iri}' changed status since it was read"
        )));
    }
    Ok(GlossaryReceipt {
        iri: iri.to_string(),
        status: Some(to),
        written: 1,
    })
}

async fn bind_term(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    iri: &str,
    asset: &str,
) -> Result<GlossaryReceipt, String> {
    let (term, _) = stored_term(state, target, iri)
        .await?
        .ok_or_else(|| invalid(format!("term '{iri}' does not exist")))?;
    let link = bind(iri, &term, asset).map_err(invalid)?;
    let mut properties = Properties::new();
    properties.insert("predicate".into(), link.predicate.into());
    properties.insert(TENANT_PROPERTY.into(), target.verified.tenant().into());
    write(
        state,
        target,
        Method::AddEdge {
            source_id: link.term,
            target_id: link.asset,
            properties_msgpack: encode(&properties)?,
        },
    )
    .await?;
    Ok(GlossaryReceipt {
        iri: iri.to_string(),
        status: Some(term.status),
        written: 1,
    })
}
