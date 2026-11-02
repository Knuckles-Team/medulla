//! MEDULLA-DECISION-ENGINE-R015: the decision engine's head vocabulary is closed
//! and purely numeric. There is no generative-model-weight head kind and no
//! free-text generation anywhere a head is read: `Evaluated`'s only fields
//! are `f64` vectors, so there is no channel through which any head kind
//! could emit generated text.

use medulla_decision_types::decision::statistical::scorer::OptionAttentionParams;
use medulla_decision_types::{
    decision::statistical::head::DecisionHeadBody,
    decision::statistical::head::FeatureStandardisation, decision::statistical::head::FittedRegime,
    decision::statistical::head::HeadKind,
    decision::statistical::head::DECISION_HEAD_SCHEMA_VERSION,
};
use medulla_decision_types::{decision::QuantScaleTag, decision::QuantisedValue};
use medulla_decision_stats::decision::features::FeatureMatrix;
use medulla_decision_stats::{decision::head_eval::read_head, decision::head_eval::HeadReading};

use super::common::bounded;

const ONE: i64 = 1 << 32;

fn q(value: i64) -> QuantisedValue {
    QuantisedValue {
        scale: QuantScaleTag::Q32,
        value,
    }
}

fn standardisation() -> FeatureStandardisation {
    FeatureStandardisation {
        center: q(0),
        scale: q(ONE),
        lower: q(-4 * ONE),
        upper: q(4 * ONE),
    }
}

/// A minimally shaped head of `kind`: linear kinds carry no scorer;
/// `OptionAttention` carries the smallest legal one (width 2, one feature).
fn head_of_kind(kind: HeadKind) -> DecisionHeadBody {
    let scorer = match kind {
        HeadKind::OptionAttention => Some(Box::new(OptionAttentionParams {
            width: 2,
            shortlist: 2,
            embed: bounded(vec![ONE / 4, -(ONE / 4)]),
            embed_bias: bounded(vec![0, 0]),
            query: bounded(vec![ONE / 8, 0, 0, ONE / 8]),
            key: bounded(vec![ONE / 8, 0, 0, ONE / 8]),
            value: bounded(vec![ONE / 8, 0, 0, ONE / 8]),
            self_weight: bounded(vec![ONE / 4, ONE / 4]),
            context_weight: bounded(vec![ONE / 4, ONE / 4]),
        })),
        HeadKind::WeightedFeatures | HeadKind::ListwiseLogistic => None,
    };
    DecisionHeadBody {
        schema_version: DECISION_HEAD_SCHEMA_VERSION,
        kind,
        regime: FittedRegime::BanditLabel,
        feature_schema_digest: "sha256:schema".to_string(),
        standardisation: bounded(vec![standardisation()]),
        weights: bounded(vec![q(ONE)]),
        calibration: None,
        training_records_digest: "sha256:training".to_string(),
        n_training: 12,
        synthetic: true,
        scorer,
    }
}

fn matrix() -> FeatureMatrix {
    FeatureMatrix {
        candidate_ids: vec!["a".to_string(), "b".to_string()],
        feature_names: vec!["score".to_string()],
        values: vec![ONE, -ONE],
    }
}

// spec: MEDULLA-DECISION-ENGINE-R015
#[test]
fn every_head_kind_produces_only_numeric_output_never_free_text() {
    for kind in [
        HeadKind::WeightedFeatures,
        HeadKind::ListwiseLogistic,
        HeadKind::OptionAttention,
    ] {
        // Exhaustive by construction: a `HeadKind` this match does not name
        // fails to compile here, so a future generative variant cannot slip
        // past this proof silently -- the strongest guarantee short of
        // forbidding the variant outright.
        match kind {
            HeadKind::WeightedFeatures | HeadKind::ListwiseLogistic | HeadKind::OptionAttention => {
            }
        }
        let reading = read_head(&head_of_kind(kind), &matrix()).expect("reads");
        let HeadReading::InDistribution(evaluated) = reading else {
            panic!("{kind:?}: fixture must be in distribution")
        };
        // `Evaluated` carries only `f64` vectors (standardised features,
        // logits, an optional calibrated distribution): no string, no token
        // stream, no channel a generative model could speak free text through.
        let _: Vec<Vec<f64>> = evaluated.standardised;
        let _: Vec<f64> = evaluated.logits;
        let _: Option<Vec<f64>> = evaluated.probabilities;
    }
}
