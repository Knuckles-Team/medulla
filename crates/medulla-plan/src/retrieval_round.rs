//! One served retrieval round (MEDULLA-FEDERATED-QUERY-R081.11, R081.3).
//!
//! `run_round` executes the request's plan over the snapshot the caller's
//! `PlanCtx` is bound to, then runs the decision parts the request carries, in a
//! fixed order. A part that is absent is skipped, and a request with no parts
//! returns the rows only. Nothing here writes. Row-level security and tenant scope
//! are applied by the handler before the `PlanCtx` is built, so the plan can only
//! see what the caller may read.

use medulla_exec::exec::{execute, PlanCtx};
use medulla_result_types::retrieval_round::{RoundRequest, RoundResult, RoundRow};
use medulla_result_types::retrieval_verdict::{RetrievalRefusal, RETRIEVAL_CONTRACT_INVALID};
use medulla_retrieval::retrieval_verdict::{judge_sufficiency, SufficiencyCall, SufficiencyRule};

/// The registered knobs a round reads (`retrieval.*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoundKnobs {
    /// `knob retrieval.min_evidence`, rows.
    pub min_evidence: u64,
    /// `knob retrieval.round_limit`, rounds. A caller may lower it, never raise it.
    pub round_limit: u32,
}

/// Why a round did not return a result.
#[derive(Debug, Clone, PartialEq)]
pub enum RoundError {
    /// The plan failed to execute.
    Plan(String),
    /// A part refused with a typed retrieval code.
    Refusal(RetrievalRefusal),
}

impl From<RetrievalRefusal> for RoundError {
    fn from(refusal: RetrievalRefusal) -> Self {
        Self::Refusal(refusal)
    }
}

fn contract_invalid(detail: &str) -> RoundError {
    RetrievalRefusal::new(RETRIEVAL_CONTRACT_INVALID, detail).into()
}

/// Check what the request must carry before any row is read.
fn validate(request: &RoundRequest) -> Result<(), RoundError> {
    if request.corpus_revision.trim().is_empty() || request.model_revision.trim().is_empty() {
        return Err(contract_invalid(
            "a round names the corpus revision and the model revision it retrieved with",
        ));
    }
    if let Some(part) = &request.sufficiency {
        if part.round == 0 || part.k == 0 {
            return Err(contract_invalid(
                "sufficiency round is 1-based and k is at least 1",
            ));
        }
    }
    Ok(())
}

/// Execute the plan, then run the requested parts in order: hierarchy, sufficiency,
/// selection, feedback, stop, arbitration. Only the parts this build serves are
/// present in [`RoundRequest`]; each later part joins at its place in that order.
pub fn run_round(
    request: &RoundRequest,
    ctx: &PlanCtx,
    knobs: &RoundKnobs,
) -> Result<RoundResult, RoundError> {
    validate(request)?;
    let rows = execute(&request.plan, ctx).map_err(RoundError::Plan)?;
    let sufficiency = match &request.sufficiency {
        None => None,
        Some(part) => {
            let _span = tracing::info_span!("medulla.retrieval.sufficiency").entered();
            let rule = SufficiencyRule {
                min_evidence: knobs.min_evidence,
                ..SufficiencyRule::default()
            };
            let call = SufficiencyCall {
                round: part.round,
                round_limit: part.round_limit.map(|l| l.min(knobs.round_limit)),
                k: part.k,
                fallback_mode: part.fallback_mode.clone(),
                fallback: part.fallback,
            };
            Some(judge_sufficiency(&rows, &rule, &call)?)
        }
    };
    Ok(RoundResult {
        rows: rows
            .rows()
            .iter()
            .map(|r| RoundRow {
                id: r.id.clone(),
                score: r.score,
            })
            .collect(),
        sufficiency,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_graph_core::graph::GraphCore;
    use medulla_result_types::retrieval_round::SufficiencyPart;
    use medulla_result_types::retrieval_verdict::{
        FallbackPolicy, RepairStep, Verdict, RETRIEVAL_ROUND_LIMIT_REQUIRED,
    };
    use medulla_semantic_index::semantic::SemanticStore;
    use medulla_types::wire::{Op, Plan};

    const KNOBS: RoundKnobs = RoundKnobs {
        min_evidence: 3,
        round_limit: 4,
    };

    fn blob(label: &str) -> Vec<u8> {
        rmp_serde::to_vec_named(&serde_json::json!({ "type": label })).expect("encodes")
    }

    fn request(sufficiency: Option<SufficiencyPart>) -> RoundRequest {
        RoundRequest {
            plan: Plan::new(vec![Op::Scan {
                label: "Doc".into(),
            }]),
            corpus_revision: "corpus-1".into(),
            model_revision: "model-1".into(),
            sufficiency,
        }
    }

    fn part(round_limit: Option<u32>) -> SufficiencyPart {
        SufficiencyPart {
            round: 1,
            round_limit,
            k: 4,
            fallback_mode: "extractive".into(),
            fallback: FallbackPolicy::Lenient,
        }
    }

    fn run(docs: usize, request: &RoundRequest) -> Result<RoundResult, RoundError> {
        let core = GraphCore::new();
        for i in 0..docs {
            core.add_node(format!("d{i}"), blob("Doc"));
        }
        core.add_node("other".into(), blob("Other"));
        let view = core.analysis_snapshot();
        let semantic = SemanticStore::new();
        run_round(request, &PlanCtx::new(&view, &semantic), &KNOBS)
    }

    #[test]
    fn run_round_returns_verdict_for_rule_plan() {
        // Two rows are below min_evidence 3: insufficient, boost k on round 1.
        let short = run(2, &request(Some(part(Some(4))))).expect("round runs");
        assert_eq!(short.rows.len(), 2);
        let verdict = short.sufficiency.expect("verdict beside rows");
        assert_eq!(verdict.verdict, Verdict::Insufficient);
        assert_eq!(verdict.evidence_count, 2);
        assert_eq!(verdict.repair, RepairStep::BoostK { k: 8 });

        // Three rows meet the rule.
        let enough = run(3, &request(Some(part(Some(4))))).expect("round runs");
        let verdict = enough.sufficiency.expect("verdict beside rows");
        assert_eq!(
            (verdict.verdict, verdict.repair),
            (Verdict::Sufficient, RepairStep::Stop)
        );

        // A caller cannot raise the round limit above the knob.
        let capped = run(3, &request(Some(part(Some(99))))).expect("round runs");
        assert_eq!(capped.sufficiency.expect("verdict").round_limit, 4);

        // No sufficiency part: rows only.
        let bare = run(3, &request(None)).expect("round runs");
        assert_eq!(bare.rows.len(), 3);
        assert!(bare.sufficiency.is_none());

        // A missing round limit and a malformed part refuse with typed codes.
        match run(3, &request(Some(part(None)))) {
            Err(RoundError::Refusal(r)) => assert_eq!(r.code, RETRIEVAL_ROUND_LIMIT_REQUIRED),
            other => panic!("expected a refusal, got {other:?}"),
        }
        let mut zero = part(Some(4));
        zero.round = 0;
        match run(3, &request(Some(zero))) {
            Err(RoundError::Refusal(r)) => assert_eq!(r.code, RETRIEVAL_CONTRACT_INVALID),
            other => panic!("expected a refusal, got {other:?}"),
        }
        let mut blank = request(None);
        blank.model_revision = " ".into();
        assert!(matches!(run(1, &blank), Err(RoundError::Refusal(_))));
    }
}
