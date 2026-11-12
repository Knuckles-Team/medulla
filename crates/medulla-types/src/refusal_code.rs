//! The stable refusal code type shared by every refusal family table
//! (MEDULLA-DECISION-ENGINE-R010.100.1).
//!
//! It lives in the lowest crate any family table needs, so a `medulla-types`
//! module declares `pub const REFUSALS: &[RefusalCode]` without depending on the
//! decision crates. The decision plane collects every table and refuses a code
//! declared twice. On the wire a code is its upper-snake string; decoding a
//! string back into a registered code is done by the registry that owns the
//! tables (`medulla_decision_types::decision::codes::resolve`), not here.

use serde::{Serialize, Serializer};

/// A stable refusal code: one entry of a family `REFUSALS` table or of the
/// shared request codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RefusalCode(&'static str);

impl RefusalCode {
    /// Declare a code. Only tables and the fixed variants call this.
    pub const fn new(code: &'static str) -> Self {
        Self(code)
    }

    /// The stable wire string.
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl Serialize for RefusalCode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0)
    }
}

/// Hand-written: the wire form is the code string, not the private field.
#[cfg(feature = "contract-schema")]
impl schemars::JsonSchema for RefusalCode {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("RefusalCode")
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::Schema::try_from(serde_json::json!({
            "type": "string",
            "description": "A registered decision refusal code",
            "pattern": "^[A-Z][A-Z0-9_]*$",
        }))
        .expect("a JSON object is a valid schema")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DECISION-ENGINE-R010.100.1
    #[test]
    fn code_serialises_as_its_string_and_orders_by_text() {
        let a = RefusalCode::new("A_CODE");
        let b = RefusalCode::new("B_CODE");
        assert_eq!(serde_json::to_string(&a).expect("encodes"), "\"A_CODE\"");
        assert!(a < b);
        assert_eq!(a.as_str(), "A_CODE");
    }
}
