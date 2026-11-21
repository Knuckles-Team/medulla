//! `Method::Glossary`: the served business glossary (MEDULLA-TYPED-PACKS-R801.3).
//!
//! Terms are `cat:CatalogGlossaryTerm` graph nodes in SKOS concept schemes. The
//! handler reads the verified tenant and principal from the request carrier
//! (never from the body), decides every refusal before its first write, and
//! commits through the ordinary graph gateway (`dispatch_graph_op`). Six ops:
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
//! * `ProposeDescription` stores a `cat:DescriptionProposal` node (typed properties) and opens a
//!   change request for the target, reviewed by the stewards governing it.
//! * `ReviewDescription` moves a proposal to `approved` or `rejected`; approving needs the
//!   change request approved and writes the text as the target's `description`
//!   (MEDULLA-TYPED-PACKS-R801.7).
//!
//! Reads are UQL, SPARQL and search over the same nodes; no read op exists.

use std::sync::Arc;

use medulla_catalog_types::glossary::{
    DescriptionProposal, DescriptionState, GlossaryOp, GlossaryReceipt, GlossaryTerm,
    GlossaryTermEntry, GlossaryTermStatus, CAT_GLOSSARY_TERM, SKOS_IN_SCHEME,
};
use medulla_core::glossary::{
    bind, check_broader_acyclic_over, check_label_free, require_review, GlossaryError, PutScheme,
};
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_protocol_types::result_contract::ingestion::{
    GlossaryBind, GlossaryProposeDescription, GlossaryPutScheme, GlossaryPutTerm,
    GlossaryReviewDescription, GlossaryTransition,
};
use medulla_server_api::state::ServerState;
use tokio::sync::RwLock;

use super::graph_client::{
    encode, read_node, took_effect, write, write_batch, GraphTarget, Properties, TENANT_PROPERTY,
};
use super::shared::typed_response;
use super::stewardship::stored_request;

/// The class of a concept scheme node.
const SCHEME_CLASS: &str = "http://www.w3.org/2004/02/skos/core#ConceptScheme";
/// The property holding the serialized term.
const RECORD_PROPERTY: &str = "record";
/// Node label of one status move of a term: the supersession chain a term's history reads from.
pub const STATUS_MOVE_LABEL: &str = "cat:GlossaryStatusMove";
const PAGE: usize = 1000;

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
        GlossaryOp::ProposeDescription { .. } => {
            typed_response::<GlossaryProposeDescription>(req_id, result)
        }
        GlossaryOp::ReviewDescription { .. } => {
            typed_response::<GlossaryReviewDescription>(req_id, result)
        }
    }
}

fn invalid(detail: impl std::fmt::Display) -> String {
    format!("INVALID_ARGUMENT: {detail}")
}

/// A rule's refusal under its registered code, or `INVALID_ARGUMENT` when it has none
/// (MEDULLA-TYPED-PACKS-R801.4).
fn glossary_refusal(error: GlossaryError) -> String {
    match error.code() {
        Some(code) => format!("{}: {error}", code.as_str()),
        None => invalid(error),
    }
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
        GlossaryOp::ProposeDescription { proposal } => propose_description(state, target, proposal).await,
        GlossaryOp::ReviewDescription { proposal, approve } => {
            review_description(state, target, proposal, *approve).await
        }
    }
}

/// The stored term of `iri` in the caller's tenant, with its node properties.
pub(super) async fn stored_term(
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

/// Every stored term of `scheme` in the verified tenant, with its IRI.
async fn scheme_terms(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    scheme: &str,
) -> Result<Vec<(String, GlossaryTerm)>, String> {
    let tenant = target.verified.tenant();
    let mut found = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let response = super::graph_pipeline::dispatch_graph_op(
            state,
            target.graph,
            target.req_id,
            target.caller,
            target.verified,
            Method::GetNodesByLabel {
                label: CAT_GLOSSARY_TERM.to_string(),
                after: after.clone(),
                limit: PAGE,
            },
        )
        .await;
        if let Some(refusal) = response.refusal_text() {
            return Err(refusal);
        }
        let Some(ResultPayload::NodeList(nodes)) = response.result else {
            break;
        };
        let count = nodes.len();
        after = nodes.last().map(|(id, _)| id.clone());
        for (id, properties) in nodes {
            let text = |key: &str| properties.get(key).and_then(serde_json::Value::as_str);
            if text(TENANT_PROPERTY) != Some(tenant) || text("scheme") != Some(scheme) {
                continue;
            }
            let Some(term) = text(RECORD_PROPERTY)
                .and_then(|body| serde_json::from_str::<GlossaryTerm>(body).ok())
            else {
                continue;
            };
            found.push((id, term));
        }
        if count < PAGE {
            break;
        }
    }
    Ok(found)
}

