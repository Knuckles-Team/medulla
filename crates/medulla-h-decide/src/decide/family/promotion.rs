//! The `promotion` question family (MEDULLA-DECISION-ENGINE-R805.3, R805.5).
//!
//! Params: `subject` (the JSON `RevisionSubject`), `receipt_digest` and `mode`
//! (`promote` or `review`); an optional `graph` names the graph the receipts
//! of a program or policy subject live in. The one declared option is the
//! challenger revision (`promote`) or the champion under review (`review`).
//!
//! `promote` resolves the receipt into held-out evidence and asks the gate
//! (`subject_gate`, which is `challenger_wins` for the base subjects): a
//! winner is chosen, a sound loser holds, evidence that cannot decide
//! abstains with its refusal code. `review` asks `regression_rollback` of the
//! evidence the champion won on and a later one. The answer is advisory:
//! only `DecisionLog` moves a pointer, and only on a committed `promote`
//! record (`stat_promotion::qualified_champion`).

use medulla_decision_outcome_types::decision::promotion::{
    ChampionPointer, HeldOutEvidence, PromotionVerdict, RevisionSubject,
};
use medulla_decision_stats::decision::promotion::{regression_rollback, subject_gate, PromotionGate};
use medulla_decision_types::decision::record::outcome::{DecisionRefusal, RefusalCode};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{DecideRequest, QuestionKind, StatisticalOutcome};
use medulla_decision_types::decision::{
    DecisionPolicy, EvidenceClass, PremiseClass, PremiseRef, ResolutionKind,
};
use medulla_graph_core::graph::GraphView;
use medulla_types::contract::OpaqueId;

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};
use crate::decide::stat_pointer::read_pointer;
use crate::decide::stat_promotion::{
    held_out_evidence, EvidenceError, EvidenceSources, PREMISE_PREFIX,
};
use crate::decide::SharedState;

use super::support::{abstained, advisory, advisory_scored, invalid, json_param, text_param};
use super::DecisionFamily;

const PARAM_SUBJECT: &str = "subject";
const PARAM_RECEIPT: &str = "receipt_digest";
const PARAM_MODE: &str = "mode";
const PARAM_GRAPH: &str = "graph";

/// The graph a program or policy receipt is read from, when the request names one.
pub(in crate::decide) async fn evidence_view(
    state: &SharedState,
    agent_id: &str,
    request: &DecideRequest,
) -> Result<Option<GraphView>, String> {
    match text_param(request, PARAM_GRAPH)? {
        Some(graph) => crate::decide::stat_support::filtered_view(state, agent_id, graph)
            .await
            .map(Some),
        None => Ok(None),
    }
}

/// The family of the `promotion` question kind.
pub struct PromotionFamily;

/// A refusal of the evidence the gate reads.
struct PromotionRefusal(RefusalCode, String);

