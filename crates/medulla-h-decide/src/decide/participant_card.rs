//! Participant cards over the claim store (MEDULLA-DECISION-ENGINE-R655.3, R655.4).
//!
//! `plan_card_refresh` recomputes the card of one subject and observer from their claims and
//! answers the graph operations that store it, or nothing when the digest did not change; the
//! router commits them after the claim that changed them. `handle_participant_card` is the card
//! read: a lookup of the stored node, never a model call, under the visibility rule of the ask.

use super::SharedState;
use medulla_decision_types::decision::knob_families::participant::default_u64;
use medulla_decision_types::participant_memory::ParticipantCard;
use medulla_protocol_types::protocol::{Response, ResultPayload};
use medulla_protocol_types::result_contract::coordination::ParticipantReadCard;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_work_types::participant::ParticipantId;
use {
    medulla_decision_statistical_types::claims::ClaimRecord,
    medulla_decision_statistical_types::claims::ObserverRef,
};
use {
    medulla_decision_stats_records::decision::participant_card::card_digest,
    medulla_decision_stats_records::decision::participant_card::rank_card,
    medulla_decision_stats_records::decision::participant_card::token_estimate,
    medulla_decision_stats_records::decision::participant_card::CardBudget,
    medulla_decision_stats_records::decision::participant_card::CardInput,
};

#[cfg(feature = "statechart")]
fn budget(now_ms: i64) -> CardBudget {
    CardBudget {
        tokens: u32::try_from(default_u64("participant.card_token_budget")).unwrap_or(u32::MAX),
        half_life_ms: default_u64("participant.card_recency_half_life"),
        now_ms,
    }
}

/// The card inputs of the claims that state a sentence.
#[cfg(feature = "statechart")]
fn inputs(claims: &[(ClaimRecord, i64)]) -> Vec<CardInput> {
    claims
        .iter()
        .filter_map(|(record, updated_ms)| {
            let text = record.perspective.as_ref()?.statement.clone()?;
            Some(CardInput {
                claim_id: record.claim_id.clone(),
                token_estimate: token_estimate(&text),
                text,
                log_odds_micro: record.log_odds_micro,
                updated_ms: *updated_ms,
            })
        })
        .collect()
}

/// Recompute the card of `subject` as `observer` sees it and answer the operations that store it
/// and announce it (`None` when the stored card has the same digest). The card is ranked over the
/// claims of that perspective that the observer's own sessions allow.
#[cfg(feature = "statechart")]
pub async fn plan_card_refresh(
    state: &SharedState,
    graph: &str,
    verified: &VerifiedRequestContext,
    subject: &ParticipantId,
    observer: &ObserverRef,
    at_ms: u64,
) -> Result<Option<Vec<serde_json::Value>>, String> {
    use medulla_h_graph::graph_ops::participant_card::refresh_card;
    use medulla_h_graph_gateway::graph_ops::participant_surface::sessions_of_participant;
    let tenant = verified.tenant();
    let Some(core) = super::claims::participants_of(state, graph).await else {
        return Ok(None);
    };
    let sessions = match observer {
        ObserverRef::Participant(slug) => sessions_of_participant(&core, tenant, slug),
        ObserverRef::System => sessions_of_participant(&core, tenant, subject),
    };
    let reader = medulla_epistemic::claims::ReaderView::of(observer.clone(), sessions);
    let persist_dir = { state.read().await.persist_dir.clone() };
    let claims = super::claims::served::ClaimStore::open(
        persist_dir.as_deref(),
        tenant,
        &verified.principal_persistence_id(),
    )?
    .perspective_claims(subject, observer, &reader)?;
    let now_ms = i64::try_from(at_ms).unwrap_or(i64::MAX);
    let ranked = rank_card(&inputs(&claims), budget(now_ms));
    let card = ParticipantCard {
        subject: subject.clone(),
        observer: observer.clone(),
        token_estimate: ranked.iter().map(|claim| token_estimate(&claim.text)).sum(),
        digest: card_digest(&ranked),
        claims: ranked,
        computed_at_ms: now_ms,
    };
    refresh_card(&core, tenant, &card, at_ms)
}

