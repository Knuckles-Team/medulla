//! MEDULLA-ORCH-VOCAB-R009.18.1 — the observation window of one `:RunTrace`.
//!
//! Every measured field is [`Measured::Measured`] or [`Measured::Unknown`];
//! an unknown value is never coerced to zero.

use serde::{Deserialize, Serialize};

/// Default bound on `latency_samples_ms` (`orchestration_limits.window_samples_max`).
pub const WINDOW_SAMPLES_MAX_DEFAULT: usize = 1000;
/// Inclusive range of `orchestration_limits.window_samples_max`.
pub const WINDOW_SAMPLES_MAX_RANGE: (usize, usize) = (10, 10_000);

/// A measured value, or the reason it is unknown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "value")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum Measured<T> {
    Measured(T),
    Unknown(String),
}

impl<T> Measured<T> {
    pub fn is_unknown(&self) -> bool {
        matches!(self, Measured::Unknown(_))
    }
}

/// The four token counters of a window (R010.101).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct WindowUsage {
    pub input_tokens: Measured<u64>,
    pub output_tokens: Measured<u64>,
    pub cache_read_tokens: Measured<u64>,
    pub cache_write_tokens: Measured<u64>,
}

/// The cost of a window in minor currency units.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct WindowCost {
    pub amount_minor: i64,
    pub currency: String,
    pub price_table_revision: String,
}

/// Currencies whose ISO 4217 minor-unit exponent is 0.
const EXPONENT_0: &[&str] = &[
    "BIF", "CLP", "DJF", "GNF", "ISK", "JPY", "KMF", "KRW", "PYG", "RWF", "UGX", "VND", "VUV",
    "XAF", "XOF", "XPF",
];
/// Currencies whose ISO 4217 minor-unit exponent is 3.
const EXPONENT_3: &[&str] = &["BHD", "IQD", "JOD", "KWD", "LYD", "OMR", "TND"];
/// Currencies whose ISO 4217 minor-unit exponent is 4.
const EXPONENT_4: &[&str] = &["CLF", "UYW"];
/// Currencies whose ISO 4217 minor-unit exponent is 2.
const EXPONENT_2: &[&str] = &[
    "AED", "AFN", "ALL", "AMD", "ANG", "AOA", "ARS", "AUD", "AWG", "AZN", "BAM", "BBD", "BDT",
    "BGN", "BMD", "BND", "BOB", "BRL", "BSD", "BTN", "BWP", "BYN", "BZD", "CAD", "CDF", "CHF",
    "CNY", "COP", "CRC", "CUP", "CVE", "CZK", "DKK", "DOP", "DZD", "EGP", "ERN", "ETB", "EUR",
    "FJD", "FKP", "GBP", "GEL", "GHS", "GIP", "GMD", "GTQ", "GYD", "HKD", "HNL", "HTG", "HUF",
    "IDR", "ILS", "INR", "IRR", "JMD", "KES", "KGS", "KHR", "KPW", "KYD", "KZT", "LAK", "LBP",
    "LKR", "LRD", "LSL", "MAD", "MDL", "MGA", "MKD", "MMK", "MNT", "MOP", "MRU", "MUR", "MVR",
    "MWK", "MXN", "MYR", "MZN", "NAD", "NGN", "NIO", "NOK", "NPR", "NZD", "PAB", "PEN", "PGK",
    "PHP", "PKR", "PLN", "QAR", "RON", "RSD", "RUB", "SAR", "SBD", "SCR", "SDG", "SEK", "SGD",
    "SHP", "SLE", "SOS", "SRD", "SSP", "STN", "SVC", "SYP", "SZL", "THB", "TJS", "TMT", "TOP",
    "TRY", "TTD", "TWD", "TZS", "UAH", "USD", "UYU", "UZS", "VES", "WST", "XCD", "YER", "ZAR",
    "ZMW", "ZWL",
];

/// The ISO 4217 minor-unit exponent of `currency` (JPY 0, USD 2, KWD 3,
/// CLF 4); `None` for a code that is not an ISO 4217 currency. There is no
/// two-decimal assumption (MEDULLA-ORCH-VOCAB-R009.18.3).
pub fn currency_exponent(currency: &str) -> Option<u32> {
    let listed = |set: &[&str]| set.contains(&currency);
    if listed(EXPONENT_2) {
        Some(2)
    } else if listed(EXPONENT_0) {
        Some(0)
    } else if listed(EXPONENT_3) {
        Some(3)
    } else if listed(EXPONENT_4) {
        Some(4)
    } else {
        None
    }
}

impl WindowCost {
    /// The cost in micro-units of the major currency unit: `amount_minor`
    /// times 10^(6 - exponent), where the exponent is the ISO 4217 minor-unit
    /// exponent of `currency`. `None` for a negative amount, an unknown
    /// currency or an overflow; the cost is then unknown, never zero.
    pub fn micro_units(&self) -> Option<u64> {
        let exponent = currency_exponent(&self.currency)?;
        let amount = u64::try_from(self.amount_minor).ok()?;
        amount.checked_mul(10u64.pow(6 - exponent))
    }
}

