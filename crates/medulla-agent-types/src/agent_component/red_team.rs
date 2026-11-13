//! Red-team attack, transform and scorer records (MEDULLA-DECISION-ENGINE-R138.1,
//! R138.2 and R138.7).
//!
//! The three records are library component bodies. They travel as the
//! attributes of a component of kind `RedTeamAttack`, `RedTeamTransform` or
//! `RedTeamScorer`; the component id is the record id. Publishing validates the
//! attributes by decoding them into the matching record here, so a stored
//! record always decodes.

use std::collections::BTreeMap;

use medulla_types::refusal_code::RefusalCode;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Refusal: a red-team record body that does not decode, has a blank field, an
/// unknown attribute or a scorer threshold above [`MAX_SCORER_THRESHOLD`].
pub const RED_TEAM_RECORD_INVALID: RefusalCode = RefusalCode::new("RED_TEAM_RECORD_INVALID");

const CODE: &str = RED_TEAM_RECORD_INVALID.as_str();

/// Every refusal code of the red-team records.
pub const REFUSALS: &[RefusalCode] = &[RED_TEAM_RECORD_INVALID];

/// The highest scorer threshold: scores and thresholds are per mille.
pub const MAX_SCORER_THRESHOLD: u16 = 1000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RedTeamAttack {
    pub id: String,
    pub category: String,
    pub goal: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RedTeamTransform {
    pub id: String,
    pub kind: String,
}

/// A scorer passes when its score reaches `threshold` (both per mille).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RedTeamScorer {
    pub id: String,
    pub category: String,
    pub threshold: u16,
}

/// Decode a record from a component's id and attributes. Every attribute must
/// be a field of the record (unknown ones are refused) and every field must be
/// present and non-blank; `numeric` names the fields that are numbers.
fn decode<T: DeserializeOwned>(
    record: &str,
    id: &str,
    attributes: &BTreeMap<String, String>,
    numeric: &[&str],
) -> Result<T, String> {
    let invalid = |detail: String| format!("{CODE}: {record} '{id}' {detail}");
    let mut body = serde_json::Map::new();
    body.insert("id".into(), serde_json::Value::String(id.to_string()));
    for (name, value) in attributes {
        let value = value.trim();
        if value.is_empty() {
            return Err(invalid(format!("has a blank '{name}' attribute")));
        }
        let json = if numeric.contains(&name.as_str()) {
            let number: u64 = value
                .parse()
                .map_err(|_| invalid(format!("'{name}' is not a number")))?;
            serde_json::Value::from(number)
        } else {
            serde_json::Value::String(value.to_string())
        };
        body.insert(name.clone(), json);
    }
    if id.trim().is_empty() {
        return Err(invalid("has a blank id".into()));
    }
    serde_json::from_value(serde_json::Value::Object(body))
        .map_err(|error| invalid(format!("does not decode: {error}")))
}

impl RedTeamAttack {
    /// Decode a stored `RedTeamAttack` component: attributes `category`, `goal`.
    pub fn from_attributes(
        id: &str,
        attributes: &BTreeMap<String, String>,
    ) -> Result<Self, String> {
        decode("red team attack", id, attributes, &[])
    }
}

impl RedTeamTransform {
    /// Decode a stored `RedTeamTransform` component: attribute `kind`.
    pub fn from_attributes(
        id: &str,
        attributes: &BTreeMap<String, String>,
    ) -> Result<Self, String> {
        decode("red team transform", id, attributes, &[])
    }
}

impl RedTeamScorer {
    /// Decode a stored `RedTeamScorer` component: attributes `category`,
    /// `threshold` (at most [`MAX_SCORER_THRESHOLD`]).
    pub fn from_attributes(
        id: &str,
        attributes: &BTreeMap<String, String>,
    ) -> Result<Self, String> {
        let scorer: Self = decode("red team scorer", id, attributes, &["threshold"])?;
        if scorer.threshold > MAX_SCORER_THRESHOLD {
            return Err(format!(
                "{CODE}: red team scorer '{id}' threshold {} exceeds \
                 {MAX_SCORER_THRESHOLD}",
                scorer.threshold
            ));
        }
        Ok(scorer)
    }
}
