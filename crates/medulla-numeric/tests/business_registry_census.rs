//! The business rule registry census (MEDULLA-BUSINESS-OPS-R002.7): five kinds, each
//! registered once, one result arm each, every refusal code of every kind listed.

use std::collections::BTreeMap;

use medulla_numeric::business::phrase_policy::Policy as ClaimPolicy;
use medulla_numeric::business::reconcile::Sets;
use medulla_numeric::business::recurrence::HolidayCalendar;
use medulla_numeric::business::registry::{evaluate_rule, RuleInputs, RuleResult};
use medulla_numeric::business::reorder::{Sale, Stock};
use medulla_numeric::business::rule_spec::{self, InputRef, RuleKind};
use medulla_numeric::business::RULE_REFUSAL_CODES;

/// 2026-01-01T00:00:00Z.
const AS_OF_MS: i64 = 1_767_225_600_000;

fn parameters(text: &str) -> BTreeMap<String, String> {
    rule_spec::parse_parameters(text).expect("canonical parameters")
}

/// A rule of `kind` with `text` parameters, and the minimal inputs of that kind.
fn minimal(kind: RuleKind) -> (rule_spec::RuleSpec, RuleInputs) {
    let (text, inputs) = match kind {
        RuleKind::Recurrence => (
            "adjustment=none\nanchor=2026-01-20\nday=20\nfrequency=monthly\nzone=UTC",
            RuleInputs::Recurrence {
                calendar: HolidayCalendar::default(),
                calendar_node: None,
                count: 1,
            },
        ),
        RuleKind::Threshold => (
            "expr=sales > 5\nmetrics=sales:Payment:sum\nperiod_days=30",
            RuleInputs::Threshold { points: Vec::new() },
        ),
        RuleKind::Reconcile => (
            "currency=same\ntolerance_minor=0\nwindow_days=3",
            RuleInputs::Reconcile { sets: Sets::default() },
        ),
        RuleKind::Reorder => (
            "lead_time_days=2\nminimum_order_quantity=0\nminimum_samples=1\nreview_window_days=7\nsafety_stock=0",
            RuleInputs::Reorder {
                sales: vec![Sale {
                    node_id: "line:1".into(),
                    revision: 1,
                    day: AS_OF_MS.div_euclid(86_400_000) - 4,
                    quantity: 2,
                }],
                stock: Stock {
                    node_id: "stock:1".into(),
                    revision: 1,
                    on_hand: 5,
                    reserved: 0,
                },
                open_lines: Vec::new(),
            },
        ),
        RuleKind::PhrasePolicy => (
            "",
            RuleInputs::PhrasePolicy {
                text: "Plain text.".into(),
                locale: "en".into(),
                policy: ClaimPolicy {
                    revision: "1".into(),
                    forbidden: Vec::new(),
                    approved: Vec::new(),
                },
                policy_node: InputRef::new("policy:1", 1),
            },
        ),
    };
    let spec = rule_spec::load(kind.as_str(), 1, parameters(text)).expect("a loadable rule");
    (spec, inputs)
}

/// Every `pub const NAME: &str = "NAME";` refusal code a source file declares.
fn declared_codes(source: &str) -> Vec<String> {
    source
        .lines()
        .filter_map(|line| line.strip_prefix("pub const "))
        .filter_map(|rest| {
            let (name, tail) = rest.split_once(": &str = \"")?;
            let value = tail.strip_suffix("\";")?;
            (name == value).then(|| value.to_string())
        })
        .collect()
}

// spec: MEDULLA-BUSINESS-OPS-R002.7
#[test]
fn five_kinds_registered_once() {
    // Five kinds, each distinct and each parsing back from its stored name.
    assert_eq!(RuleKind::ALL.len(), 5);
    for (at, kind) in RuleKind::ALL.into_iter().enumerate() {
        assert_eq!(RuleKind::parse(kind.as_str()), Some(kind));
        assert!(RuleKind::ALL[..at].iter().all(|earlier| *earlier != kind));
    }

    // Each kind evaluates over a minimal input and answers the result arm of that kind.
    for kind in RuleKind::ALL {
        let (spec, inputs) = minimal(kind);
        let result = evaluate_rule(&spec, inputs, AS_OF_MS)
            .unwrap_or_else(|refusal| panic!("{}: {refusal}", kind.as_str()));
        let arm = matches!(
            (&result, kind),
            (RuleResult::Recurrence(_), RuleKind::Recurrence)
                | (RuleResult::Threshold(_), RuleKind::Threshold)
                | (RuleResult::Reconcile(_), RuleKind::Reconcile)
                | (RuleResult::Reorder(_), RuleKind::Reorder)
                | (RuleResult::PhrasePolicy(_), RuleKind::PhrasePolicy)
        );
        assert!(arm, "{} answered another kind's result arm", kind.as_str());
        assert_eq!(result.kind(), kind);
        assert_eq!(result.trace().rule_digest, spec.digest);
        assert!(result.canonical_json().starts_with('{'));
    }

    // The inputs of one kind are not the inputs of another.
    let (spec, _) = minimal(RuleKind::Recurrence);
    let (_, other) = minimal(RuleKind::Reconcile);
    let refused = evaluate_rule(&spec, other, AS_OF_MS).unwrap_err();
    assert_eq!(refused.code, rule_spec::RULE_INPUT_INVALID);

    // Every refusal code any kind or the rule record declares is in the list, and the list
    // names no code twice.
    for source in [
        include_str!("../src/business/rule_spec.rs"),
        include_str!("../src/business/recurrence.rs"),
        include_str!("../src/business/threshold.rs"),
        include_str!("../src/business/reconcile.rs"),
        include_str!("../src/business/reorder.rs"),
        include_str!("../src/business/phrase_policy.rs"),
    ] {
        for code in declared_codes(source) {
            assert!(
                RULE_REFUSAL_CODES.contains(&code.as_str()),
                "{code} is declared but not in RULE_REFUSAL_CODES"
            );
        }
    }
    let mut sorted = RULE_REFUSAL_CODES.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), RULE_REFUSAL_CODES.len());
}
