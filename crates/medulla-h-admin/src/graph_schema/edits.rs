//! Ontology edit chain storage (MEDULLA-TYPED-PACKS-R651.3, R651.5).
//!
//! Edits and naming findings are graph nodes written on the gateway's staged
//! [`GraphCore`], so they commit, audit and replicate with the schema change
//! and UQL reads them as ordinary labelled nodes. One head node keeps the
//! chain count and the newest digest, which makes an append O(1).

use medulla_catalog_types::graph_schema::GraphSchemaOp;
use medulla_core::ontology::edits::{
    edit_digest, revert, verify_chain, verify_head, EditDraft, EditRefusal,
};
use medulla_graph_core::graph::GraphCore;
use medulla_types::contract::Digest256;
use medulla_types::ontology_edit::{EditKind, NamingFinding, NamingMode, OntologyEdit};
use serde_json::{json, Value};

const EDIT_LABEL: &str = "OntologyEdit";
const FINDING_LABEL: &str = "NamingFinding";
const HEAD_NODE: &str = "ontology-edit-head";

fn edit_node_id(edit_id: &str) -> String {
    format!("ontology-edit:{edit_id}")
}

fn refusal(error: EditRefusal) -> String {
    error.to_string()
}

pub(super) fn put(core: &GraphCore, node_id: String, label: &str, mut value: Value) -> Result<(), String> {
    value["label"] = json!(label);
    let bytes = rmp_serde::to_vec_named(&value)
        .map_err(|error| format!("ontology edit node encode failed: {error}"))?;
    core.add_node(node_id, bytes);
    Ok(())
}

fn read(core: &GraphCore, node_id: &str) -> Option<Value> {
    let bytes = core.get_node_properties(node_id)?;
    rmp_serde::from_slice(&bytes).ok()
}

fn decode_edit(mut value: Value) -> Option<OntologyEdit> {
    value.as_object_mut()?.remove("label");
    serde_json::from_value(value).ok()
}

/// The recorded chain length and newest digest (`(0, None)` before any edit).
fn read_head(core: &GraphCore) -> (usize, Option<Digest256>) {
    let Some(value) = read(core, HEAD_NODE) else {
        return (0, None);
    };
    let count = value["count"]
        .as_u64()
        .and_then(|count| usize::try_from(count).ok())
        .unwrap_or(0);
    let head = value["head"]
        .as_str()
        .and_then(|hex| Digest256::parse(hex).ok());
    (count, head)
}

fn write_head(core: &GraphCore, count: usize, head: Digest256) -> Result<(), String> {
    put(
        core,
        HEAD_NODE.to_string(),
        "OntologyEditHead",
        json!({ "count": count, "head": head.to_hex() }),
    )
}

fn store_edit(core: &GraphCore, edit: &OntologyEdit) -> Result<(), String> {
    let value = serde_json::to_value(edit).map_err(|error| error.to_string())?;
    put(core, edit_node_id(&edit.edit_id), EDIT_LABEL, value)
}

/// The digest of the newest edit, the value `GraphSchema` results return.
pub(super) fn head(core: &GraphCore) -> Option<Digest256> {
    read_head(core).1
}

/// The whole chain in order, verified against the recorded head.
fn load_chain(core: &GraphCore) -> Result<Vec<OntologyEdit>, String> {
    let mut chain: Vec<OntologyEdit> = core
        .get_nodes_by_label(EDIT_LABEL, 0)
        .into_iter()
        .filter_map(|(_, bytes)| decode_edit(rmp_serde::from_slice(&bytes).ok()?))
        .collect();
    chain.sort_by(|left, right| left.edit_id.cmp(&right.edit_id));
    verify_chain(&chain, head(core)).map_err(refusal)?;
    Ok(chain)
}

