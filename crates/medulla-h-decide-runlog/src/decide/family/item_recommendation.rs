//! The `item_recommendation` question family (MEDULLA-DECISION-ENGINE-R001.101,
//! R175.3, R175.4, R175.5).
//!
//! The question carries one declared option, `catalog`, and one param,
//! `recommend`: a JSON `RecommendRequest` naming the catalog graph, the query
//! embedding, the history item ids, the pause steps and `top_k`. The serving
//! path reads the catalog graph's embedded nodes under the caller's access
//! (`ctx.recommend_catalog`). The family reads the encoder fitted for the
//! catalog's current digest, encodes the catalog with its frozen codebooks,
//! refines the query and answers an `Advisory` ranking the items by cosine on
//! the Q32 scale. An empty catalog, a query of another width, a catalog with no
//! encoder and a catalog whose encoders predate its digest abstain with
//! `EMPTY_CATALOG`, `DIMENSION_MISMATCH`, `ENCODER_MISSING` and `ENCODER_STALE`;
//! no ranking is returned. `Decide` writes nothing.

use sha2::{Digest, Sha256};

use medulla_decision_outcome_types::decision::recommend::RecommendRequest;
use medulla_decision_program_types::decision::knob_families::recommend::RecommendKnobs;
use medulla_decision_statistical_types::decision::record::outcome::refusal::DecisionRefusal;
use medulla_decision_statistical_types::decision::statistical::declared::DeclaredOption;
use medulla_graph_core::graph::GraphView;
use medulla_persistence_records::decision_jobs::decode_artifact;
use medulla_types::evidence_class::EvidenceClass;
use medulla_types::refusal_code::RefusalCode;
use {
    medulla_decision_statistical_types::decision::item_recommendation::encoder_key,
    medulla_decision_statistical_types::decision::item_recommendation::encoder_prefix,
    medulla_decision_statistical_types::decision::item_recommendation::DIMENSION_MISMATCH,
    medulla_decision_statistical_types::decision::item_recommendation::EMPTY_CATALOG,
    medulla_decision_statistical_types::decision::item_recommendation::ENCODER_MISSING,
    medulla_decision_statistical_types::decision::item_recommendation::ENCODER_STALE,
};
use {
    medulla_decision_statistical_types::decision::record::premise::PremiseClass,
    medulla_decision_statistical_types::decision::record::premise::PremiseRef,
    medulla_decision_statistical_types::decision::DecisionPolicy,
};
use {
    medulla_decision_statistical_types::decision::statistical::question_kind::QuestionKind,
    medulla_decision_types::decision::statistical::DecideRequest,
    medulla_decision_types::decision::statistical::StatisticalOutcome,
};
use {
    medulla_decision_stats_records::decision::recommend::encode_catalog,
    medulla_decision_stats_records::decision::recommend::refine_and_rank,
    medulla_decision_stats_records::decision::recommend::Encoder,
    medulla_decision_stats_records::decision::recommend::RecommendError,
};

use medulla_h_decide_base::decide::stat_recommend::PARAM_RECOMMEND;
use {
    medulla_h_decide_base::decide::stat_executor::Executed,
    medulla_h_decide_base::decide::stat_executor::ExecutionContext,
    medulla_h_decide_base::decide::stat_executor::FamilyPremise,
    medulla_h_decide_base::decide::stat_executor::FamilyPremiseSource,
};

use medulla_h_decide_base::decide::family::DecisionFamily;
use {
    medulla_h_decide_base::decide::family::support::abstained,
    medulla_h_decide_base::decide::family::support::advisory_scored,
    medulla_h_decide_base::decide::family::support::invalid,
    medulla_h_decide_base::decide::family::support::json_param,
    medulla_h_decide_base::decide::family::support::single_option,
};

/// The one option a recommendation question declares.
const OPTION: &str = "catalog";

/// The family of the `item_recommendation` question kind.
pub struct ItemRecommendationFamily;

