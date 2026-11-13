//! Served `ApproveEntityLink` (MEDULLA-UNIFIED-DATA-PLANE-R030.3, MEDULLA-CQ-XSRC-R004.4).
//!
//! The proposal is rebuilt on the server from the two attached records' nodes (node id
//! `<application>:<entity id>`, property `name`, optional `host`); the caller never supplies a
//! match class or a score. The Fellegi-Sunter model declares the `name` and `host` fields; the
//! calibrated probability `1 / (1 + 2^-score)` is classified by the registry knobs
//! `entity_link.match_min` and `entity_link.nonmatch_max`. A pair that is not a `Match` refuses
//! `LINK_NOT_APPROVABLE`; a missing node refuses `LINK_PROPOSAL_UNKNOWN`. The approver is the
//! verified caller. Approval writes one `owl:sameAs` edge between the nodes and nothing else.

use std::collections::BTreeMap;

use medulla_index_types::entity_resolution::{
    approve, FellegiSunterModel, FieldWeight, ResolutionRecord,
};
use medulla_server_api::knob_config::knobs;
use medulla_types::entity_link_proposal::{
    EntityLinkApproval, EntityLinkProposal, EntityRef, MatchClass,
};
use serde_json::{json, Value};

use super::*;

/// The predicate of the edge an approval writes.
const SAME_AS: &str = "owl:sameAs";

fn props_of(core: &GraphCore, node: &str) -> Option<serde_json::Map<String, Value>> {
    let blob = core.get_node_properties(node)?;
    medulla_types::msgpack::decode_property_object(&blob).ok()
}

fn record_of(entity: &EntityRef, props: &serde_json::Map<String, Value>) -> ResolutionRecord {
    let mut fields = BTreeMap::new();
    for key in ["name", "host"] {
        if let Some(text) = props.get(key).and_then(Value::as_str) {
            fields.insert(key.to_string(), text.to_string());
        }
    }
    ResolutionRecord {
        entity: entity.clone(),
        name: fields.get("name").cloned().unwrap_or_default(),
        fields,
    }
}

fn logit(probability: f64) -> f64 {
    (probability / (1.0 - probability)).log2()
}

/// The declared model: agreement weights of the two fields and the thresholds that the
/// knobs set on the calibrated probability.
fn model() -> FellegiSunterModel {
    let knobs = knobs();
    let weight = |field: &str| FieldWeight {
        field: field.to_string(),
        m: 0.97,
        u: 0.01,
        agree_at: 0.99,
    };
    FellegiSunterModel {
        fields: vec![weight("name"), weight("host")],
        upper: logit(knobs.entity_link_match_min.min(0.999_999)),
        lower: logit(knobs.entity_link_nonmatch_max.max(0.000_001)),
    }
}

/// Rebuild the proposal for a pair from its two nodes.
fn rebuild(
    core: &GraphCore,
    left: &EntityRef,
    right: &EntityRef,
) -> Result<EntityLinkProposal, String> {
    let load = |entity: &EntityRef| {
        props_of(core, &entity.node_id())
            .map(|props| record_of(entity, &props))
            .ok_or_else(|| format!("no record `{}`", entity.node_id()))
    };
    let (a, b) = (load(left)?, load(right)?);
    let model = model();
    let score = model.score(&a, &b);
    let range = (model.upper - model.lower).abs().max(f64::EPSILON);
    let scaled = ((score - model.lower) / range).clamp(0.0, 1.0);
    Ok(EntityLinkProposal {
        left: left.clone(),
        right: right.clone(),
        match_class: model.classify(score),
        match_score_milli: (scaled * 1000.0).round() as u32,
        approved_by: None,
    })
}

pub(crate) fn handle_approve(
    ctx: &QueryHandlerCtx<'_>,
    left: EntityRef,
    right: EntityRef,
) -> Response {
    use medulla_protocol_types::result_contract::query as results;
    let req_id = ctx.req_id;
    let reply = |code: &str, detail: String| Response::err(req_id, format!("{code}: {detail}"));
    let caller = ctx.caller.to_string();
    if left == right {
        return reply(
            "LINK_NOT_APPROVABLE",
            "a record cannot link to itself".into(),
        );
    }
    let proposal = match rebuild(ctx.core, &left, &right) {
        Ok(proposal) => proposal,
        Err(detail) => return reply("LINK_PROPOSAL_UNKNOWN", detail),
    };
    if proposal.match_class != MatchClass::Match {
        return reply(
            "LINK_NOT_APPROVABLE",
            format!(
                "the pair is classified {:?}, not Match",
                proposal.match_class
            ),
        );
    }
    let approved = match approve(&proposal, &caller) {
        Ok(approved) => approved,
        Err(refused) => return reply(refused.code(), "approval refused".into()),
    };
    let (source, target) = match approved.assert_same_as() {
        Ok((source, target)) => (source.node_id(), target.node_id()),
        Err(error) => return reply("LINK_NOT_APPROVABLE", error.to_string()),
    };
    let props = json!({
        "relationship": SAME_AS,
        "approved_by": caller,
        "match_score_milli": approved.match_score_milli,
    });
    let blob = match msgpack_bytes(&props) {
        Ok(blob) => blob,
        Err(error) => return reply("INTERNAL", error),
    };
    if let Err(error) = ctx.core.add_edge(source.clone(), target.clone(), blob) {
        return reply("INTERNAL", error);
    }
    let body = EntityLinkApproval {
        proposal: approved,
        source,
        target,
        predicate: SAME_AS.to_string(),
    };
    match ResultPayload::of::<results::ApproveEntityLink>(body) {
        Ok(payload) => Response::ok(req_id, payload),
        Err(error) => Response::err(req_id, error),
    }
}
