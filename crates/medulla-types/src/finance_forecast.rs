//! Probabilistic forecast evidence (MEDULLA-FINANCE-PRIMITIVES-R902).
//!
//! A `ForecastSample` is one drawn sample path of a model forecast: the series it
//! forecasts, the model revision, the seed and sampling controls that produced it,
//! and the drawn values. It is always stored with `probabilistic = true`: a sample
//! is a draw, never a measurement, so the deterministic sizing and backtest kernels
//! refuse it as an input. It may enter the decision engine only as evidence of
//! class [`EvidenceClass::Claim`] carrying the premise `forecast:seed=<n>`.
//!
//! The stored node carries one property per field under the names in [`property`];
//! admission writes them from the validated sample, never from the mapped payload,
//! so a source cannot store a forecast with `probabilistic = false`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::evidence_class::EvidenceClass;

/// The ontology class a forecast sample is stored under (finance module).
pub const FORECAST_SAMPLE_CLASS: &str = "ForecastSample";

/// Wire code of an invalid forecast sample at ingest.
pub const FORECAST_SAMPLE_INVALID: &str = "FORECAST_SAMPLE_INVALID";

/// Wire code of a probabilistic input handed to a deterministic kernel.
pub const PROBABILISTIC_INPUT_REFUSED: &str = "PROBABILISTIC_INPUT_REFUSED";

/// Most values one sample may carry.
pub const MAX_SAMPLE_VALUES: usize = 100_000;

/// Stored property names of a forecast sample node (the finance ontology datatype properties).
pub mod property {
    pub const SERIES_ID: &str = "forecastSeriesId";
    pub const MODEL_REVISION: &str = "forecastModelRevision";
    pub const SEED: &str = "forecastSeed";
    pub const TEMPERATURE: &str = "forecastTemperature";
    pub const TOP_P: &str = "forecastTopP";
    pub const SAMPLE_COUNT: &str = "forecastSampleCount";
    /// JSON array of the drawn values.
    pub const VALUES: &str = "forecastValues";
    pub const PROBABILISTIC: &str = "probabilistic";
}

/// One drawn forecast sample with the controls that produced it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ForecastSample {
    /// The series the forecast is about.
    pub series_id: String,
    /// The revision of the model that drew the sample.
    pub model_revision: String,
    /// The sampler seed.
    pub seed: u64,
    /// The sampling temperature, finite and non-negative.
    pub temperature: f64,
    /// The nucleus-sampling mass, in `(0, 1]`.
    pub top_p: f64,
    /// How many paths the draw sampled, at least one.
    pub sample_count: u32,
    /// The drawn values, finite and non-empty.
    pub values: Vec<f64>,
    /// Always true once admitted.
    pub probabilistic: bool,
}

fn invalid(detail: impl std::fmt::Display) -> String {
    format!("{FORECAST_SAMPLE_INVALID}: {detail}")
}

