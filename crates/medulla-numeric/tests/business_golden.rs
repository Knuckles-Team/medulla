//! MEDULLA-BUSINESS-OPS-R002.9: each rule kind returns identical result bytes for identical
//! inputs on two host architectures.
//!
//! Every vector renders one result to a canonical line of text and compares it with the
//! literal below, which was fixed once: integer arithmetic only, no clock, no randomness,
//! no float, so the line is the same on x86-64 and on aarch64. The rule digests are SHA-256
//! over fixed framing and are pinned too.

use std::collections::BTreeMap;

use medulla_numeric::business::phrase_policy::{self, Mode, Phrase};
use medulla_numeric::business::reconcile::{
    self, CurrencyRule, Fee, MatchKind, Node, Params, Refund, Sets,
};
use medulla_numeric::business::recurrence::{
    self, Adjustment, Date, DaySelector, Frequency, HolidayCalendar, RecurrenceRule,
};
use medulla_numeric::business::reorder::{self, OpenLine, Sale, Stock};
use medulla_numeric::business::rule_spec::{load, InputRef, RuleTrace};
use medulla_numeric::business::threshold::{
    self, Aggregate, Comparison, DataPoint, Expr, MetricDef,
};

fn trace(kind: &str) -> RuleTrace {
    RuleTrace::new(
        &load(kind, 1, BTreeMap::new()).unwrap(),
        vec![InputRef::new("rule:golden", 1)],
    )
}

fn date(year: i32, month: u8, day: u8) -> Date {
    Date::from_ymd(year, month, day).unwrap()
}

#[test]
fn rule_digests_are_pinned() {
    let expected = [
        (
            "recurrence",
            "f2399d57c335d344b8dbbf3b88227bcf5cefa7dda0384fa0ee46304c1e5a1a06",
        ),
        (
            "threshold",
            "5a7054eeef9b14ef80201bb733ea0bf9a2a45b2848947a5115857e8ca7927099",
        ),
        (
            "reconcile",
            "f5945c88f96f204f0864be5e6a31f8cb81768fc6aded36b74f3f4e4b27c8127f",
        ),
        (
            "reorder",
            "9bee0a57c5d84f8cf8d6283a7ea5c951b6a6e9ca5348e1c35ef063976cb0591b",
        ),
        (
            "phrase_policy",
            "5ca32371f9eb3421a0324ec8f688733746f8bf31a8da397ddfb666947b1e8c2f",
        ),
    ];
    for (kind, digest) in expected {
        assert_eq!(load(kind, 1, BTreeMap::new()).unwrap().digest, digest, "{kind}");
    }
    let parameters: BTreeMap<String, String> = [("interval", "1"), ("frequency", "monthly")]
        .into_iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    assert_eq!(
        load("recurrence", 1, parameters).unwrap().digest,
        "11d57cab910331141bac786f0954d3f929783f325c3b3a98817ee158a432b6cb"
    );
}

fn recurrence_line() -> String {
    let rule = RecurrenceRule {
        frequency: Frequency::Monthly,
        interval: 1,
        day: DaySelector::DayOfMonth(31),
        months: Vec::new(),
        adjustment: Adjustment::PreviousBusinessDay,
        zone: "America/New_York".to_string(),
        anchor: date(2027, 1, 1),
    };
    let calendar = HolidayCalendar {
        revision: "2027-r1".to_string(),
        holidays: [date(2027, 2, 26)].into_iter().collect(),
    };
    let result =
        recurrence::next_due(&rule, &calendar, date(2027, 1, 15), 4, trace("recurrence")).unwrap();
    let dates: Vec<String> = result
        .dates
        .iter()
        .map(|d| format!("{}/{}", d.due, d.scheduled))
        .collect();
    format!(
        "zone={};calendar={};due={};digest={}",
        result.zone,
        result.calendar_revision,
        dates.join(","),
        &result.trace.rule_digest[..16]
    )
}

fn threshold_line() -> String {
    let metrics = [MetricDef {
        name: "revenue".to_string(),
        series: "payments".to_string(),
        aggregate: Aggregate::Sum,
    }];
    let point = |node: &str, day: i64, amount: i64| DataPoint {
        node_id: node.to_string(),
        revision: 1,
        series: "payments".to_string(),
        day,
        amount,
    };
    let points = [
        point("pay:1", 19_990, 6_000_000),
        point("pay:2", 19_800, 5_500_000),
        point("pay:old", 19_635, 9_000_000),
    ];
    let gate = Expr::Compare {
        metric: "revenue".to_string(),
        op: Comparison::GreaterOrEqual,
        value: 10_000_000,
    };
    let result =
        threshold::applies(&gate, &metrics, &points, 365, 20_000, trace("threshold")).unwrap();
    let values: Vec<String> = result
        .metrics
        .iter()
        .map(|m| format!("{}={:?}:{}", m.name, m.value, m.nodes.join("+")))
        .collect();
    format!(
        "applies={:?};{};digest={}",
        result.applies,
        values.join(","),
        &result.trace.rule_digest[..16]
    )
}

