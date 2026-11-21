//! The `schema_mapping` question family (MEDULLA-DECISION-ENGINE-R033.2.2).
//!
//! A column (`column.name`, `column.sql_type`, optional `column.samples`) is
//! scored against the declared ontology properties by
//! `medulla_decision_stats::decision::schema_mapping::rank`. Each declared
//! option is one property: its option id is the property IRI and its text
//! `datatype` the XSD datatype it declares. The top property is proposed only
//! when its score reaches `knob schema_mapping.accept_min`; below it the
//! decision abstains with `InsufficientConfidence`. The answer proposes and
//! never writes. The weights and the floor are decision-policy knobs read from
//! the policy the request resolves.

use medulla_decision_stats::decision::ladder::LadderResult;
use medulla_decision_stats::decision::schema_mapping::{
    rank, OntologyProperty, SqlColumn, ValueType, Weights,
};
use medulla_decision_types::decision::knob_families::schema_mapping as mapping_knobs;
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, ScoredOption, StatisticalErrorCode, StatisticalOutcome,
    TypedValue,
};
use medulla_decision_types::decision::{
    AbstainReason, DecisionPolicy, PremiseRef, QuantScaleTag, QuantisedValue,
};
use medulla_graph_core::graph::GraphView;
use medulla_types::contract::BoundedVec;

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};
use crate::decide::stat_support::refusal;

use super::DecisionFamily;

/// The family of the `schema_mapping` question kind.
pub struct SchemaMappingFamily;

struct Column {
    name: String,
    value_type: ValueType,
    samples: Vec<String>,
}

fn text_param<'a>(request: &'a DecideRequest, name: &str) -> Option<&'a str> {
    request
        .params
        .iter()
        .find(|param| param.name == name)
        .and_then(|param| match &param.value {
            TypedValue::Text(text) => Some(text.as_str()),
            _ => None,
        })
}

fn column_of(request: &DecideRequest) -> Result<Column, String> {
    let missing = |detail: &str| refusal(StatisticalErrorCode::SchemaMappingColumnMissing, detail);
    let name = text_param(request, "column.name")
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| missing("a schema_mapping question names its column.name"))?;
    let sql_type = text_param(request, "column.sql_type")
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| missing("a schema_mapping question names its column.sql_type"))?;
    let value_type = ValueType::from_sql(sql_type).ok_or_else(|| {
        refusal(
            StatisticalErrorCode::SchemaMappingSqlTypeUnknown,
            format!("sql type '{sql_type}' is outside the closed set"),
        )
    })?;
    let samples = match request.params.iter().find(|p| p.name == "column.samples") {
        None => Vec::new(),
        Some(param) => match &param.value {
            TypedValue::IriList(list) => list.iter().cloned().collect(),
            _ => {
                return Err(refusal(
                    StatisticalErrorCode::ParameterInvalid,
                    "column.samples must be an iri_list of literal strings",
                ))
            }
        },
    };
    Ok(Column {
        name: name.to_string(),
        value_type,
        samples,
    })
}

fn datatype_of(option: &DeclaredOption) -> Option<ValueType> {
    option
        .texts
        .iter()
        .find(|entry| entry.key == "datatype")
        .and_then(|entry| ValueType::from_xsd(&entry.text))
}

fn q32_of(micros: i64) -> i64 {
    ((i128::from(micros) << 32) / 1_000_000) as i64
}

impl DecisionFamily for SchemaMappingFamily {
    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        column_of(request).map(|_| ())
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

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        // Only declared properties carry a datatype; a graph or library
        // source falls through to the shared matrix ladder.
        let CandidateSource::Declared { options } = &request.candidates else {
            return None;
        };
        Some(column_of(request).and_then(|column| conclude(&column, options.as_slice(), policy)))
    }
}

fn conclude(
    column: &Column,
    options: &[DeclaredOption],
    policy: &DecisionPolicy,
) -> Result<Executed, String> {
    let weights = Weights::from_micros(mapping_knobs::weights_micros(policy));
    let properties: Vec<OntologyProperty<'_>> = options
        .iter()
        .map(|option| OntologyProperty {
            iri: &option.option_id,
            datatype: datatype_of(option),
        })
        .collect();
    let profile = SqlColumn {
        name: &column.name,
        value_type: column.value_type,
        samples: &column.samples,
    };
    let ranked = rank(&profile, &properties, &weights);
    let accept = q32_of(mapping_knobs::accept_min_micros(policy));
    let invalid = |detail: String| refusal(StatisticalErrorCode::ParameterInvalid, detail);
    let mut family_premises = Vec::new();
    let outcome = match ranked.first() {
        Some(top) if top.score.score >= accept => {
            family_premises.push(FamilyPremise {
                subject: "schema_mapping".into(),
                fact: format!("mapping:{}={}", column.name, properties[top.index].iri),
                class: top.score.class,
                source: FamilyPremiseSource::Request("candidates.options"),
            });
            let scores = ranked
                .iter()
                .map(|scored| ScoredOption {
                    option_id: properties[scored.index].iri.to_string(),
                    score: QuantisedValue {
                        scale: QuantScaleTag::Q32,
                        value: scored.score.score,
                    },
                    probability: None,
                })
                .collect();
            StatisticalOutcome::advisory(BoundedVec::new(scores).map_err(invalid)?, false)
        }
        _ => StatisticalOutcome::Abstained {
            reasons: BoundedVec::new(vec![AbstainReason::InsufficientConfidence])
                .map_err(invalid)?,
        },
    };
    Ok(Executed {
        matrix: None,
        ladder: LadderResult {
            outcome,
            calibration: None,
            explained: None,
            audit: None,
            logging: Vec::new(),
        },
        reading: None,
        seed: [0u8; 32],
        harness_contrast: None,
        family: None,
        family_premises,
        route: None,
    })
}
