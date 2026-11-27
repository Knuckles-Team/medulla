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

use medulla_decision_outcome_types::decision::promotion::{ChampionPointer, PromotionVerdict};
use medulla_decision_stats::decision::promotion::{
    regression_rollback, subject_gate, PromotionGate,
};
use medulla_decision_stats::decision::promotion_tier::tier_gate;
use medulla_decision_stats::decision::skill_eval::{canary_verdict, check_traffic};
use medulla_decision_types::decision::codes::{
    ABLATION_NOISE_BAND_MISSING, ABLATION_WITHIN_NOISE, CANARY_NO_CHAMPION,
    CANARY_TRAFFIC_OUT_OF_BOUNDS,
};
use medulla_decision_types::decision::held_out::{HeldOutEvidence, ReceiptKind, RevisionSubject};
use medulla_decision_types::decision::promotion_answer::{PromotionAnswer, PromotionAnswerVerdict};
use medulla_decision_types::decision::promotion_tier::{
    SkillTier, TierContext, TierKnobs, TierReview, PARAM_TIER_REVIEWER, PLATFORM_TEAM,
    PREMISE_ALWAYS_ON, PREMISE_TARGET,
};
use medulla_decision_types::decision::record::outcome::DecisionRefusal;
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::outcome::FamilyAnswer;
use medulla_decision_types::decision::statistical::{
    DecideRequest, QuestionKind, StatisticalOutcome,
};
use medulla_decision_types::decision::{DecisionPolicy, PremiseClass, PremiseRef, ResolutionKind};
use medulla_decision_types::skill_eval::{
    CompileState, ModelCompile, CANARY_TRAFFIC_MAX_PPM, CANARY_TRAFFIC_MIN_PPM,
};
use medulla_graph_core::graph::GraphView;
use medulla_types::contract::OpaqueId;
use medulla_types::evidence_class::EvidenceClass;
use medulla_types::refusal_code::RefusalCode;

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyExecuted, FamilyPremise, FamilyPremiseSource,
};
use crate::decide::stat_promotion::{
    held_out_evidence, EvidenceError, EvidenceSources, PREMISE_PREFIX,
};
use crate::decide::SharedState;
use medulla_persistence_records::decision_jobs::read_pointer;

use super::support::{
    abstained, advisory, advisory_scored, invalid, json_param, text_param, unsigned_param,
};
use super::DecisionFamily;

const PARAM_SUBJECT: &str = "subject";
const PARAM_RECEIPT: &str = "receipt_digest";
const PARAM_MODE: &str = "mode";
const PARAM_GRAPH: &str = "graph";
const PARAM_TRAFFIC: &str = "traffic_ppm";
const PARAM_REGRESSION: &str = "regression_receipt";
const PARAM_TIER_REVIEW: &str = "tier_review";

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
    Canary,
}

struct Ask {
    subject: RevisionSubject,
    receipt: String,
    mode: Mode,
    option: String,
    /// The canary traffic share the caller asks for (`canary` mode).
    traffic: Option<u64>,
    /// A later receipt of the champion, read as the regression verdict (`canary` mode).
    regression: Option<String>,
    /// The review receipt of the tier a skill is promoted to (R714.2).
    tier_review: Option<TierReview>,
    /// The verified reviewer the engine bound to a tiered promotion of a person; `None` for a
    /// service, whose review names its approver (MEDULLA-IDENTITY-R739).
    reviewer: Option<String>,
}

fn parse(request: &DecideRequest) -> Result<Ask, String> {
    let subject: RevisionSubject = json_param(request, PARAM_SUBJECT)?;
    let receipt = text_param(request, PARAM_RECEIPT)?
        .ok_or_else(|| invalid(format!("parameter {PARAM_RECEIPT} is required")))?
        .to_owned();
    let mode = match text_param(request, PARAM_MODE)? {
        Some("promote") => Mode::Promote,
        Some("review") => Mode::Review,
        Some("canary") => Mode::Canary,
        _ => return Err(invalid("parameter mode is `promote`, `review` or `canary`")),
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
        traffic: unsigned_param(request, PARAM_TRAFFIC)?,
        regression: text_param(request, PARAM_REGRESSION)?.map(str::to_owned),
        tier_review: match request
            .params
            .iter()
            .any(|param| param.name == PARAM_TIER_REVIEW)
        {
            true => Some(json_param(request, PARAM_TIER_REVIEW)?),
            false => None,
        },
        reviewer: text_param(request, PARAM_TIER_REVIEWER)?.map(str::to_owned),
    })
}

