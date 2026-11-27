//! Served retrieval feedback (MEDULLA-FEDERATED-QUERY-R705.3).
//!
//! `RetrievalFeedbackRecord` is gateway-routed (`Route::RetrievalFeedback`): a caller
//! sends only `upvote` or `downvote`; the handler replaces `stamp` with the verified
//! tenant and principal and the engine clock, then commits one `RetrievalFeedback` node
//! through the staged gateway (`commit_gateway`). The stamped method is what is logged, so
//! `durable_apply::apply_retrieval_feedback` rebuilds the same node on replay.
//!
//! [`load_feedback`] is the read side: `Discover` reads the explicit events of the
//! caller's projected view and the implicit events derived from stored retrieval
//! outcomes, and turns them into per-document boosts (`document_boost`).

use super::*;
use medulla_protocol_types::result_contract::query::RetrievalFeedbackRecord;
use medulla_result_types::feedback_event::{
    FeedbackStamp, RetrievalFeedback, RETRIEVAL_FEEDBACK_LABEL,
};
use medulla_retrieval::feedback_boost::{document_boost, DocumentBoost, DEFAULT_HALF_LIFE_S};
use medulla_server_ops::api::ingest::graph::commit_gateway;

fn refuse(ctx: &MutationCtx<'_>, detail: &str) -> Option<Response> {
    Some(Response::err(
        ctx.req_id,
        format!("INVALID_ARGUMENT: {detail}"),
    ))
}

/// The `Route::RetrievalFeedback` entry: validate, stamp, then commit the one node.
pub(super) async fn handle_record(
    ctx: &MutationCtx<'_>,
    tenant_id: &str,
    method: &Method,
) -> Option<Response> {
    let Method::RetrievalFeedbackRecord {
        document_ref,
        kind,
        weight,
        ..
    } = method
    else {
        return None;
    };
    if !kind.is_explicit() {
        return refuse(ctx, "implicit feedback kinds are derived, never recorded");
    }
    if !weight.is_finite() || !(-1.0..=1.0).contains(weight) {
        return refuse(ctx, "weight must be finite and in [-1.0, 1.0]");
    }
    if document_ref.trim().is_empty() {
        return refuse(ctx, "empty document_ref");
    }
    let Some(principal) = ctx.principal.or(ctx.caller) else {
        return Some(Response::err(
            ctx.req_id,
            "ACCESS_DENIED: retrieval feedback needs a verified principal",
        ));
    };
    let now_s = medulla_kernel::kernel::dispatch_support::authoritative_now_ms() / 1000;
    // MEDULLA-FEDERATED-QUERY-R705.4: a refusal writes nothing, so both checks precede
    // the commit. A document the principal cannot read is refused exactly like an absent
    // one, so the refusal leaks nothing about it.
    let mut readable = ctx.core.analysis_snapshot();
    ctx.isolation
        .filter_view(ctx.caller.unwrap_or(principal), &mut readable);
    if !readable.node_map.contains_key(document_ref.as_str()) {
        return Some(Response::err(
            ctx.req_id,
            format!(
                "{}: the document is not readable by this principal",
                medulla_types::contract::ServerErrorCode::FeedbackTargetHidden
            ),
        ));
    }
    let limit = medulla_server_api::knob_config::knobs().feedback_rate_limit_per_hour;
    if recent_events(ctx.core, tenant_id, principal, now_s) >= limit {
        return Some(Response::err(
            ctx.req_id,
            format!(
                "{}: {limit} feedback events were already recorded in the last hour",
                medulla_types::contract::ServerErrorCode::FeedbackRateLimit
            ),
        ));
    }
    // A wire stamp is replaced, never trusted.
    let stamp = FeedbackStamp {
        tenant: tenant_id.to_string(),
        principal: principal.to_string(),
        at: now_s,
        idempotency_key: ctx.idempotency_key.to_string(),
    };
    let event = RetrievalFeedback::from_record(document_ref, *kind, *weight, &stamp);
    let node_id = match event.node_id() {
        Ok(id) => id,
        Err(error) => return refuse(ctx, &error),
    };
    let props = match rmp_serde::to_vec_named(&event.node_properties()) {
        Ok(props) => props,
        Err(error) => return refuse(ctx, &format!("feedback encoding failed: {error}")),
    };
    let stamped = Method::RetrievalFeedbackRecord {
        document_ref: document_ref.clone(),
        kind: *kind,
        weight: *weight,
        stamp: Some(stamp),
    };
    let plan = MutationPlan::for_method(&stamped);
    Some(
        commit_gateway(ctx, &plan, &stamped, move |core| {
            core.remove_node(node_id.clone());
            core.add_node(node_id, props);
            ResultPayload::of::<RetrievalFeedbackRecord>(event)
        })
        .await,
    )
}