/// The review a subject needs before it is approved: an approved change request naming it
/// (MEDULLA-TYPED-PACKS-R801.4).
async fn review_gate(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    iri: &str,
    change_request: Option<&str>,
) -> Result<(), String> {
    let request = match change_request {
        Some(id) => stored_request(state, target, id)
            .await
            .ok()
            .map(|stored| stored.request),
        None => None,
    };
    require_review(iri, request.as_ref()).map_err(glossary_refusal)
}

/// The review a term needs before it is approved, and a preferred label no other approved term
/// of the scheme carries (MEDULLA-TYPED-PACKS-R801.4).
async fn approval_gate(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    iri: &str,
    term: &GlossaryTerm,
    change_request: Option<&str>,
) -> Result<(), String> {
    review_gate(state, target, iri, change_request).await?;
    let stored = scheme_terms(state, target, &term.scheme).await?;
    check_label_free(&stored, iri, term).map_err(glossary_refusal)
}

/// What a status move addresses: a glossary term, or an article, which has no term record
/// (MEDULLA-TYPED-PACKS-R108.13).
enum Subject {
    Term(GlossaryTerm),
    Article {
        status: GlossaryTermStatus,
        status_key: String,
        properties: Properties,
    },
}

const ARTICLE_STATUS_LOCAL: &str = "articleStatus";

/// The stored subject of `iri` in the caller's tenant.
async fn stored_subject(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    iri: &str,
) -> Result<Option<Subject>, String> {
    let Some(properties) = read_node(state, target, iri).await? else {
        return Ok(None);
    };
    let is_article = properties
        .get("type")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|label| label == "Article" || label.ends_with("#Article"));
    if !is_article {
        return Ok(stored_term(state, target, iri)
            .await
            .map_err(|refusal| {
                if refusal.contains("is not a glossary term") {
                    invalid(format!("node '{iri}' is not a glossary term or article"))
                } else {
                    refusal
                }
            })?
            .map(|(term, _)| Subject::Term(term)));
    }
    // An article written through `AddTriples` carries no tenant property of its own; one that
    // does carry it must name the caller's tenant.
    if properties
        .get(TENANT_PROPERTY)
        .and_then(serde_json::Value::as_str)
        .is_some_and(|tenant| tenant != target.verified.tenant())
    {
        return Err(invalid(format!("article '{iri}' belongs to another tenant")));
    }
    let status_key = properties
        .keys()
        .find(|key| *key == ARTICLE_STATUS_LOCAL || key.ends_with("#articleStatus"))
        .cloned()
        .ok_or_else(|| invalid(format!("article '{iri}' has no status")))?;
    let name = properties
        .get(&status_key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let status: GlossaryTermStatus = serde_json::from_value(serde_json::Value::String(name.to_string()))
        .map_err(|_| invalid(format!("article '{iri}' has the unknown status '{name}'")))?;
    Ok(Some(Subject::Article {
        status,
        status_key,
        properties,
    }))
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
    batch.validate().map_err(glossary_refusal)?;
    let stored = scheme_terms(state, target, scheme).await?;
    check_broader_acyclic_over(stored, &batch.terms).map_err(glossary_refusal)?;
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
    single.validate().map_err(glossary_refusal)?;
    let stored = scheme_terms(state, target, &term.scheme).await?;
    check_broader_acyclic_over(stored, &single.terms).map_err(glossary_refusal)?;
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
    let subject = stored_subject(state, target, iri)
        .await?
        .ok_or_else(|| invalid(format!("'{iri}' does not exist")))?;
    let from = match &subject {
        Subject::Term(term) => term.status,
        Subject::Article { status, .. } => *status,
    };
    let to = from
        .apply(event.event())
        .map_err(|refused| invalid(format!("'{iri}': {refused:?}")))?;
    if event == medulla_catalog_types::glossary::GlossaryTransition::Approve {
        match &subject {
            Subject::Term(term) => approval_gate(state, target, iri, term, change_request).await?,
            Subject::Article { .. } => review_gate(state, target, iri, change_request).await?,
        }
    }
    let tenant = target.verified.tenant();
    let actor = target.verified.agent_id();
    // The subject and its move record commit as one batch: the subject carries the latest actor
    // and request, and each move is one `cat:GlossaryStatusMove` node chained to the move into
    // the state it left through `supersedes`, so the history reads by UQL (R801.5). An article
    // node changes only its status, actor and request; every other stored property is kept.
    let (mut properties, kind) = match subject {
        Subject::Term(mut term) => {
            term.status = to;
            (term_properties(tenant, iri, &term)?, "term")
        }
        Subject::Article {
            status_key,
            mut properties,
            ..
        } => {
            properties.insert(status_key, status_name(to).into());
            (properties, "article")
        }
    };
    properties.insert("status_actor".into(), actor.into());
    if let Some(request) = change_request {
        properties.insert("status_change_request".into(), request.into());
    }
    let mut record = Properties::new();
    record.insert("type".into(), STATUS_MOVE_LABEL.into());
    record.insert("subject".into(), iri.into());
    record.insert("subject_kind".into(), kind.into());
    record.insert("from".into(), status_name(from).into());
    record.insert("to".into(), status_name(to).into());
    record.insert("actor".into(), actor.into());
    if let Some(request) = change_request {
        record.insert("change_request".into(), request.into());
    }
    if from != GlossaryTermStatus::Draft {
        record.insert(
            "supersedes".into(),
            move_id(tenant, iri, status_name(from)).into(),
        );
    }
    record.insert(TENANT_PROPERTY.into(), tenant.into());
    write_batch(
        state,
        target,
        &[
            serde_json::json!({"op": "upsert_node", "id": iri, "properties": properties}),
            serde_json::json!({
                "op": "upsert_node",
                "id": move_id(tenant, iri, status_name(to)),
                "properties": record,
            }),
        ],
    )
    .await?;
    Ok(GlossaryReceipt {
        iri: iri.to_string(),
        status: Some(to),
        written: 2,
    })
}