fn premise(fact: String, receipt: &str) -> FamilyPremise {
    // The rollout premises keep their own `rollout.` namespace
    // (MEDULLA-DECISION-ENGINE-R654.5).
    // The ablation, canary and compile premises keep their own names (MEDULLA-DECISION-ENGINE-R712.5).
    let own = ["rollout.", "ablation:", "canary:", "compile:", "tier:"];
    let fact = if own.iter().any(|prefix| fact.starts_with(prefix)) {
        fact
    } else {
        format!("{PREMISE_PREFIX}{fact}")
    };
    FamilyPremise {
        subject: "promotion".into(),
        fact,
        class: PremiseClass::Observation,
        source: FamilyPremiseSource::Observation(receipt.to_owned()),
    }
}

fn opaque(value: &str) -> Result<OpaqueId, String> {
    OpaqueId::new(value.to_owned())
        .map_err(|_| invalid(format!("`{value}` is not a revision reference")))
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
    champion: Option<&str>,
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
    executed.family = Some(answer_of(PromotionAnswer {
        verdict: answer_verdict(verdict)?,
        champion_ref: champion.map(str::to_owned),
        challenger_ref: ask.option.clone(),
        reason: None,
    }));
    Ok(executed)
}

/// The typed verdict of the gate's own word for it (MEDULLA-DECISION-ENGINE-R805.14).
fn answer_verdict(verdict: &str) -> Result<PromotionAnswerVerdict, String> {
    match verdict {
        "promote" => Ok(PromotionAnswerVerdict::Promote),
        "hold" => Ok(PromotionAnswerVerdict::Hold),
        "rollback" => Ok(PromotionAnswerVerdict::Rollback),
        "ramp" => Ok(PromotionAnswerVerdict::Ramp),
        other => Err(invalid(format!("`{other}` is not a promotion verdict"))),
    }
}

/// The typed answer a promotion question seals in its record.
fn answer_of(answer: PromotionAnswer) -> FamilyExecuted {
    FamilyExecuted {
        answer: Some(FamilyAnswer::Promotion(Box::new(answer))),
        premises: Vec::new(),
        resolution: None,
        evidence: None,
    }
}

/// An abstention that also seals the typed answer: the verdict `abstain` with the refusal code
/// as its reason and no champion.
fn refuse(ask: &Ask, refusal: &PromotionRefusal) -> Executed {
    let mut executed = abstained(QuestionKind::Promotion, refusal);
    executed.family = Some(answer_of(PromotionAnswer {
        verdict: PromotionAnswerVerdict::Abstain,
        champion_ref: None,
        challenger_ref: ask.option.clone(),
        reason: Some(refusal.0.as_str().to_owned()),
    }));
    executed
}

fn evidence_facts(evidence: &HeldOutEvidence) -> Vec<String> {
    let mut facts = vec![
        format!("delta_ppm={}", evidence.delta_ppm),
        format!("case_count={}", evidence.case_count),
    ];
    // A rollout set is cited by its pair count, the lower bootstrap bound of its
    // mean difference and its own digest (MEDULLA-DECISION-ENGINE-R654.5).
    if evidence.receipt_kind == ReceiptKind::RolloutSet {
        facts.push(format!("rollout.pairs={}", evidence.case_count));
        facts.push(format!("rollout.delta_lower_ppm={}", evidence.delta_ppm));
        facts.push(format!("rollout.set_digest={}", evidence.receipt_digest));
    }
    facts
}

