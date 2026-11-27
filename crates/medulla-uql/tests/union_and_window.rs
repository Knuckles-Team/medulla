//! The `UNION` stage and the `VALID AS OF <window>` clause compile into the one plan
//! (MEDULLA-FEDERATED-QUERY-R720.9).
#![cfg(feature = "text")]

use medulla_query_types::wire::{Op, TimeAxis};
use medulla_uql::{parse_statement, parse_statement_at, Body, Params};

/// The query time every parse here is handed: an engine-clock reading, in seconds.
const NOW_SECS: f64 = 1_700_000_000.0;

// proves R720.9: UQL union stage and valid-as-of window
// spec: MEDULLA-FEDERATED-QUERY-R720.9
/// Verifies MEDULLA-FEDERATED-QUERY-R720.9: both clauses lower into one linear plan.
#[test]
fn union_and_valid_as_of_window_compile_into_one_plan() {
    let src = "MATCH (:Incident) |> UNION [MATCH (:Incident)] [MATCH (:Alert)] \
               |> VALID AS OF 1 h |> LIMIT 5";
    let statement = parse_statement_at(src, &Params::new(), NOW_SECS).expect("the program parses");
    let Body::Pipeline(plan) = statement.body else {
        panic!("a union program is one linear plan, not a second plan");
    };
    let unions: Vec<&Op> = plan
        .ops
        .iter()
        .filter(|op| matches!(op, Op::FuseRrf { .. }))
        .collect();
    assert_eq!(unions.len(), 1, "one union stage in the one plan");
    let Op::FuseRrf { branches, .. } = unions[0] else {
        unreachable!()
    };
    assert_eq!(branches.len(), 2, "the union combines both match results");

    let window = plan
        .ops
        .iter()
        .find_map(|op| match op {
            Op::AsOf {
                ts,
                axis: TimeAxis::Valid,
            } => Some(*ts),
            _ => None,
        })
        .expect("the window lowers to a valid-time AsOf");
    assert_eq!(
        window,
        NOW_SECS - 3_600.0,
        "the instant is exactly one hour before the query time"
    );
    assert!(parse_statement_at(
        "MATCH (:A) |> VALID AS OF 1 parsecs",
        &Params::new(),
        NOW_SECS
    )
    .is_err());
    // A parse without a query time refuses the duration form instead of reading a clock.
    assert!(parse_statement("MATCH (:A) |> VALID AS OF 1 h", &Params::new()).is_err());
    assert!(matches!(plan.ops.last(), Some(Op::Limit { .. })));
}