struct Abstention(RefusalCode, String);

impl DecisionRefusal for Abstention {
    fn code(&self) -> RefusalCode {
        self.0
    }
    fn detail(&self) -> String {
        self.1.clone()
    }
}

fn abstain(code: RefusalCode, detail: &str) -> Result<Executed, String> {
    Ok(abstained(
        QuestionKind::ItemRecommendation,
        &Abstention(code, detail.to_string()),
    ))
}

fn recommend(
    ctx: &ExecutionContext,
    request: &DecideRequest,
    policy: &DecisionPolicy,
) -> Result<Executed, String> {
    let ask: RecommendRequest = json_param(request, PARAM_RECOMMEND)?;
    let catalog = ctx
        .recommend_catalog
        .ok_or_else(|| invalid("the catalog graph was not read"))?;
    if catalog.items.is_empty() {
        return abstain(EMPTY_CATALOG, "the catalog graph holds no embedded item");
    }
    if ask.query_embedding.len() != catalog.dimension() {
        return abstain(
            DIMENSION_MISMATCH,
            "the query embedding's width differs from the catalog's",
        );
    }
    let digest = catalog.digest_hex();
    let key = encoder_key(&ask.catalog_ref, &digest);
    let Some(bytes) = ctx.store.decision_artifact(ctx.tenant_id, &key)? else {
        let fitted = ctx.store.decision_artifacts_with_prefix(
            ctx.tenant_id,
            &encoder_prefix(&ask.catalog_ref),
            1,
        )?;
        return if fitted.is_empty() {
            abstain(ENCODER_MISSING, "no encoder was fitted for the catalog")
        } else {
            abstain(
                ENCODER_STALE,
                "the encoders fitted for the catalog predate its current digest",
            )
        };
    };
    let encoder: Encoder = decode_artifact(&bytes, "catalog encoder")?;
    let knobs = RecommendKnobs::from_policy(policy);
    let ranked = encode_catalog(&encoder, &catalog.items).and_then(|encoded| {
        refine_and_rank(
            &encoder,
            &encoded,
            &ask.query_embedding,
            &ask.history_item_ids,
            ask.pause_steps,
            ask.top_k,
            &knobs,
        )
    });
    let ranked = match ranked {
        Ok(ranked) => ranked,
        Err(RecommendError::EmptyCatalog) => {
            return abstain(EMPTY_CATALOG, "the catalog graph holds no embedded item")
        }
        Err(RecommendError::DimensionMismatch) => {
            return abstain(
                DIMENSION_MISMATCH,
                "a vector's width differs from the encoder's",
            )
        }
        Err(RecommendError::KnobOutOfRange) => {
            return Err(invalid("a recommend knob is outside its range"))
        }
    };
    let encoder_digest = hex::encode(Sha256::digest(&bytes));
    let observed = |subject: &'static str, fact: String| FamilyPremise {
        subject: subject.into(),
        fact,
        class: PremiseClass::Observation,
        source: FamilyPremiseSource::Observation(key.clone()),
    };
    let mut premises = vec![
        observed(
            "item_recommendation.encoder",
            format!("encoder:{encoder_digest}"),
        ),
        observed("item_recommendation.catalog", format!("catalog:{digest}")),
        observed(
            "item_recommendation.pause_steps",
            format!("pause_steps:{}", ask.pause_steps),
        ),
    ];
    premises.extend(ranked.iter().map(|item| {
        observed(
            "item_recommendation.item",
            format!("item:{}={}", item.item_id, item.score_q32),
        )
    }));
    let mut executed = advisory_scored(
        ranked
            .into_iter()
            .map(|item| (item.item_id, item.score_q32))
            .collect(),
    )?;
    executed.family_premises = premises;
    Ok(executed)
}

impl DecisionFamily for ItemRecommendationFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, OPTION)
    }

    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(recommend(ctx, request, policy))
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
