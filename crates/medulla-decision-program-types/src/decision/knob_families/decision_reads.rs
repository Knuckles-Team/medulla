//! The `decision.*` read knobs of the committed-decision reads
//! (MEDULLA-DECISION-ENGINE-R870.1). A decision-plane knob: it has no
//! `knob-table.md` row.

use {
    medulla_decision_statistical_types::decision::knobs::KnobSafety,
    medulla_decision_statistical_types::decision::knobs::KnobSpec,
    medulla_decision_statistical_types::decision::knobs::KnobValue,
};

/// The most records one `DecisionLog` `List` page returns.
pub const LIST_PAGE_MAX: KnobSpec = KnobSpec {
    name: "decision.list_page_max",
    unit: "records",
    default: KnobValue::Integer(200),
    min: KnobValue::Integer(1),
    max: KnobValue::Integer(1000),
    safety: KnobSafety::Tunable,
};

/// The most wall-clock time one `DecisionLog` `WhyNot` re-solve may spend
/// (MEDULLA-DECISION-ENGINE-R870.3).
pub const WHY_NOT_BUDGET_MS: KnobSpec = KnobSpec {
    name: "decision.why_not_budget_ms",
    unit: "ms",
    default: KnobValue::Integer(2000),
    min: KnobValue::Integer(100),
    max: KnobValue::Integer(30000),
    safety: KnobSafety::Tunable,
};

/// Every `decision.*` read knob.
pub const KNOBS: &[KnobSpec] = &[LIST_PAGE_MAX, WHY_NOT_BUDGET_MS];

/// The budget a `WhyNot` read enforces: the request's own, tightened to the
/// knob (the smaller of the two); a read carries no pinned policy.
pub fn why_not_budget_ms(requested: Option<u32>) -> u32 {
    let knob = match WHY_NOT_BUDGET_MS.default {
        KnobValue::Integer(value) => value as u32,
        _ => 2000,
    };
    requested.map_or(knob, |asked| asked.min(knob))
}

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
