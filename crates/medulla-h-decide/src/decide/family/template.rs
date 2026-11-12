//! Worked example of a new question family (MEDULLA-DECISION-ENGINE-R001.101.4).
//!
//! `TEMPLATE.md` embeds this file verbatim; the `template_compiles` test
//! keeps the two identical, so the template a contributor copies always
//! compiles against the current `DecisionFamily` trait.

use super::DecisionFamily;

/// The refusal table: one row per stage failure the family names.
const REFUSALS: &[(&str, &str)] = &[("no candidate", "template_no_candidate")];

/// The example family. A real family is named `<Kind>Family` and lives in
/// `family/<kind>.rs`.
pub struct TemplateFamily;

impl DecisionFamily for TemplateFamily {
    fn refusals(&self, detail: String) -> String {
        for (needle, code) in REFUSALS {
            if detail.contains(needle) {
                return (*code).to_string();
            }
        }
        detail
    }
}