/// Ask the tier gate whether `skill_ref` may take `tier`: the review the caller names, the
/// latest suite report, the always-on count and the approver's membership in the graph decide.
/// The premises it seals are `tier:target=<tier>` and `tier:always_on=<n>/<max>`.
fn tier_verdict(
    ctx: &ExecutionContext,
    graph: Option<&GraphView>,
    ask: &Ask,
    skill_ref: &str,
    tier: SkillTier,
) -> Result<Result<Vec<String>, PromotionRefusal>, String> {
    use crate::decide::stat_skill_eval::read_report;
    use crate::decide::stat_skill_tier::{
        always_on_set, is_author, is_member, read_tier, read_tiers, teams_of,
    };
    let knobs = medulla_server_api::knob_config::knobs();
    let tiers = read_tiers(ctx.store, ctx.tenant_id)?;
    let always_on = always_on_set(&tiers)
        .into_iter()
        .filter(|held| held != skill_ref)
        .count() as u64;
    // The reviewer is the verified caller of a person (bound by `bind_reviewer`), else the
    // approver a service names. A person's team is decided by the graph: the platform team when
    // the reviewer is a member of it, else the team the review names when the reviewer is a
    // member, else the first team the reviewer belongs to; a reviewer in no team has none.
    let reviewer = ask.reviewer.clone().or_else(|| {
        ask.tier_review
            .as_ref()
            .map(|review| review.approver_principal.clone())
    });
    let (review, approver_in_team, reviewer_is_author) = match (&ask.tier_review, graph, reviewer) {
        (Some(review), Some(view), Some(reviewer)) => {
            let author = is_author(view, skill_ref, &reviewer);
            if ask.reviewer.is_some() {
                let teams = teams_of(view, &reviewer);
                let team = if teams.contains(PLATFORM_TEAM) {
                    PLATFORM_TEAM.to_owned()
                } else if teams.contains(&review.reviewer_team) {
                    review.reviewer_team.clone()
                } else {
                    teams.into_iter().next().unwrap_or_default()
                };
                let derived = TierReview {
                    reviewer_team: team.clone(),
                    approver_principal: reviewer,
                    ..review.clone()
                };
                (Some(derived), !team.is_empty(), author)
            } else {
                let member = is_member(view, &review.approver_principal, &review.reviewer_team);
                (Some(review.clone()), member, author)
            }
        }
        (review, _, _) => (review.clone(), false, false),
    };
    let context = TierContext {
        current: read_tier(ctx.store, ctx.tenant_id, skill_ref)?,
        approver_in_team,
        reviewer_is_author,
    };
    let report = read_report(ctx.store, ctx.tenant_id, skill_ref)?;
    let gate_knobs = TierKnobs {
        always_on_max: knobs.skills_always_on_max,
        min_cases: knobs.eval_min_cases,
    };
    match tier_gate(
        tier,
        review.as_ref(),
        report.as_ref(),
        always_on,
        &context,
        &gate_knobs,
    ) {
        Ok(()) => Ok(Ok(vec![
            format!("{PREMISE_TARGET}{}", tier.as_str()),
            format!(
                "{PREMISE_ALWAYS_ON}{always_on}/{}",
                gate_knobs.always_on_max
            ),
        ])),
        Err(code) => Ok(Err(PromotionRefusal(
            code,
            format!(
                "the review does not qualify {skill_ref} for tier {}",
                tier.as_str()
            ),
        ))),
    }
}

