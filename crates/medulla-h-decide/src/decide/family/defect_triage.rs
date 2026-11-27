//! The `defect_triage` question family (MEDULLA-SDLC-R004.3 to R004.5).
//!
//! The serving path reads the defect, its frames, symbols, blame and the open defects once,
//! before the executor, and hands them over in `ExecutionContext.defect_triage`; this family
//! never reads the graph. It ranks the suspects, picks the owner and a duplicate, and seals
//! the triage as the typed answer. A triage that abstains still seals: the outcome stays
//! advisory and `abstain_reason` is set inside the answer. `Decide` writes nothing; the
//! `TriageDefect` handler commits the record and the router writes the `affects` edges.

use medulla_decision_outcome_types::decision::defect_triage::{
    cap_frames, decide_triage, find_duplicate, pick_owner, rank_suspects, SuspectInput,
    TriageEvidence, TriageKnobs, DEFECT_CLOSED, DEFECT_UNKNOWN, KNOBS,
};
use medulla_decision_types::decision::defect_triage::TriageAbstainReason;
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::outcome::FamilyAnswer;
use medulla_decision_types::decision::statistical::{
    DecideRequest, StatisticalOutcome, TypedValue,
};
use medulla_decision_types::decision::{DecisionPolicy, PremiseClass, PremiseRef, ResolutionKind};
use medulla_graph_core::graph::GraphView;
use medulla_source_types::lifecycle::DefectStatus;
use medulla_types::evidence_class::EvidenceClass;

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyExecuted, FamilyPremise, FamilyPremiseSource,
};

use super::support::{advisory, invalid, param, text_param};
use super::DecisionFamily;

/// Most owners a triage request names.
const MAX_PATH_OWNERS: usize = 64;

/// The family of the `defect_triage` question kind.
pub struct DefectTriageFamily;

fn refused(
    code: &medulla_types::refusal_code::RefusalCode,
    detail: String,
) -> Result<Executed, String> {
    Err(format!("{}: {detail}", code.as_str()))
}

fn observed(fact: String, defect: &str) -> FamilyPremise {
    FamilyPremise {
        subject: "defect_triage".into(),
        fact,
        class: PremiseClass::Observation,
        source: FamilyPremiseSource::Observation(defect.to_string()),
    }
}

fn int_param(request: &DecideRequest, name: &str) -> Result<Option<u64>, String> {
    match param(request, name)? {
        None => Ok(None),
        Some(TypedValue::Int(value)) => u64::try_from(*value)
            .map(Some)
            .map_err(|_| invalid(format!("parameter {name} must not be negative"))),
        Some(_) => Err(invalid(format!("parameter {name} is not an integer"))),
    }
}

fn path_owners(request: &DecideRequest) -> Result<Vec<String>, String> {
    match param(request, "path_owners")? {
        None => Ok(Vec::new()),
        Some(TypedValue::IriList(list)) if list.as_slice().len() <= MAX_PATH_OWNERS => {
            Ok(list.as_slice().to_vec())
        }
        Some(TypedValue::IriList(_)) => Err(invalid(format!(
            "path_owners names more than {MAX_PATH_OWNERS} principals"
        ))),
        Some(_) => Err(invalid("parameter path_owners is not an IRI list")),
    }
}

impl DecisionFamily for DefectTriageFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        match options {
            [only]
                if only.classification.as_slice().is_empty()
                    && only.numbers.as_slice().is_empty()
                    && only.texts.as_slice().is_empty() =>
            {
                Ok(())
            }
            _ => Err(invalid(
                "a defect_triage question declares exactly one option, the defect id, and no facts",
            )),
        }
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        text_param(request, "graph")?
            .filter(|graph| !graph.trim().is_empty())
            .ok_or_else(|| invalid("a defect_triage question requires the graph param"))?;
        path_owners(request)?;
        int_param(request, "as_of_ms")?;
        text_param(request, "culprit")?;
        Ok(())
    }

    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(triage(ctx, request, policy))
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

    fn evidence_class(&self, outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        matches!(outcome, StatisticalOutcome::Advisory { .. }).then_some(EvidenceClass::Observation)
    }
}