/// The principal's `RetrievalFeedback` events in the hour before `now_s`: a query over the
/// stored nodes by time, with no separate counter (MEDULLA-FEDERATED-QUERY-R705.4).
fn recent_events(core: &GraphCore, tenant: &str, principal: &str, now_s: u64) -> u64 {
    const HOUR_S: u64 = 3_600;
    let count = core
        .get_nodes_by_label(RETRIEVAL_FEEDBACK_LABEL, 0)
        .into_iter()
        .filter_map(|(_, blob)| {
            let props = medulla_types::msgpack::decode_property_object(&blob).ok()?;
            RetrievalFeedback::from_node_properties(&props)
        })
        .filter(|event| {
            event.tenant == tenant
                && event.principal == principal
                && event.at.saturating_add(HOUR_S) > now_s
        })
        .count();
    u64::try_from(count).unwrap_or(u64::MAX)
}

/// The knobs of the read side, resolved once per `Discover` call.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FeedbackKnobs {
    pub events_per_doc: usize,
    pub implicit_outcomes: usize,
    pub cited_weight: f64,
    pub ignored_weight: f64,
}

impl FeedbackKnobs {
    pub(crate) fn from_registry() -> Self {
        let knobs = medulla_server_api::knob_config::knobs();
        Self {
            events_per_doc: usize::try_from(knobs.feedback_events_per_doc).unwrap_or(usize::MAX),
            implicit_outcomes: usize::try_from(knobs.feedback_implicit_outcomes)
                .unwrap_or(usize::MAX),
            cited_weight: knobs.feedback_cited_weight,
            ignored_weight: knobs.feedback_ignored_weight,
        }
    }
}

/// The implicit events of the tenant's stored retrieval outcomes: a `cited` event for each
/// cited unit, an `ignored` event for each returned unit that was not. Nothing is written.
#[cfg(feature = "redb")]
pub(crate) async fn load_implicit_events(
    state: &Arc<RwLock<ServerState>>,
    tenant: &str,
    knobs: &FeedbackKnobs,
) -> Vec<RetrievalFeedback> {
    use medulla_decision_types::decision::statistical::retrieval::StoredRetrievalOutcome;
    use medulla_persistence_records::agent_library::AgentLibraryStateExt;
    use medulla_persistence_records::decision_jobs::{decode_artifact, RETRIEVAL_OUTCOME_PREFIX};
    let Some(store) = state.read().await.agent_library() else {
        return Vec::new();
    };
    let Ok(rows) = store.decision_artifacts_with_prefix(
        tenant,
        RETRIEVAL_OUTCOME_PREFIX,
        knobs.implicit_outcomes,
    ) else {
        return Vec::new();
    };
    let mut events = Vec::new();
    for (_, bytes) in rows {
        let Ok(stored) = decode_artifact::<StoredRetrievalOutcome>(&bytes, "retrieval outcome")
        else {
            continue;
        };
        let returned: Vec<String> = stored
            .outcome
            .returned
            .iter()
            .map(|unit| unit.evidence_id.clone())
            .collect();
        let cited: Vec<String> = stored.outcome.cited.iter().cloned().collect();
        events.extend(medulla_retrieval::feedback_boost::implicit_events(
            &cited,
            &returned,
            stored.recorded_at_ms / 1000,
            &stored.producer,
            tenant,
            (knobs.cited_weight, knobs.ignored_weight),
        ));
    }
    events
}

/// Without a control store there is no stored outcome to derive events from.
#[cfg(not(feature = "redb"))]
pub(crate) async fn load_implicit_events(
    _state: &Arc<RwLock<ServerState>>,
    _tenant: &str,
    _knobs: &FeedbackKnobs,
) -> Vec<RetrievalFeedback> {
    Vec::new()
}

