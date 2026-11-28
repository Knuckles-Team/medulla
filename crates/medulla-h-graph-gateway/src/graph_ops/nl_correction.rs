//! Served certified natural-language correction
//! (MEDULLA-FEDERATED-QUERY-R058.7).
//!
//! `NlQueryCorrect` is gateway-routed (`Route::NlCorrection`). The router has
//! already refused a caller without the `query:nl:certify` scope
//! (`NL_CORRECTION_UNAUTHORIZED`). This door validates the record and commits it
//! as the one `AddNode` upsert through the staged gateway, so it is durable and
//! replicated like every other graph write. The node id is the SHA-256 of the
//! question and both queries, so recording the same correction again returns the
//! first record and commits nothing. A correction is its own node type: it never
//! changes a certified glossary term.

use super::gateway::commit_gateway_coalescable;
use super::*;
use medulla_kernel::kernel::dispatch_support::authoritative_now_ms;
use medulla_protocol_types::result_contract::query::NlQueryCorrect;
use medulla_query_types::nl_query::{
    NlCorrection, NL_CORRECTION_LABEL, NL_TURN_QUESTION_MAX_CHARS, NL_UQL_MAX_CHARS,
};

const NODE_PREFIX: &str = "nl_correction:";

fn invalid(detail: impl std::fmt::Display) -> String {
    format!("INVALID_ARGUMENT: NlQueryCorrect: {detail}")
}

/// Validate one certified correction and build its record. The approved UQL must
/// parse with the caller grammar, so an approved pair can never teach the planner
/// text the engine refuses. Writes nothing.
pub fn record_nl_correction(
    question: &str,
    rejected_uql: &str,
    approved_uql: &str,
    certifier: &str,
    certified_at_ms: u64,
) -> Result<NlCorrection, String> {
    let question_chars = question.trim().chars().count();
    if question_chars == 0 || question_chars > NL_TURN_QUESTION_MAX_CHARS {
        return Err(invalid(format!(
            "the question must hold 1 to {NL_TURN_QUESTION_MAX_CHARS} characters"
        )));
    }
    if certifier.trim().is_empty() {
        return Err(invalid("a correction needs a verified certifier"));
    }
    for (name, text) in [
        ("rejected_uql", rejected_uql),
        ("approved_uql", approved_uql),
    ] {
        let chars = text.trim().chars().count();
        if chars == 0 || chars > NL_UQL_MAX_CHARS {
            return Err(invalid(format!(
                "{name} must hold 1 to {NL_UQL_MAX_CHARS} characters"
            )));
        }
    }
    if rejected_uql.trim() == approved_uql.trim() {
        return Err(invalid("the approved UQL equals the rejected UQL"));
    }
    medulla_uql::parse(approved_uql).map_err(|error| {
        invalid(format!(
            "approved_uql does not parse: {}",
            error.render(approved_uql)
        ))
    })?;
    Ok(NlCorrection {
        question: question.to_string(),
        rejected_uql: rejected_uql.to_string(),
        approved_uql: approved_uql.to_string(),
        certifier: certifier.to_string(),
        certified_at_ms,
    })
}

fn node_id(correction: &NlCorrection) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for part in [
        &correction.question,
        &correction.rejected_uql,
        &correction.approved_uql,
    ] {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    format!("{NODE_PREFIX}{}", hex::encode(hasher.finalize()))
}

fn node_blob(tenant_scope: &str, correction: &NlCorrection) -> Result<Vec<u8>, String> {
    let json = serde_json::to_string(correction)
        .map_err(|error| invalid(format!("record encoding failed: {error}")))?;
    rmp_serde::to_vec_named(&serde_json::json!({
        "type": NL_CORRECTION_LABEL,
        "tenant_scope": tenant_scope,
        "json": json,
    }))
    .map_err(|error| invalid(format!("record encoding failed: {error}")))
}

