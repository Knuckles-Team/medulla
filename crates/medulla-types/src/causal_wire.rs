//! Causal model fit verdict (MEDULLA-FEDERATED-QUERY-R608.1): the result body of
//! `Method::CausalVerify`. Pure serde; the fit itself is
//! `medulla_epistemic::verify_fit`.

use serde::{Deserialize, Serialize};

/// Whether every variable of the model fits the observations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum CausalVerdictKind {
    Consistent,
    Inconsistent,
}

/// The fit of one variable: normalized RMSE of its structural equation's
/// residuals against the pass limit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct VariableFit {
    pub id: String,
    pub nrmse: f64,
    pub limit: f64,
}

/// The verdict of `Method::CausalVerify`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CausalVerdict {
    pub verdict: CausalVerdictKind,
    /// One fit per model variable, in model order.
    pub per_variable: Vec<VariableFit>,
    /// The variable with the largest nrmse (`None` for an empty model).
    pub worst: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-FEDERATED-QUERY-R608.1
    #[test]
    fn verdict_round_trips() {
        let verdict = CausalVerdict {
            verdict: CausalVerdictKind::Inconsistent,
            per_variable: vec![
                VariableFit {
                    id: "x".into(),
                    nrmse: 0.1,
                    limit: 0.25,
                },
                VariableFit {
                    id: "y".into(),
                    nrmse: 0.5,
                    limit: 0.25,
                },
            ],
            worst: Some("y".into()),
        };
        let json = serde_json::to_value(&verdict).unwrap();
        assert_eq!(json["verdict"], "inconsistent");
        assert_eq!(json["per_variable"][1]["id"], "y");
        assert_eq!(json["worst"], "y");
        let back: CausalVerdict = serde_json::from_value(json).unwrap();
        assert_eq!(back, verdict);
        assert_eq!(
            serde_json::to_value(CausalVerdictKind::Consistent).unwrap(),
            "consistent"
        );
    }
}
