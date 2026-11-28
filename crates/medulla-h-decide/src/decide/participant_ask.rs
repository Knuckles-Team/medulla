//! `Method::ParticipantRead` `Ask`: a question about one participant (MEDULLA-DECISION-ENGINE-R655.2).
//!
//! The claims live in the statechart store, which only this crate reads, so the question is
//! answered here and the graph-side reads of `ParticipantRead` stay in `medulla-h-graph`. The
//! handler checks that the subject and the observer are stored participants of the caller's tenant
//! (`PARTICIPANT_NOT_VISIBLE`, the envelope an absent participant gets) and that the caller may act
//! as the observer (`OBSERVER_NOT_PERMITTED`), narrows the sessions by the memory scope, lists the
//! claims the reader may see, grounds the question in their statements and cites each sentence. A
//! question nothing grounds is an abstention, `NO_GROUNDED_CLAIMS`. Nothing is written.

use super::SharedState;
use medulla_decision_stats_records::decision::participant_card::confidence_ppm;
use medulla_decision_types::decision::knob_families::participant::default_u64;
use medulla_decision_types::decision::record::outcome::{AbstainReason, RefusalSurface};
use medulla_decision_types::participant_memory::{ParticipantAnswer, ParticipantQuestion};
use medulla_protocol_types::protocol::{Response, ResultPayload};
use medulla_protocol_types::result_contract::coordination::ParticipantReadAsk;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_types::refusal_code::RefusalCode;
use {
    medulla_decision_statistical_types::claims::ClaimRecord,
    medulla_decision_statistical_types::claims::EvidenceChannel,
};
use {
    medulla_decision_stats_records::decision::participant_ask::cite,
    medulla_decision_stats_records::decision::participant_ask::ground,
    medulla_decision_stats_records::decision::participant_ask::AskClaim,
};

/// The prefix a worker may put before a turn id in a piece of evidence's source digest.
const TURN_PREFIX: &str = "turn:";

/// Serve one question.
pub async fn handle_participant_ask(
    state: &SharedState,
    req_id: u64,
    graph: &str,
    verified: &VerifiedRequestContext,
    question: ParticipantQuestion,
) -> Response {
    #[cfg(feature = "statechart")]
    {
        match ask(state, graph, verified, &question)
            .await
            .and_then(ResultPayload::of::<ParticipantReadAsk>)
        {
            Ok(payload) => Response::ok(req_id, payload),
            Err(error) => Response::err(req_id, error),
        }
    }
    #[cfg(not(feature = "statechart"))]
    {
        let _ = (state, graph, verified, question);
        Response::err(
            req_id,
            "ParticipantRead Ask requires the `statechart` feature",
        )
    }
}

/// The turn ids a claim's mined evidence cites, in evidence order.
fn turn_ids(record: &ClaimRecord) -> Vec<String> {
    record
        .evidence
        .iter()
        .filter(|evidence| evidence.channel == EvidenceChannel::Mined)
        .map(|evidence| {
            evidence
                .source_digest
                .strip_prefix(TURN_PREFIX)
                .unwrap_or(&evidence.source_digest)
                .to_string()
        })
        .collect()
}

#[cfg(feature = "statechart")]
async fn ask(
    state: &SharedState,
    graph: &str,
    verified: &VerifiedRequestContext,
    question: &ParticipantQuestion,
) -> Result<ParticipantAnswer, String> {
    use medulla_decision_statistical_types::claims::ObserverRef;
    use {
        medulla_h_graph_gateway::graph_ops::participant_surface::participant_exists,
        medulla_h_graph_gateway::graph_ops::participant_surface::participant_unavailable,
        medulla_h_graph_gateway::graph_ops::participant_surface::principal_view,
        medulla_h_graph_gateway::graph_ops::participant_surface::resolve_scope_ref,
    };
    let max_chars =
        usize::try_from(default_u64("participant.ask_question_max_chars")).unwrap_or(usize::MAX);
    if question.question.trim().is_empty() || question.question.chars().count() > max_chars {
        return Err(
            "INVALID_ARGUMENT: the question is empty or over participant.ask_question_max_chars"
                .to_string(),
        );
    }
    if question.as_of_ms.is_some() {
        return Err(
            "INVALID_ARGUMENT: a claim carries no creation time, so as_of_ms is not served"
                .to_string(),
        );
    }
    let tenant = verified.tenant();
    let core = super::claims::participants_of(state, graph).await;
    let exists = |slug: &_| {
        core.as_ref()
            .is_some_and(|core| participant_exists(core, tenant, slug))
    };
    if !exists(&question.subject) {
        return Err(participant_unavailable(question.subject.as_str()));
    }
    if let ObserverRef::Participant(observer) = &question.observer {
        if !exists(observer) {
            return Err(participant_unavailable(observer.as_str()));
        }
    }
    let core = core.ok_or_else(|| participant_unavailable(question.subject.as_str()))?;
    let view = principal_view(&core, tenant, verified.agent_id(), Some(&question.observer))?;
    let sessions = match &question.memory_scope {
        Some(reference) => resolve_scope_ref(
            &core,
            tenant,
            verified.agent_id(),
            reference,
            &view.sessions,
        )?,
        None => view.sessions.clone(),
    };
    let reader = medulla_epistemic::claims::ReaderView::of(view.observer, sessions);
    let persist_dir = { state.read().await.persist_dir.clone() };
    let claims = super::claims::served::ClaimStore::open(
        persist_dir.as_deref(),
        tenant,
        &verified.principal_persistence_id(),
    )?
    .perspective_claims(&question.subject, &question.observer, &reader)?;
    let held: Vec<AskClaim> = claims
        .iter()
        .filter_map(|(record, _)| {
            let statement = record.perspective.as_ref()?.statement.clone()?;
            Some(AskClaim {
                claim_id: record.claim_id.clone(),
                statement,
                confidence_ppm: confidence_ppm(record.log_odds_micro),
                turn_ids: turn_ids(record),
            })
        })
        .collect();
    let max_claims =
        usize::try_from(default_u64("participant.ask_max_claims")).unwrap_or(usize::MAX);
    let grounded = ground(&question.question, &held, max_claims);
    if grounded.is_empty() {
        return Ok(ParticipantAnswer {
            text: String::new(),
            citations: Vec::new(),
            abstained: Some(AbstainReason::Surface {
                surface: RefusalSurface::ParticipantRead,
                code: RefusalCode::new("NO_GROUNDED_CLAIMS"),
                detail: "no claim the reader may see answers the question".to_string(),
            }),
        });
    }
    let (text, citations) = cite(&grounded);
    Ok(ParticipantAnswer {
        text,
        citations,
        abstained: None,
    })
}