/// The boosts of the candidate documents: the visible `RetrievalFeedback` nodes of the
/// caller's projected `view` whose `document_ref` is a candidate (newest first, at most
/// `events_per_doc` per document) plus the already-derived `implicit` events, folded by
/// `document_boost`.
pub(crate) fn load_feedback(
    view: &GraphCore,
    tenant: &str,
    candidates: &[String],
    now_s: u64,
    knobs: &FeedbackKnobs,
    implicit: &[RetrievalFeedback],
) -> Vec<DocumentBoost> {
    let wanted: std::collections::BTreeSet<&str> = candidates.iter().map(String::as_str).collect();
    let mut events: Vec<RetrievalFeedback> = view
        .get_nodes_by_label(RETRIEVAL_FEEDBACK_LABEL, 0)
        .into_iter()
        .filter_map(|(_, blob)| {
            let props = medulla_types::msgpack::decode_property_object(&blob).ok()?;
            RetrievalFeedback::from_node_properties(&props)
        })
        .filter(|event| event.tenant == tenant && wanted.contains(event.document_ref.as_str()))
        .collect();
    events.sort_by_key(|event| std::cmp::Reverse(event.at));
    let mut kept: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    events.retain(|event| {
        let count = kept.entry(event.document_ref.clone()).or_insert(0);
        *count += 1;
        *count <= knobs.events_per_doc
    });
    events.extend(
        implicit
            .iter()
            .filter(|event| wanted.contains(event.document_ref.as_str()))
            .cloned(),
    );
    candidates
        .iter()
        .map(|id| document_boost(id, &events, now_s, DEFAULT_HALF_LIFE_S))
        .filter(|boost| boost.value != 0.0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_result_types::feedback_event::FeedbackKind;

    fn stored(core: &GraphCore, document: &str, kind: FeedbackKind, weight: f64, at: u64) {
        let event = RetrievalFeedback {
            tenant: "t1".into(),
            principal: "alice".into(),
            document_ref: document.into(),
            kind,
            weight,
            at,
            idempotency_key: format!("{document}-{at}"),
        };
        core.add_node(
            event.node_id().unwrap(),
            rmp_serde::to_vec_named(&event.node_properties()).unwrap(),
        );
    }

    // spec: MEDULLA-FEDERATED-QUERY-R705.3
    #[test]
    fn wire_stamp_is_replaced_and_implicit_kinds_refused() {
        // Implicit kinds are never recordable.
        assert!(FeedbackKind::Upvote.is_explicit());
        assert!(FeedbackKind::Downvote.is_explicit());
        assert!(!FeedbackKind::Cited.is_explicit());
        assert!(!FeedbackKind::Ignored.is_explicit());

        // The stored node carries the stamped tenant and principal, never a wire value, and
        // the replay arm rebuilds the same node from the logged, stamped method.
        let core = GraphCore::new();
        let stamp = FeedbackStamp {
            tenant: "t1".into(),
            principal: "alice".into(),
            at: 100,
            idempotency_key: "req-1".into(),
        };
        let logged = Method::RetrievalFeedbackRecord {
            document_ref: "doc".into(),
            kind: FeedbackKind::Upvote,
            weight: 0.2,
            stamp: Some(stamp.clone()),
        };
        assert!(medulla_core::durable_apply::apply(&core, &logged).is_ok());
        let rows = core.get_nodes_by_label(RETRIEVAL_FEEDBACK_LABEL, 0);
        assert_eq!(rows.len(), 1);
        let props = medulla_types::msgpack::decode_property_object(&rows[0].1).unwrap();
        let event = RetrievalFeedback::from_node_properties(&props).unwrap();
        assert_eq!(event.tenant, "t1");
        assert_eq!(event.principal, "alice");
        assert_eq!(
            rows[0].0,
            RetrievalFeedback::from_record("doc", FeedbackKind::Upvote, 0.2, &stamp)
                .node_id()
                .unwrap()
        );
    }

    // spec: MEDULLA-FEDERATED-QUERY-R705.3
    #[test]
    fn load_feedback_reads_tenant_candidates_and_implicit_events() {
        let core = GraphCore::new();
        stored(&core, "a", FeedbackKind::Upvote, 0.2, 100);
        stored(&core, "b", FeedbackKind::Downvote, -0.2, 100);
        let knobs = FeedbackKnobs {
            events_per_doc: 10,
            implicit_outcomes: 16,
            cited_weight: 0.05,
            ignored_weight: -0.02,
        };
        let candidates = vec!["a".to_string(), "c".to_string()];
        let implicit = vec![RetrievalFeedback {
            tenant: "t1".into(),
            principal: "p".into(),
            document_ref: "c".into(),
            kind: FeedbackKind::Cited,
            weight: 0.05,
            at: 100,
            idempotency_key: String::new(),
        }];
        let boosts = load_feedback(&core, "t1", &candidates, 100, &knobs, &implicit);
        let value = |id: &str| {
            boosts
                .iter()
                .find(|b| b.document_ref == id)
                .map(|b| b.value)
        };
        assert_eq!(value("a"), Some(0.2));
        assert_eq!(value("c"), Some(0.05));
        assert_eq!(value("b"), None);
        assert!(load_feedback(&core, "other", &candidates, 100, &knobs, &[])
            .iter()
            .all(|b| b.document_ref != "a"));
    }
}