fn triage(
    ctx: &ExecutionContext,
    request: &DecideRequest,
    policy: &DecisionPolicy,
) -> Result<Executed, String> {
    let facts = ctx
        .defect_triage
        .ok_or_else(|| invalid("a defect_triage question reads its facts before the executor"))?;
    // R004.4: raised here, because `validate_candidates` has neither the request nor the facts.
    let Some(defect) = facts.defect.as_ref() else {
        return refused(
            &DEFECT_UNKNOWN,
            "the tenant graph holds no such defect".to_string(),
        );
    };
    if matches!(
        defect.status,
        DefectStatus::Closed | DefectStatus::Rejected | DefectStatus::Verified
    ) {
        return refused(
            &DEFECT_CLOSED,
            format!("defect {} is already {:?}", defect.id, defect.status),
        );
    }
    let knobs = TriageKnobs::from_policy(policy);
    let culprit = text_param(request, "culprit")?.filter(|culprit| !culprit.trim().is_empty());
    let as_of = match int_param(request, "as_of_ms")? {
        Some(at) => at,
        None => defect
            .symptoms
            .iter()
            .map(|symptom| symptom.observed_at)
            .max()
            .unwrap_or(defect.created_at),
    };
    let owners = path_owners(request)?;
    let kept = cap_frames(&facts.frames, &knobs);
    let inputs: Vec<SuspectInput> = kept
        .iter()
        .filter_map(|frame| frame.suspect.clone())
        .collect();
    let suspects = rank_suspects(&inputs, culprit, as_of, &knobs);
    let mut recent_authors: Vec<String> = Vec::new();
    for symbol in &suspects {
        let author = inputs
            .iter()
            .find(|input| &input.symbol_id == symbol)
            .and_then(|input| facts.authors.get(&input.blame_changeset));
        if let Some(author) = author {
            if !recent_authors.contains(author) {
                recent_authors.push(author.clone());
            }
        }
    }
    let owner = pick_owner(&owners, &recent_authors);
    let duplicate = find_duplicate(&defect.repo, &suspects, &facts.open, &knobs);
    let mut evidence_refs: Vec<String> = kept
        .iter()
        .map(|frame| format!("symptom:{}", frame.digest))
        .collect();
    if let Some(culprit) = culprit {
        evidence_refs.push(format!("culprit:{culprit}"));
    }
    let decision = decide_triage(
        &defect.id,
        defect.severity,
        u32::try_from(defect.symptoms.len()).unwrap_or(u32::MAX),
        TriageEvidence {
            suspects: suspects.clone(),
            owner,
            duplicate_of: duplicate,
            evidence_refs,
        },
        &knobs,
    );

    let mut premises = vec![observed("triage.defect".to_string(), &defect.id)];
    for frame in &kept {
        premises.push(observed(
            format!("triage.symptom_digest:{}", frame.digest),
            &defect.id,
        ));
    }
    for symbol in &suspects {
        if let Some(input) = inputs.iter().find(|input| &input.symbol_id == symbol) {
            premises.push(observed(
                format!(
                    "triage.blame:{}={}@{}",
                    input.symbol_id, input.blame_changeset, input.blame_at
                ),
                &defect.id,
            ));
        }
    }
    if let Some(culprit) = culprit {
        premises.push(observed(format!("triage.culprit:{culprit}"), &defect.id));
    }
    if let Some(reason) = decision.abstain_reason {
        let code = match reason {
            TriageAbstainReason::NoSymbolMatch => "TRIAGE_NO_SYMBOL_MATCH",
            TriageAbstainReason::NoOwner => "TRIAGE_NO_OWNER",
            TriageAbstainReason::InsufficientEvidence => "TRIAGE_INSUFFICIENT_EVIDENCE",
        };
        premises.push(observed(format!("triage.abstain:{code}"), &defect.id));
    }
    for (index, spec) in KNOBS.iter().enumerate() {
        premises.push(FamilyPremise {
            subject: "policy".into(),
            fact: format!("knob:{}={}", spec.name, policy.knob(&KNOBS[index]).render()),
            class: PremiseClass::Definition,
            source: FamilyPremiseSource::Policy,
        });
    }
    let mut executed = advisory(vec![defect.id.clone()])?;
    executed.family = Some(FamilyExecuted {
        answer: Some(FamilyAnswer::DefectTriage(Box::new(decision))),
        premises: Vec::new(),
        resolution: Some(ResolutionKind::Entailment),
        evidence: Some(EvidenceClass::Observation),
    });
    executed.family_premises = premises;
    Ok(executed)
}