/// Append one edit after checking the newest stored edit against the head.
pub(super) fn record(core: &GraphCore, draft: EditDraft) -> Result<Digest256, String> {
    let (count, recorded) = read_head(core);
    let newest = (count > 0)
        .then(|| read(core, &edit_node_id(&format!("edit-{count:08}"))))
        .flatten()
        .and_then(decode_edit);
    verify_head(newest.as_ref(), recorded).map_err(refusal)?;
    let edit = draft.link_after(count, recorded);
    store_edit(core, &edit)?;
    let digest = edit_digest(&edit);
    write_head(core, count + 1, digest)?;
    Ok(digest)
}

/// `RevertEdit`: append the inversions of every edit of the subject, newest
/// first, and return the new head. A refusal appends nothing.
pub(super) fn revert_subject(core: &GraphCore, subject: &str) -> Result<Option<Digest256>, String> {
    let mut chain = load_chain(core)?;
    let stored = chain.len();
    let appended = revert(&mut chain, subject, None).map_err(refusal)?;
    for edit in &appended {
        store_edit(core, edit)?;
    }
    if let Some(newest) = appended.last() {
        write_head(core, stored + appended.len(), edit_digest(newest))?;
    }
    Ok(head(core))
}

/// The content digest a schema source carries, if it is attached.
pub(super) fn source_digest(core: &GraphCore, source_id: &str) -> Option<Digest256> {
    let sources = core.schema_sources();
    let source = sources.dynamic.get(source_id)?;
    source.ontology_sha256.or(source.shapes_sha256)
}

fn kind_of(before: Option<Digest256>, after: Option<Digest256>) -> EditKind {
    match (before, after) {
        (None, _) => EditKind::Add,
        (Some(_), None) => EditKind::Remove,
        (Some(_), Some(_)) => EditKind::Change,
    }
}

/// Record the change of `source_id` from `before` to its digest now.
pub(super) fn record_change(
    core: &GraphCore,
    subject_iri: &str,
    source_id: &str,
    before: Option<Digest256>,
    sealed_by: Option<String>,
) -> Result<Option<Digest256>, String> {
    let after = source_digest(core, source_id);
    record(
        core,
        EditDraft {
            kind: kind_of(before, after),
            subject_iri: subject_iri.to_string(),
            before_digest: before,
            after_digest: after,
            sealed_by,
        },
    )
    .map(Some)
}

/// After an `AttachApproved`, the edit sealed by its approval lease; every
/// other attach or detach is not an ontology edit and returns the head.
pub(super) fn after_attach(
    core: &GraphCore,
    op: &GraphSchemaOp,
    before: Option<Digest256>,
    changed: bool,
) -> Result<Option<Digest256>, String> {
    match op {
        GraphSchemaOp::AttachApproved {
            source_id,
            approval_lease_id,
            ..
        } if changed => record_change(
            core,
            source_id,
            source_id,
            before,
            Some(approval_lease_id.clone()),
        ),
        _ => Ok(head(core)),
    }
}

/// Naming findings of one ontology document under `mode`.
pub(super) fn lint(ontology_ttl: &str, mode: NamingMode) -> Result<Vec<NamingFinding>, String> {
    let (classes, _) = medulla_rdf::pack::declared_classes_and_ontologies(ontology_ttl);
    let properties = medulla_rdf::pack::declared_properties(ontology_ttl);
    let findings = medulla_core::ontology::edits::lint_names(&classes, &properties, mode);
    medulla_core::ontology::edits::refuse_naming_errors(&findings).map_err(refusal)?;
    Ok(findings)
}

/// Store findings as nodes UQL reads; one node per subject and rule.
pub(super) fn store_findings(core: &GraphCore, findings: &[NamingFinding]) -> Result<(), String> {
    for finding in findings {
        let key = format!("{}|{}", finding.subject_iri, finding.rule);
        let id = format!(
            "naming-finding:{}",
            Digest256::sha256(key.as_bytes()).to_hex()
        );
        let value = serde_json::to_value(finding).map_err(|error| error.to_string())?;
        put(core, id, FINDING_LABEL, value)?;
    }
    Ok(())
}

/// The naming mode the `ontology.naming_mode` knob selects.
pub(super) fn naming_mode() -> NamingMode {
    NamingMode::from_knob(medulla_server_api::knob_config::knobs().ontology_naming_mode)
}