/// One observation window of a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RunObservationWindow {
    pub run_id: String,
    pub run_revision: u64,
    pub window_id: String,
    pub window_revision: u64,
    pub start_ms: u64,
    pub end_ms: u64,
    pub latency_samples_ms: Measured<Vec<u64>>,
    pub error_count: Measured<u64>,
    pub eligible_attempts: Measured<u64>,
    pub usage: WindowUsage,
    pub cost: Measured<WindowCost>,
    pub model_ref: Measured<String>,
    pub profile_revision: Measured<String>,
    pub width: Measured<u32>,
    pub budget_consumed: Measured<u64>,
    pub budget_remaining: Measured<u64>,
    pub capacity_receipt_ref: Measured<String>,
}

impl RunObservationWindow {
    /// Structural checks: ids present, interval ordered, samples bounded.
    pub fn validate(&self, samples_max: usize) -> Result<(), String> {
        if self.run_id.trim().is_empty() || self.window_id.trim().is_empty() {
            return Err("run_id and window_id are required".into());
        }
        if self.end_ms < self.start_ms {
            return Err("window end_ms is before start_ms".into());
        }
        let (lo, hi) = WINDOW_SAMPLES_MAX_RANGE;
        if !(lo..=hi).contains(&samples_max) {
            return Err(format!("window_samples_max must be in [{lo}, {hi}]"));
        }
        if let Measured::Measured(samples) = &self.latency_samples_ms {
            if samples.len() > samples_max {
                return Err(format!(
                    "latency_samples_ms holds {} samples, above {samples_max}",
                    samples.len()
                ));
            }
        }
        Ok(())
    }

    /// The error rate; `Unknown` when no attempt was eligible or a counter is unknown.
    pub fn error_rate(&self) -> Measured<f64> {
        match (&self.error_count, &self.eligible_attempts) {
            (Measured::Measured(_), Measured::Measured(0)) => {
                Measured::Unknown("zero eligible attempts".into())
            }
            (Measured::Measured(errors), Measured::Measured(eligible)) => {
                Measured::Measured(*errors as f64 / *eligible as f64)
            }
            _ => Measured::Unknown("error counters unknown".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unknown<T>() -> Measured<T> {
        Measured::Unknown("not reported".into())
    }

    // spec: MEDULLA-ORCH-VOCAB-R009.18.1
    #[test]
    fn unknown_stays_unknown_and_round_trips() {
        let window = RunObservationWindow {
            run_id: "run:1".into(),
            run_revision: 2,
            window_id: "window:1".into(),
            window_revision: 1,
            start_ms: 10,
            end_ms: 20,
            latency_samples_ms: Measured::Measured(vec![3, 4]),
            error_count: Measured::Measured(0),
            eligible_attempts: Measured::Measured(0),
            usage: WindowUsage {
                input_tokens: Measured::Measured(5),
                output_tokens: unknown(),
                cache_read_tokens: unknown(),
                cache_write_tokens: unknown(),
            },
            cost: unknown(),
            model_ref: unknown(),
            profile_revision: unknown(),
            width: unknown(),
            budget_consumed: unknown(),
            budget_remaining: unknown(),
            capacity_receipt_ref: unknown(),
        };
        let json = serde_json::to_value(&window).expect("encode");
        assert_eq!(json["usage"]["output_tokens"]["state"], "unknown");
        let back: RunObservationWindow = serde_json::from_value(json).expect("decode");
        assert_eq!(back, window);
        assert!(back.usage.output_tokens.is_unknown());
        assert!(back.error_rate().is_unknown());
        assert!(back.validate(WINDOW_SAMPLES_MAX_DEFAULT).is_ok());
        let mut over = back.clone();
        over.latency_samples_ms = Measured::Measured(vec![1; 11]);
        assert!(over.validate(10).is_err());
        assert!(back.validate(5).is_err());
    }

    // spec: MEDULLA-ORCH-VOCAB-R009.18.3
    #[test]
    fn micro_units_use_the_iso_4217_exponent() {
        let cost = |amount_minor: i64, currency: &str| WindowCost {
            amount_minor,
            currency: currency.into(),
            price_table_revision: "r1".into(),
        };
        assert_eq!(cost(7, "USD").micro_units(), Some(70_000));
        assert_eq!(cost(7, "JPY").micro_units(), Some(7_000_000));
        assert_eq!(cost(7, "KWD").micro_units(), Some(7_000));
        assert_eq!(cost(7, "CLF").micro_units(), Some(700));
        assert_eq!(cost(7, "XXX").micro_units(), None);
        assert_eq!(cost(-1, "USD").micro_units(), None);
        assert_eq!(currency_exponent("usd"), None);
    }
}
