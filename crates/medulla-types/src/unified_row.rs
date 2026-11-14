//! One row of a served unified plan (`UnifiedQuery`, `TxnUnifiedQuery`).
//!
//! On the wire a row is the MessagePack array `[id, score | nil]`. A row that a
//! non-materialised source answered carries the named values the source attached (for
//! an operation-bound mapping, its fields: MEDULLA-CQ-XSRC-R002.7, R003.4, R003.5) as a
//! third element `{"texts": {name: text}, "values": {name: number}}`, each map omitted
//! when empty. The third element is omitted when the row has neither, so a stored row
//! keeps the two-element shape every existing reader decodes.

use std::collections::BTreeMap;
use std::fmt;

use serde::de::{self, SeqAccess, Visitor};
use serde::ser::SerializeTuple;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// One ranked row of a unified plan: its id, final score and source field values.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UnifiedRow {
    pub id: String,
    pub score: Option<f32>,
    /// The named text values the source attached to the row; empty for a stored row.
    pub texts: BTreeMap<String, String>,
    /// The named numeric values the source attached to the row; empty for a stored row.
    pub values: BTreeMap<String, f64>,
}

/// The third element of a source row.
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RowFields {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    texts: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    values: BTreeMap<String, f64>,
}

impl UnifiedRow {
    /// A row with no source field values.
    pub fn new(id: impl Into<String>, score: Option<f32>) -> Self {
        Self {
            id: id.into(),
            score,
            ..Self::default()
        }
    }

    /// The `(id, score)` pair of the row.
    pub fn pair(&self) -> (String, Option<f32>) {
        (self.id.clone(), self.score)
    }

    fn has_fields(&self) -> bool {
        !self.texts.is_empty() || !self.values.is_empty()
    }
}

impl From<(String, Option<f32>)> for UnifiedRow {
    fn from((id, score): (String, Option<f32>)) -> Self {
        Self::new(id, score)
    }
}

impl Serialize for UnifiedRow {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let len = if self.has_fields() { 3 } else { 2 };
        let mut tuple = serializer.serialize_tuple(len)?;
        tuple.serialize_element(&self.id)?;
        tuple.serialize_element(&self.score)?;
        if self.has_fields() {
            tuple.serialize_element(&RowFields {
                texts: self.texts.clone(),
                values: self.values.clone(),
            })?;
        }
        tuple.end()
    }
}

impl<'de> Deserialize<'de> for UnifiedRow {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct RowVisitor;

        impl<'de> Visitor<'de> for RowVisitor {
            type Value = UnifiedRow;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a row array [id, score | nil] or [id, score | nil, {texts, values}]")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<UnifiedRow, A::Error> {
                let id: String = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(0, &self))?;
                let score: Option<f32> = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(1, &self))?;
                let fields: RowFields = seq.next_element()?.unwrap_or_default();
                if seq.next_element::<de::IgnoredAny>()?.is_some() {
                    return Err(de::Error::invalid_length(4, &self));
                }
                Ok(UnifiedRow {
                    id,
                    score,
                    texts: fields.texts,
                    values: fields.values,
                })
            }
        }

        deserializer.deserialize_seq(RowVisitor)
    }
}

#[cfg(feature = "contract-schema")]
impl schemars::JsonSchema for UnifiedRow {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("UnifiedRow")
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::Schema::try_from(serde_json::json!({
            "type": "array",
            "description": "One unified plan row: [id, score | nil] or [id, score | nil, {texts, values}]",
            "prefixItems": [
                {"type": "string"},
                {"type": ["number", "null"], "format": "float"},
                {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "texts": {"type": "object", "additionalProperties": {"type": "string"}},
                        "values": {"type": "object", "additionalProperties": {"type": "number"}},
                    },
                },
            ],
            "minItems": 2,
            "maxItems": 3,
        }))
        .expect("a JSON object is a valid schema")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-CQ-XSRC-R002.7
    #[test]
    fn a_row_without_fields_keeps_the_pair_shape() {
        let row = UnifiedRow::new("a", Some(0.5));
        let bytes = rmp_serde::to_vec_named(&row).expect("encodes");
        let pair: (String, Option<f32>) = rmp_serde::from_slice(&bytes).expect("a pair");
        assert_eq!(pair, ("a".to_string(), Some(0.5)));
        let back: UnifiedRow = rmp_serde::from_slice(&bytes).expect("decodes");
        assert_eq!(back, row);
    }

    // spec: MEDULLA-CQ-XSRC-R003.4
    #[test]
    fn a_source_row_carries_its_texts_and_values_as_a_third_element() {
        let mut row = UnifiedRow::new("https://billing.example.com:8443/health", None);
        row.texts.insert("status".into(), "up".into());
        row.values.insert("monitorId".into(), 42.0);
        let json = serde_json::to_value(&row).expect("encodes");
        assert_eq!(
            json,
            serde_json::json!([
                "https://billing.example.com:8443/health",
                null,
                {"texts": {"status": "up"}, "values": {"monitorId": 42.0}}
            ])
        );
        let bytes = rmp_serde::to_vec_named(&row).expect("encodes");
        let back: UnifiedRow = rmp_serde::from_slice(&bytes).expect("decodes");
        assert_eq!(back, row);
    }

    // spec: MEDULLA-CQ-XSRC-R003.5
    #[test]
    fn a_malformed_row_is_refused() {
        for bad in [
            serde_json::json!(["a", null, {}, 1]),
            serde_json::json!(["a", null, {"other": {}}]),
        ] {
            assert!(serde_json::from_value::<UnifiedRow>(bad).is_err());
        }
    }
}
