# Adding a question root

A new question root is four edits and one test. Copy this file's pieces.

1. **One family file**, `family/<kind>.rs`, implementing `DecisionFamily`
   (override only the stages the kind owns; the defaults do nothing). The
   file below is compiled by the `template_compiles` test, so it cannot rot:

```rust
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
```

2. **One registration line** in `family/mod.rs`: a `mod <kind>;` line, a
   `static <KIND>FAMILY` in `for_kind`, and the `QuestionKind::<Kind>` arm.
   The exhaustive `match` makes a kind without a family a compile error.
3. **One refusal table** in the family file (`REFUSALS` above): each row maps a
   stage failure to its typed refusal code, applied through `refusals`.
4. **One served test** in `stat_tests/<kind>_tests.rs` that calls
   `Method::Decide` for the new kind through the router and asserts the
   record, the refusal codes and the replay.

Spec: `MEDULLA-DECISION-ENGINE-R001.101.4`.