/// Without the claim store there are no claims to rank and no card to refresh.
#[cfg(not(feature = "statechart"))]
pub async fn plan_card_refresh(
    _state: &SharedState,
    _graph: &str,
    _verified: &VerifiedRequestContext,
    _subject: &ParticipantId,
    _observer: &ObserverRef,
    _at_ms: u64,
) -> Result<Option<Vec<serde_json::Value>>, String> {
    Ok(None)
}

/// Serve `ParticipantRead` `Card`: the stored card of `subject` as `observer` sees it, with the
/// claims the caller's own view does not allow left out. A subject or observer that is not a
/// stored participant of the tenant is `PARTICIPANT_NOT_VISIBLE`, a caller that does not act as the
/// observer is `OBSERVER_NOT_PERMITTED`, and a participant with no stored card is `CARD_ABSENT`.
pub async fn handle_participant_card(
    state: &SharedState,
    req_id: u64,
    graph: &str,
    verified: &VerifiedRequestContext,
    subject: ParticipantId,
    observer: ObserverRef,
) -> Response {
    #[cfg(feature = "statechart")]
    {
        match read_card(state, graph, verified, &subject, &observer)
            .await
            .and_then(ResultPayload::of::<ParticipantReadCard>)
        {
            Ok(payload) => Response::ok(req_id, payload),
            Err(error) => Response::err(req_id, error),
        }
    }
    #[cfg(not(feature = "statechart"))]
    {
        let _ = (state, graph, verified, subject, observer);
        Response::err(
            req_id,
            "ParticipantRead Card requires the `statechart` feature",
        )
    }
}

#[cfg(feature = "statechart")]
async fn read_card(
    state: &SharedState,
    graph: &str,
    verified: &VerifiedRequestContext,
    subject: &ParticipantId,
    observer: &ObserverRef,
) -> Result<ParticipantCard, String> {
    use medulla_h_graph::graph_ops::participant_card::read_card;
    use {
        medulla_h_graph_gateway::graph_ops::participant_surface::participant_exists,
        medulla_h_graph_gateway::graph_ops::participant_surface::participant_unavailable,
        medulla_h_graph_gateway::graph_ops::participant_surface::principal_view,
    };
    let tenant = verified.tenant();
    let core = super::claims::participants_of(state, graph).await;
    let exists = |slug: &ParticipantId| {
        core.as_ref()
            .is_some_and(|core| participant_exists(core, tenant, slug))
    };
    if !exists(subject) {
        return Err(participant_unavailable(subject.as_str()));
    }
    if let ObserverRef::Participant(slug) = observer {
        if !exists(slug) {
            return Err(participant_unavailable(slug.as_str()));
        }
    }
    let core = core.ok_or_else(|| participant_unavailable(subject.as_str()))?;
    let view = principal_view(&core, tenant, verified.agent_id(), Some(observer))?;
    let Some(mut card) = read_card(&core, tenant, subject, observer) else {
        return Err("CARD_ABSENT: no card is stored for this subject and observer".to_string());
    };
    let reader = medulla_epistemic::claims::ReaderView::of(view.observer, view.sessions);
    let persist_dir = { state.read().await.persist_dir.clone() };
    let allowed: std::collections::BTreeSet<String> = super::claims::served::ClaimStore::open(
        persist_dir.as_deref(),
        tenant,
        &verified.principal_persistence_id(),
    )?
    .perspective_claims(subject, observer, &reader)?
    .into_iter()
    .map(|(record, _)| record.claim_id)
    .collect();
    card.claims
        .retain(|claim| allowed.contains(&claim.claim_id));
    Ok(card)
}
