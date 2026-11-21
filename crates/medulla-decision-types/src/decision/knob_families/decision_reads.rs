//! The `decision.*` read knobs of the committed-decision reads
//! (MEDULLA-DECISION-ENGINE-R870.1). A decision-plane knob: it has no
//! `knob-table.md` row.

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};

/// The most records one `DecisionLog` `List` page returns.
pub const LIST_PAGE_MAX: KnobSpec = KnobSpec {
    name: "decision.list_page_max",
    unit: "records",
    default: KnobValue::Integer(200),
    min: KnobValue::Integer(1),
    max: KnobValue::Integer(1000),
    safety: KnobSafety::Tunable,
};

/// Every `decision.*` read knob.
pub const KNOBS: &[KnobSpec] = &[LIST_PAGE_MAX];

/// The page maximum a `List` read enforces: the knob's default, because a
/// read carries no pinned policy.
pub fn list_page_max() -> u32 {
    match LIST_PAGE_MAX.default {
        KnobValue::Integer(value) => value as u32,
        _ => 200,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_maximum_sits_inside_its_range() {
        assert_eq!(list_page_max(), 200);
        assert_eq!(
            LIST_PAGE_MAX.default.compare(&LIST_PAGE_MAX.max),
            Some(std::cmp::Ordering::Less)
        );
    }
}