fn promote(
    ctx: &ExecutionContext,
    graph: Option<&GraphView>,
    policy: &DecisionPolicy,
    ask: &Ask,
) -> Result<Executed, String> {
    let evidence = match resolve(ctx, graph, policy, &ask.subject, &ask.receipt)? {
        Ok(evidence) => evidence,
        Err(refusal) => return Ok(refuse(ask, &refusal)),
    };
    if evidence.challenger_ref != ask.option {
        return Err(invalid(
            "the declared option is not the challenger the receipt evaluates",
        ));
    }
    // A skill never wins on a delta inside the noise band of its identical runs.
    if let RevisionSubject::Skill { skill_ref, .. } = &ask.subject {
        if let Some(noise) = crate::decide::stat_skill_ablation::read_noise_band(
            ctx.store,
            ctx.tenant_id,
            skill_ref,
        )? {
            if evidence.delta_ppm.unsigned_abs() <= noise.band_ppm {
                return Ok(refuse(
                    ask,
                    &PromotionRefusal(
                        ABLATION_WITHIN_NOISE,
                        format!(
                            "a delta of {} ppm is inside the noise band of {} ppm",
                            evidence.delta_ppm, noise.band_ppm
                        ),
                    ),
                ));
            }
        }
    }
    // A tiered skill promotion also needs its review receipt (MEDULLA-DECISION-ENGINE-R714.3).
    let mut tier_facts = Vec::new();
    if let RevisionSubject::Skill {
        skill_ref,
        tier: Some(tier),
    } = &ask.subject
    {
        match tier_verdict(ctx, graph, ask, skill_ref, *tier)? {
            Ok(facts) => tier_facts = facts,
            Err(refusal) => return Ok(refuse(ask, &refusal)),
        }
    }
    // The champion active now is pinned in the record: a later move of the
    // pointer is refused if another revision has taken the seat since.
    let (_, state) = read_pointer(ctx.store, ctx.tenant_id, &ask.subject.pointer_key())?;
    let mut facts = evidence_facts(&evidence);
    facts.extend(tier_facts);
    facts.push(format!(
        "champion_at_decision={}",
        state.target().unwrap_or("none")
    ));
    match subject_gate(&ask.subject, &evidence, policy) {
        PromotionGate::Promote => decided(ask, "promote", facts, Some(&ask.option), state.target()),
        PromotionGate::Hold { reason } => {
            facts.push(format!("hold_reason={reason}"));
            decided(ask, "hold", facts, None, state.target())
        }
        PromotionGate::Abstain(code) => Ok(refuse(
            ask,
            &PromotionRefusal(
                code,
                "the held-out evidence cannot decide a promotion".to_owned(),
            ),
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
        return decided(
            ask,
            "hold",
            vec!["hold_reason=no_champion".to_owned()],
            None,
            None,
        );
    };
    let promoted_by = active.receipt_digest.clone().unwrap_or_default();
    let promoted_digest =
        crate::decide::stat_promotion::promoted_receipt(ctx.store, ctx.tenant_id, &promoted_by)?;
    let latest = match resolve(ctx, graph, policy, &ask.subject, &ask.receipt)? {
        Ok(evidence) => evidence,
        Err(refusal) => return Ok(refuse(ask, &refusal)),
    };
    let promoted = match promoted_digest {
        Some(digest) => match resolve(ctx, graph, policy, &ask.subject, &digest)? {
            Ok(evidence) => evidence,
            Err(refusal) => return Ok(refuse(ask, &refusal)),
        },
        None => {
            return decided(
                ask,
                "hold",
                vec!["hold_reason=promotion_evidence_missing".to_owned()],
                None,
                active.target.as_deref(),
            )
        }
    };
    let target = active.target.clone().unwrap_or_default();
    let pointer = ChampionPointer {
        subject: ask.subject.clone(),
        revision_ref: opaque(&target)?,
        parent_ref: match state
            .stack
            .as_slice()
            .last()
            .and_then(|event| event.target.as_deref())
        {
            Some(parent) => Some(opaque(parent)?),
            None => None,
        },
        promoted_by_decision: opaque(&format!("decision:{promoted_by}"))?,
        canary: None,
    };
    let mut facts = evidence_facts(&latest);
    match regression_rollback(&pointer, &promoted, &latest, policy) {
        PromotionVerdict::Rollback(rollback) => {
            facts.push(format!("rollback_to={}", rollback.to_ref.as_str()));
            let chosen = format!("rollback:{}", rollback.to_ref.as_str());
            decided(ask, "rollback", facts, Some(&chosen), Some(target.as_str()))
        }
        PromotionVerdict::Hold { reason } => {
            facts.push(format!("hold_reason={reason}"));
            decided(ask, "hold", facts, None, Some(target.as_str()))
        }
        PromotionVerdict::Promote { .. } => {
            decided(ask, "hold", facts, None, Some(target.as_str()))
        }
    }
}

/// Whether a later receipt of the champion calls for a rollback (MEDULLA-DECISION-ENGINE-R805.2),
/// the regression verdict a canary reads. A champion with no promotion evidence on record has no
/// regression to show.
fn regressed(
    ctx: &ExecutionContext,
    graph: Option<&GraphView>,
    policy: &DecisionPolicy,
    ask: &Ask,
    latest_receipt: &str,
) -> Result<Result<bool, PromotionRefusal>, String> {
    let (_, state) = read_pointer(ctx.store, ctx.tenant_id, &ask.subject.pointer_key())?;
    let Some(active) = state.active.as_ref() else {
        return Ok(Ok(false));
    };
    let promoted_by = active.receipt_digest.clone().unwrap_or_default();
    let Some(promoted_digest) =
        crate::decide::stat_promotion::promoted_receipt(ctx.store, ctx.tenant_id, &promoted_by)?
    else {
        return Ok(Ok(false));
    };
    let latest = match resolve(ctx, graph, policy, &ask.subject, latest_receipt)? {
        Ok(evidence) => evidence,
        Err(refusal) => return Ok(Err(refusal)),
    };
    let promoted = match resolve(ctx, graph, policy, &ask.subject, &promoted_digest)? {
        Ok(evidence) => evidence,
        Err(refusal) => return Ok(Err(refusal)),
    };
    let pointer = ChampionPointer {
        subject: ask.subject.clone(),
        revision_ref: opaque(active.target.as_deref().unwrap_or_default())?,
        parent_ref: match state
            .stack
            .as_slice()
            .last()
            .and_then(|event| event.target.as_deref())
        {
            Some(parent) => Some(opaque(parent)?),
            None => None,
        },
        promoted_by_decision: opaque(&format!("decision:{promoted_by}"))?,
        canary: None,
    };
    Ok(Ok(matches!(
        regression_rollback(&pointer, &promoted, &latest, policy),
        PromotionVerdict::Rollback(_)
    )))
}

/// The `canary` mode (MEDULLA-DECISION-ENGINE-R712.3): the challenger rollouts, the noise band of
/// the skill and the regression verdict of its champion decide `ramp`, `hold` or `rollback`.
fn canary(
    ctx: &ExecutionContext,
    graph: Option<&GraphView>,
    policy: &DecisionPolicy,
    ask: &Ask,
) -> Result<Executed, String> {
    let RevisionSubject::Skill { skill_ref, .. } = &ask.subject else {
        return Err(invalid("a canary promotes a skill revision"));
    };
    let refused = |code, detail: &str| Ok(refuse(ask, &PromotionRefusal(code, detail.to_owned())));
    let (_, state) = read_pointer(ctx.store, ctx.tenant_id, &ask.subject.pointer_key())?;
    if state.active.is_none() {
        return refused(
            CANARY_NO_CHAMPION,
            "the skill has no champion to run a canary against",
        );
    }
    let knob_traffic = medulla_server_api::knob_config::knobs().canary_traffic_ppm;
    let traffic = match check_traffic(
        ask.traffic.unwrap_or(knob_traffic),
        CANARY_TRAFFIC_MIN_PPM,
        CANARY_TRAFFIC_MAX_PPM,
    ) {
        Ok(traffic) => traffic,
        Err(_) => {
            return refused(
                CANARY_TRAFFIC_OUT_OF_BOUNDS,
                "the traffic share is outside its bounds",
            )
        }
    };
    let Some(noise) =
        crate::decide::stat_skill_ablation::read_noise_band(ctx.store, ctx.tenant_id, skill_ref)?
    else {
        return refused(
            ABLATION_NOISE_BAND_MISSING,
            "no identical runs are recorded for the skill",
        );
    };
    let evidence = match resolve(ctx, graph, policy, &ask.subject, &ask.receipt)? {
        Ok(evidence) => evidence,
        Err(refusal) => return Ok(refuse(ask, &refusal)),
    };
    if evidence.challenger_ref != ask.option {
        return Err(invalid(
            "the declared option is not the challenger the receipt evaluates",
        ));
    }
    let regression = match ask.regression.as_deref() {
        Some(receipt) => match regressed(ctx, graph, policy, ask, receipt)? {
            Ok(regressed) => regressed,
            Err(refusal) => return Ok(refuse(ask, &refusal)),
        },
        None => false,
    };
    let verdict = canary_verdict(evidence.delta_ppm, noise.band_ppm, regression);
    let stale = stale_compiles(ctx, skill_ref)?;
    let mut facts = evidence_facts(&evidence);
    facts.push(format!("ablation:delta={}", evidence.delta_ppm));
    facts.push(format!("ablation:band={}", noise.band_ppm));
    facts.push(format!("canary:traffic={traffic}"));
    facts.push(format!("canary:verdict={}", verdict.as_str()));
    facts.push(format!("compile:stale={stale}"));
    let chosen = match verdict {
        medulla_decision_types::skill_eval::CanaryVerdict::Ramp => Some(ask.option.clone()),
        medulla_decision_types::skill_eval::CanaryVerdict::Hold => None,
        medulla_decision_types::skill_eval::CanaryVerdict::Rollback => {
            Some(format!("rollback:{}", ask.option))
        }
    };
    decided(
        ask,
        verdict.as_str(),
        facts,
        chosen.as_deref(),
        state.target(),
    )
}

/// How many compiled rows of `skill_ref` are stale.
fn stale_compiles(ctx: &ExecutionContext, skill_ref: &str) -> Result<usize, String> {
    use medulla_persistence_records::decision_jobs::decode_artifact;
    let mut stale = 0;
    for (_, bytes) in
        ctx.store
            .decision_artifacts_with_prefix(ctx.tenant_id, "model-compile:", 10_000)?
    {
        let row: ModelCompile = decode_artifact(&bytes, "model compile")?;
        if row.skill_ref == skill_ref && row.state == CompileState::Stale {
            stale += 1;
        }
    }
    Ok(stale)
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
            Mode::Canary => canary(ctx, graph_view, policy, &ask),
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