/// The stored correction under `node_id` of `tenant_scope`; anything else reads
/// as absent.
fn read_stored(core: &GraphCore, node_id: &str, tenant_scope: &str) -> Option<NlCorrection> {
    let blob = core.get_node_properties(node_id)?;
    let props = medulla_types::msgpack::decode_property_object(&blob).ok()?;
    if props.get("type").and_then(|v| v.as_str()) != Some(NL_CORRECTION_LABEL)
        || props.get("tenant_scope").and_then(|v| v.as_str()) != Some(tenant_scope)
    {
        return None;
    }
    serde_json::from_str(props.get("json")?.as_str()?).ok()
}

/// The `Route::NlCorrection` entry: admit the correction, then commit the node
/// upsert through the coalescable gateway. A correction already stored answers
/// with the stored record and commits nothing.
pub async fn handle_correct(
    ctx: &MutationCtx<'_>,
    _tenant_id: &str,
    method: &Method,
) -> Option<Response> {
    let Method::NlQueryCorrect {
        text,
        rejected_uql,
        approved_uql,
        ..
    } = method
    else {
        return None;
    };
    let Some(certifier) = ctx.caller.filter(|caller| !caller.trim().is_empty()) else {
        return Some(Response::err(
            ctx.req_id,
            "ACCESS_DENIED: a certified correction needs a verified caller",
        ));
    };
    let correction = match record_nl_correction(
        text,
        rejected_uql,
        approved_uql,
        certifier,
        authoritative_now_ms(),
    ) {
        Ok(correction) => correction,
        Err(error) => return Some(Response::err(ctx.req_id, error)),
    };
    let id = node_id(&correction);
    if let Some(stored) = read_stored(ctx.core, &id, ctx.tenant_scope) {
        return Some(Response::ok(
            ctx.req_id,
            ResultPayload::of::<NlQueryCorrect>(stored),
        ));
    }
    let properties_msgpack = match node_blob(ctx.tenant_scope, &correction) {
        Ok(blob) => blob,
        Err(error) => return Some(Response::err(ctx.req_id, error)),
    };
    let write = Method::AddNode {
        node_id: id,
        properties_msgpack,
    };
    let plan = MutationPlan::for_method(&write);
    let owned = write.clone();
    Some(
        commit_gateway_coalescable(ctx, &plan, &write, move |core| {
            medulla_server_mutation::mutation::apply_coalescable_write(core, &owned)?;
            ResultPayload::of::<NlQueryCorrect>(correction)
        })
        .await,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(question: &str, rejected: &str, approved: &str) -> Result<NlCorrection, String> {
        record_nl_correction(question, rejected, approved, "certifier-1", 1_000)
    }

    // spec: MEDULLA-FEDERATED-QUERY-R058.7
    #[test]
    fn a_correction_needs_a_parsable_distinct_approved_query() {
        let ok = record(
            "net revenue of orders",
            "MATCH (:Order)",
            "MATCH (:Order) |> WHERE amount_paid IS NOT NULL",
        )
        .expect("recorded");
        assert_eq!(ok.certifier, "certifier-1");
        assert_eq!(ok.certified_at_ms, 1_000);
        for refused in [
            record("net revenue", "MATCH (:Order)", "MATCH ((("),
            record("net revenue", "MATCH (:Order)", "MATCH (:Order)"),
            record("net revenue", "", "MATCH (:Order)"),
            record(
                "  ",
                "MATCH (:Order)",
                "MATCH (:Order) |> WHERE x IS NOT NULL",
            ),
            record_nl_correction("net revenue", "MATCH (:A)", "MATCH (:B)", " ", 1),
        ] {
            let text = refused.expect_err("refused");
            assert!(
                text.starts_with("INVALID_ARGUMENT: NlQueryCorrect:"),
                "{text}"
            );
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R058.7
    #[test]
    fn the_node_id_names_the_whole_correction() {
        let a = record("q one", "MATCH (:A)", "MATCH (:B)").unwrap();
        let same = record("q one", "MATCH (:A)", "MATCH (:B)").unwrap();
        let other = record("q one", "MATCH (:A)", "MATCH (:C)").unwrap();
        assert_eq!(node_id(&a), node_id(&same));
        assert_ne!(node_id(&a), node_id(&other));
        assert!(node_id(&a).starts_with(NODE_PREFIX));
    }
}