/// The id of the move record of `iri` into `status`.
fn move_id(tenant: &str, iri: &str, status: &str) -> String {
    format!("glossary_move:{tenant}:{iri}:{status}")
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
    let link = bind(iri, &term, asset).map_err(glossary_refusal)?;
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

/// Node label of a description proposal.
pub const DESCRIPTION_PROPOSAL_LABEL: &str = "cat:DescriptionProposal";

fn proposal_digest(text: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("sha256:{:x}", Sha256::digest(text.as_bytes()))
}

fn proposal_id(tenant: &str, target: &str, digest: &str) -> String {
    format!("description_proposal:{tenant}:{target}:{digest}")
}

async fn propose_description(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    proposal: &DescriptionProposal,
) -> Result<GlossaryReceipt, String> {
    use medulla_types::contract::ServerErrorCode;
    if proposal.inputs.is_empty() {
        return Err(format!(
            "{}: a proposed description names at least one input",
            ServerErrorCode::ProposalInputsMissing.as_str()
        ));
    }
    if proposal.state != DescriptionState::Proposed {
        return Err(invalid("a new proposal is in the proposed state"));
    }
    if proposal.text.trim().is_empty() || proposal.target.trim().is_empty() {
        return Err(invalid("a proposal names its target and its text"));
    }
    if read_node(state, target, &proposal.target).await?.is_none() {
        return Err(invalid(format!("target '{}' does not exist", proposal.target)));
    }
    let tenant = target.verified.tenant();
    let digest = proposal_digest(&proposal.text);
    let id = proposal_id(tenant, &proposal.target, &digest);
    if read_node(state, target, &id).await?.is_some() {
        return Ok(GlossaryReceipt {
            iri: id,
            status: None,
            written: 0,
        });
    }
    // The change request is opened first: a target nobody governs has no reviewer, so its
    // proposal could never be approved.
    let request = super::stewardship::open_governed_change(
        state,
        target,
        &proposal.target,
        &digest,
        std::slice::from_ref(&proposal.target),
    )
    .await?
    .ok_or_else(|| {
        format!(
            "{}: no owner or steward governs '{}'",
            medulla_types::contract::ServerErrorCode::StewardRoleMissing.as_str(),
            proposal.target
        )
    })?;
    let mut properties = Properties::new();
    properties.insert("type".into(), DESCRIPTION_PROPOSAL_LABEL.into());
    properties.insert("target".into(), proposal.target.clone().into());
    properties.insert("text".into(), proposal.text.clone().into());
    properties.insert("text_digest".into(), digest.into());
    if let Some(samples) = &proposal.inputs.samples_digest {
        properties.insert("samples_digest".into(), samples.clone().into());
    }
    properties.insert("lineage_refs".into(), serde_json::json!(proposal.inputs.lineage_refs));
    properties.insert("usage_refs".into(), serde_json::json!(proposal.inputs.usage_refs));
    properties.insert("similar_refs".into(), serde_json::json!(proposal.inputs.similar_refs));
    properties.insert("state".into(), DescriptionState::Proposed.as_str().into());
    properties.insert("proposed_by".into(), target.verified.agent_id().into());
    properties.insert("change_request".into(), request.into());
    properties.insert(TENANT_PROPERTY.into(), tenant.into());
    write(
        state,
        target,
        Method::CreateNodeIfAbsent {
            node_id: id.clone(),
            properties_msgpack: encode(&properties)?,
        },
    )
    .await?;
    Ok(GlossaryReceipt {
        iri: id,
        status: None,
        written: 1,
    })
}

async fn review_description(
    state: &Arc<RwLock<ServerState>>,
    target: &GraphTarget<'_>,
    id: &str,
    approve: bool,
) -> Result<GlossaryReceipt, String> {
    use medulla_types::contract::ServerErrorCode;
    let properties = read_node(state, target, id)
        .await?
        .ok_or_else(|| invalid(format!("proposal '{id}' does not exist")))?;
    let text = |key: &str| properties.get(key).and_then(serde_json::Value::as_str);
    if text(TENANT_PROPERTY) != Some(target.verified.tenant())
        || text("type") != Some(DESCRIPTION_PROPOSAL_LABEL)
    {
        return Err(invalid(format!("'{id}' is not a proposal of this tenant")));
    }
    if text("state") != Some(DescriptionState::Proposed.as_str()) {
        return Err(invalid(format!("proposal '{id}' is already decided")));
    }
    let subject = text("target").ok_or_else(|| invalid("the stored proposal has no target"))?;
    let description = text("text").ok_or_else(|| invalid("the stored proposal has no text"))?;
    let request_id = text("change_request")
        .ok_or_else(|| invalid("the stored proposal has no change request"))?;
    let stored = stored_request(state, target, request_id).await?;
    if !stored
        .request
        .reviewers
        .iter()
        .any(|reviewer| reviewer == target.verified.agent_id())
    {
        return Err(format!(
            "{}: the caller is not a reviewer of '{subject}'",
            ServerErrorCode::StewardRoleMissing.as_str()
        ));
    }
    if approve {
        require_review(subject, Some(&stored.request)).map_err(glossary_refusal)?;
    }
    let state_after = if approve {
        DescriptionState::Approved
    } else {
        DescriptionState::Rejected
    };
    let mut conditions = Properties::new();
    conditions.insert("state".into(), DescriptionState::Proposed.as_str().into());
    let mut updates = Properties::new();
    updates.insert("state".into(), state_after.as_str().into());
    updates.insert("reviewed_by".into(), target.verified.agent_id().into());
    let committed = write(
        state,
        target,
        Method::CompareAndSetNodeFields {
            node_id: id.to_string(),
            conditions_msgpack: encode(&conditions)?,
            updates_msgpack: encode(&updates)?,
        },
    )
    .await?;
    if !took_effect(&committed) {
        return Err(invalid(format!("proposal '{id}' changed since it was read")));
    }
    let mut written = 1;
    if approve {
        let mut subject_properties = read_node(state, target, subject)
            .await?
            .ok_or_else(|| invalid(format!("target '{subject}' no longer exists")))?;
        subject_properties.insert("description".into(), description.into());
        write(
            state,
            target,
            Method::AddNode {
                node_id: subject.to_string(),
                properties_msgpack: encode(&subject_properties)?,
            },
        )
        .await?;
        written += 1;
    }
    Ok(GlossaryReceipt {
        iri: id.to_string(),
        status: None,
        written,
    })
}