impl DecisionRefusal for PromotionRefusal {
    fn code(&self) -> RefusalCode {
        self.0
    }
    fn detail(&self) -> String {
        self.1.clone()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Promote,
    Review,
}

struct Ask {
    subject: RevisionSubject,
    receipt: String,
    mode: Mode,
    option: String,
}

fn parse(request: &DecideRequest) -> Result<Ask, String> {
    let subject: RevisionSubject = json_param(request, PARAM_SUBJECT)?;
    let receipt = text_param(request, PARAM_RECEIPT)?
        .ok_or_else(|| invalid(format!("parameter {PARAM_RECEIPT} is required")))?
        .to_owned();
    let mode = match text_param(request, PARAM_MODE)? {
        Some("promote") => Mode::Promote,
        Some("review") => Mode::Review,
        _ => return Err(invalid("parameter mode is `promote` or `review`")),
    };
    let medulla_decision_types::decision::statistical::CandidateSource::Declared { options } =
        &request.candidates
    else {
        return Err(invalid("a promotion question declares its candidate"));
    };
    let [only] = options.as_slice() else {
        return Err(invalid("a promotion question declares exactly one option"));
    };
    Ok(Ask {
        subject,
        receipt,
        mode,
        option: only.option_id.clone(),
    })
}

fn premise(fact: String, receipt: &str) -> FamilyPremise {
    FamilyPremise {
        subject: "promotion".into(),
        fact: format!("{PREMISE_PREFIX}{fact}"),
        class: PremiseClass::Observation,
        source: FamilyPremiseSource::Observation(receipt.to_owned()),
    }
}

fn opaque(value: &str) -> Result<OpaqueId, String> {
    OpaqueId::new(value.to_owned()).map_err(|_| invalid(format!("`{value}` is not a revision reference")))
}

fn resolve(
    ctx: &ExecutionContext,
    graph: Option<&GraphView>,
    policy: &DecisionPolicy,
    subject: &RevisionSubject,
    receipt: &str,
) -> Result<Result<HeldOutEvidence, PromotionRefusal>, String> {
    let sources = EvidenceSources {
        store: ctx.store,
        tenant: ctx.tenant_id,
        graph,
        policy,
    };
    match held_out_evidence(&sources, subject, receipt) {
        Ok(evidence) => Ok(Ok(evidence)),
        Err(EvidenceError::Refused(code)) => Ok(Err(PromotionRefusal(
            code,
            "the receipt resolves to no held-out evidence".to_owned(),
        ))),
        Err(EvidenceError::Store(error)) => Err(error),
    }
}

fn decided(
    ask: &Ask,
    verdict: &str,
    extra: Vec<String>,
    chosen: Option<&str>,
) -> Result<Executed, String> {
    let mut executed = match chosen {
        Some(id) => advisory(vec![id.to_owned()])?,
        None => advisory_scored(vec![(ask.option.clone(), 0)])?,
    };
    let mut facts = vec![
        format!("subject={}", ask.subject.pointer_key()),
        format!("receipt_digest={}", ask.receipt),
        format!("verdict={verdict}"),
        format!("challenger={}", ask.option),
    ];
    facts.extend(extra);
    executed.family_premises = facts
        .into_iter()
        .map(|fact| premise(fact, &ask.receipt))
        .collect();
    Ok(executed)
}

fn evidence_facts(evidence: &HeldOutEvidence) -> Vec<String> {
    vec![
        format!("delta_ppm={}", evidence.delta_ppm),
        format!("case_count={}", evidence.case_count),
    ]
}

fn promote(
    ctx: &ExecutionContext,
    graph: Option<&GraphView>,
    policy: &DecisionPolicy,
    ask: &Ask,
) -> Result<Executed, String> {
    let evidence = match resolve(ctx, graph, policy, &ask.subject, &ask.receipt)? {
        Ok(evidence) => evidence,
        Err(refusal) => return Ok(abstained(QuestionKind::Promotion, &refusal)),
    };
    if evidence.challenger_ref != ask.option {
        return Err(invalid(
            "the declared option is not the challenger the receipt evaluates",
        ));
    }
    // The champion active now is pinned in the record: a later move of the
    // pointer is refused if another revision has taken the seat since.
    let (_, state) = read_pointer(ctx.store, ctx.tenant_id, &ask.subject.pointer_key())?;
    let mut facts = evidence_facts(&evidence);
    facts.push(format!(
        "champion_at_decision={}",
        state.target().unwrap_or("none")
    ));
    match subject_gate(&ask.subject, &evidence, policy) {
        PromotionGate::Promote => decided(ask, "promote", facts, Some(&ask.option)),
        PromotionGate::Hold { reason } => {
            facts.push(format!("hold_reason={reason}"));
            decided(ask, "hold", facts, None)
        }
        PromotionGate::Abstain(code) => Ok(abstained(
            QuestionKind::Promotion,
            &PromotionRefusal(code, "the held-out evidence cannot decide a promotion".to_owned()),
        )),
    }
}

fn review(
    ctx: &ExecutionContext,
    graph: Option<&GraphView>,
    policy: &DecisionPolicy,
    ask: &Ask,
) -> Result<Executed, String> {
    let (_, state) = read_pointer(ctx.store, ctx.tenant_id, &ask.subject.pointer_key())?;
    let Some(active) = state.active.as_ref() else {
        return decided(ask, "hold", vec!["hold_reason=no_champion".to_owned()], None);
    };
    let promoted_by = active.receipt_digest.clone().unwrap_or_default();
    let promoted_digest = crate::decide::stat_promotion::promoted_receipt(ctx.store, ctx.tenant_id, &promoted_by)?;
    let latest = match resolve(ctx, graph, policy, &ask.subject, &ask.receipt)? {
        Ok(evidence) => evidence,
        Err(refusal) => return Ok(abstained(QuestionKind::Promotion, &refusal)),
    };
    let promoted = match promoted_digest {
        Some(digest) => match resolve(ctx, graph, policy, &ask.subject, &digest)? {
            Ok(evidence) => evidence,
            Err(refusal) => return Ok(abstained(QuestionKind::Promotion, &refusal)),
        },
        None => return decided(ask, "hold", vec!["hold_reason=promotion_evidence_missing".to_owned()], None),
    };
    let target = active.target.clone().unwrap_or_default();
    let pointer = ChampionPointer {
        subject: ask.subject.clone(),
        revision_ref: opaque(&target)?,
        parent_ref: match state.stack.last().and_then(|event| event.target.as_deref()) {
            Some(parent) => Some(opaque(parent)?),
            None => None,
        },
        promoted_by_decision: opaque(&format!("decision:{promoted_by}"))?,
    };
    let mut facts = evidence_facts(&latest);
    match regression_rollback(&pointer, &promoted, &latest, policy) {
        PromotionVerdict::Rollback(rollback) => {
            facts.push(format!("rollback_to={}", rollback.to_ref.as_str()));
            let chosen = format!("rollback:{}", rollback.to_ref.as_str());
            decided(ask, "rollback", facts, Some(&chosen))
        }
        PromotionVerdict::Hold { reason } => {
            facts.push(format!("hold_reason={reason}"));
            decided(ask, "hold", facts, None)
        }
        PromotionVerdict::Promote { .. } => decided(ask, "hold", facts, None),
    }
}

impl DecisionFamily for PromotionFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        match options {
            [_] => Ok(()),
            _ => Err(invalid("a promotion question declares exactly one option")),
        }
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        parse(request).map(|_| ())
    }

    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(parse(request).and_then(|ask| match ask.mode {
            Mode::Promote => promote(ctx, graph_view, policy, &ask),
            Mode::Review => review(ctx, graph_view, policy, &ask),
        }))
    }

    fn premises(
        &self,
        _request: &DecideRequest,
        executed: &Executed,
        policy_digest: &str,
    ) -> Vec<PremiseRef> {
        executed
            .family_premises
            .iter()
            .map(|premise| premise.seal(policy_digest))
            .collect()
    }

    fn resolution_kind(&self, _outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        Some(ResolutionKind::Statistical)
    }

    fn evidence_class(&self, _outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        Some(EvidenceClass::Observation)
    }
}