fn reconcile_line() -> String {
    let node = |id: &str, amount: i64, day: i64| Node {
        id: id.to_string(),
        revision: 1,
        amount,
        currency: "USD".to_string(),
        day,
    };
    let sets = Sets {
        orders: vec![
            node("order:1", 5_000, 100),
            node("order:2", 3_000, 100),
            node("order:3", 2_000, 101),
        ],
        payments: vec![
            node("pay:1", 5_000, 100),
            node("pay:2", 3_000, 101),
            node("pay:3", 2_000, 101),
        ],
        refunds: vec![Refund {
            id: "refund:1".to_string(),
            revision: 1,
            payment_id: "pay:2".to_string(),
            amount: 500,
        }],
        payouts: vec![node("payout:1", 9_199, 103)],
        fees: vec![Fee {
            id: "fee:1".to_string(),
            revision: 1,
            payout_id: "payout:1".to_string(),
            amount: 300,
        }],
    };
    let params = Params {
        tolerance_minor: 1,
        window_days: 5,
        currency: CurrencyRule::SameCurrency,
    };
    let result = reconcile::match_sets(&sets, &params, trace("reconcile")).unwrap();
    let kind = |k: MatchKind| match k {
        MatchKind::OrderPayment => "OP",
        MatchKind::PaymentPayout => "PP",
    };
    let matches: Vec<String> = result
        .matches
        .iter()
        .map(|m| {
            format!(
                "{} [{}]>[{}] r={} g={}",
                kind(m.kind),
                m.left.join(","),
                m.right.join(","),
                m.residual.minor,
                m.date_gap
            )
        })
        .collect();
    let unmatched: Vec<String> = result
        .unmatched
        .iter()
        .map(|u| format!("{} {} {:?}", kind(u.kind), u.id, u.reason))
        .collect();
    format!(
        "{};unmatched={};ties={};digest={}",
        matches.join("|"),
        unmatched.join(","),
        result.ties.len(),
        &result.trace.rule_digest[..16]
    )
}

fn reorder_line() -> String {
    let sale = |node: &str, day: i64, quantity: i64| Sale {
        node_id: node.to_string(),
        revision: 1,
        day,
        quantity,
    };
    let policy = reorder::Policy {
        lead_time_days: 10,
        safety_stock: 20,
        minimum_order_quantity: 50,
        review_window_days: 30,
        minimum_samples: 3,
    };
    let sales = [
        sale("line:1", 999, 40),
        sale("line:2", 985, 30),
        sale("line:3", 971, 20),
    ];
    let stock = Stock {
        node_id: "stock:s1".to_string(),
        revision: 4,
        on_hand: 10,
        reserved: 3,
    };
    let open = [OpenLine {
        node_id: "pol:1".to_string(),
        revision: 2,
        quantity: 5,
    }];
    let result =
        reorder::reorder_point(&policy, &sales, &stock, &open, 1_000, trace("reorder")).unwrap();
    format!(
        "status={};reorder={};point={:?};position={};quantity={};samples={};digest={}",
        result.status.as_str(),
        result.reorder,
        result.point,
        result.position,
        result.quantity,
        result.samples,
        &result.trace.rule_digest[..16]
    )
}

fn phrase_line() -> String {
    let phrase = |text: &str, mode: Mode| Phrase {
        text: text.to_string(),
        locale: "en".to_string(),
        mode,
    };
    let policy = phrase_policy::Policy {
        revision: "rev-5".to_string(),
        forbidden: vec![
            phrase("cures cancer", Mode::Exact),
            phrase("guaranteed", Mode::Word),
        ],
        approved: vec![phrase("supports healthy skin", Mode::Exact)],
    };
    let result = phrase_policy::check_text(
        "Supports healthy skin. It CURES cancer and is guaranteed.",
        "en",
        &policy,
        trace("phrase_policy"),
    )
    .unwrap();
    let spans: Vec<String> = result
        .forbidden
        .iter()
        .map(|s| format!("{}:{}-{}:{}", s.sentence, s.start, s.end, s.phrase))
        .collect();
    format!(
        "verdict={};spans={};unmatched={};revision={};digest={}",
        result.verdict.as_str(),
        spans.join(","),
        result.unmatched.join("|"),
        result.policy_revision,
        &result.trace.rule_digest[..16]
    )
}

// spec: MEDULLA-BUSINESS-OPS-R002.9
#[test]
fn golden_vectors_match_on_both_architectures() {
    assert_eq!(
        recurrence_line(),
        "zone=America/New_York;calendar=2027-r1;\
         due=2027-01-29/2027-01-31,2027-02-25/2027-02-28,2027-03-31/2027-03-31,2027-04-30/2027-04-30;\
         digest=f2399d57c335d344"
    );
    assert_eq!(
        threshold_line(),
        "applies=Yes;revenue=Some(11500000):pay:1+pay:2;digest=5a7054eeef9b14ef"
    );
    assert_eq!(
        reconcile_line(),
        "OP [order:1]>[pay:1] r=0 g=0|OP [order:3]>[pay:3] r=0 g=0|PP [pay:1,pay:2,pay:3]>[payout:1] r=1 g=7;\
         unmatched=OP order:2 NoMatchWithinTolerance,OP pay:2 NoMatchWithinTolerance;ties=0;\
         digest=f5945c88f96f204f"
    );
    assert_eq!(
        reorder_line(),
        "status=decided;reorder=true;point=Some(50);position=12;quantity=50;samples=3;\
         digest=9bee0a57c5d84f8c"
    );
    assert_eq!(
        phrase_line(),
        "verdict=fail;spans=1:3-15:cures cancer,1:23-33:guaranteed;unmatched=;revision=rev-5;\
         digest=5ca32371f9eb3421"
    );
    // Rendering twice gives the same bytes.
    assert_eq!(recurrence_line(), recurrence_line());
    assert_eq!(reconcile_line(), reconcile_line());
}