impl ForecastSample {
    /// Build a sample from a source payload, validating every field. `probabilistic`
    /// is forced true whatever the payload says.
    pub fn from_source(source: &Map<String, Value>) -> Result<Self, String> {
        let text = |name: &str| {
            source
                .get(name)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| invalid(format!("{name} must be a non-empty string")))
        };
        let number = |name: &str| {
            source
                .get(name)
                .and_then(Value::as_f64)
                .filter(|value| value.is_finite())
                .ok_or_else(|| invalid(format!("{name} must be a finite number")))
        };
        let seed = source
            .get("seed")
            .and_then(Value::as_u64)
            .ok_or_else(|| invalid("seed must be an unsigned integer"))?;
        let sample_count = source
            .get("sample_count")
            .and_then(Value::as_u64)
            .and_then(|count| u32::try_from(count).ok())
            .filter(|count| *count >= 1)
            .ok_or_else(|| invalid("sample_count must be an integer of at least 1"))?;
        let temperature = number("temperature")?;
        if temperature < 0.0 {
            return Err(invalid("temperature must not be negative"));
        }
        let top_p = number("top_p")?;
        if !(top_p > 0.0 && top_p <= 1.0) {
            return Err(invalid("top_p must be in (0, 1]"));
        }
        let values = source
            .get("values")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid("values must be an array"))?;
        if values.is_empty() || values.len() > MAX_SAMPLE_VALUES {
            return Err(invalid(format!(
                "values must hold 1 to {MAX_SAMPLE_VALUES} entries"
            )));
        }
        let values = values
            .iter()
            .map(|value| {
                value
                    .as_f64()
                    .filter(|value| value.is_finite())
                    .ok_or_else(|| invalid("every value must be a finite number"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            series_id: text("series_id")?,
            model_revision: text("model_revision")?,
            seed,
            temperature,
            top_p,
            sample_count,
            values,
            probabilistic: true,
        })
    }

    /// The stored properties of this sample, one per field, `probabilistic` always true.
    pub fn stored_properties(&self) -> Vec<(&'static str, Value)> {
        vec![
            (property::SERIES_ID, self.series_id.clone().into()),
            (property::MODEL_REVISION, self.model_revision.clone().into()),
            (property::SEED, self.seed.into()),
            (property::TEMPERATURE, self.temperature.into()),
            (property::TOP_P, self.top_p.into()),
            (property::SAMPLE_COUNT, self.sample_count.into()),
            (
                property::VALUES,
                serde_json::to_string(&self.values)
                    .unwrap_or_default()
                    .into(),
            ),
            (property::PROBABILISTIC, true.into()),
        ]
    }

    /// Read a sample back from its stored properties, or `None` when the node is not a
    /// readable forecast sample. The flag is read as stored, never assumed.
    pub fn from_stored(props: &Map<String, Value>) -> Option<Self> {
        let text = |name: &str| props.get(name).and_then(Value::as_str).map(str::to_owned);
        Some(Self {
            series_id: text(property::SERIES_ID)?,
            model_revision: text(property::MODEL_REVISION)?,
            seed: props.get(property::SEED)?.as_u64()?,
            temperature: props.get(property::TEMPERATURE)?.as_f64()?,
            top_p: props.get(property::TOP_P)?.as_f64()?,
            sample_count: u32::try_from(props.get(property::SAMPLE_COUNT)?.as_u64()?).ok()?,
            values: serde_json::from_str(&text(property::VALUES)?).ok()?,
            probabilistic: props.get(property::PROBABILISTIC)?.as_bool()?,
        })
    }

    /// The evidence class a forecast may enter the decision engine as, with its premise.
    pub fn decision_evidence(&self) -> (EvidenceClass, String) {
        (EvidenceClass::Claim, format!("forecast:seed={}", self.seed))
    }
}

/// Whether a stored node's `type` property names the forecast sample class.
pub fn is_forecast_sample_class(type_property: Option<&Value>) -> bool {
    type_property
        .and_then(Value::as_str)
        .is_some_and(|class| class.trim_start_matches(':') == FORECAST_SAMPLE_CLASS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn payload() -> Map<String, Value> {
        json!({"series_id": "SPY", "model_revision": "m-7", "seed": 42, "temperature": 0.7,
               "top_p": 0.9, "sample_count": 8, "values": [1.0, 2.5, 3.0], "probabilistic": false})
        .as_object()
        .unwrap()
        .clone()
    }

    #[test]
    fn admission_forces_the_flag_and_keeps_the_controls() {
        let sample = ForecastSample::from_source(&payload()).unwrap();
        assert!(sample.probabilistic);
        assert_eq!((sample.seed, sample.sample_count), (42, 8));
        let stored: Map<String, Value> = sample
            .stored_properties()
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect();
        assert_eq!(ForecastSample::from_stored(&stored), Some(sample));
    }

    #[test]
    fn every_invalid_field_is_refused_with_the_code() {
        for (field, bad) in [
            ("seed", json!(-1)),
            ("temperature", json!(-0.1)),
            ("top_p", json!(0.0)),
            ("top_p", json!(1.5)),
            ("sample_count", json!(0)),
            ("values", json!([])),
            ("values", json!([1.0, "x"])),
            ("series_id", json!("  ")),
            ("model_revision", json!(3)),
        ] {
            let mut source = payload();
            source.insert(field.into(), bad);
            let error = ForecastSample::from_source(&source).unwrap_err();
            assert!(
                error.starts_with(FORECAST_SAMPLE_INVALID),
                "{field}: {error}"
            );
        }
    }

    #[test]
    fn a_forecast_enters_decisions_only_as_a_claim_with_its_seed_premise() {
        let sample = ForecastSample::from_source(&payload()).unwrap();
        assert_eq!(
            sample.decision_evidence(),
            (EvidenceClass::Claim, "forecast:seed=42".to_string())
        );
    }

    #[test]
    fn the_class_check_accepts_the_prefixed_form_only() {
        assert!(is_forecast_sample_class(Some(&json!("ForecastSample"))));
        assert!(is_forecast_sample_class(Some(&json!(":ForecastSample"))));
        assert!(!is_forecast_sample_class(Some(&json!("BarSeries"))));
        assert!(!is_forecast_sample_class(None));
    }
}
