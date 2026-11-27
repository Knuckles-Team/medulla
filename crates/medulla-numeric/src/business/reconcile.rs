//! Reconciliation matching (MEDULLA-BUSINESS-OPS-R002.4).
//!
//! Kind `reconcile`. Input: three node sets (orders, payments, payouts), an amount
//! tolerance in minor units, a date window in days and a currency rule. Output: matches
//! `order <-> payment` and `payment <-> payout`, including one-to-many and many-to-one,
//! each with its residual; the nodes no match took, each with a reason. A refund nets
//! against its payment and a fee nets against its payout. Ties break by the smaller
//! date gap, then the smaller absolute residual, then the lower node ids; a tie is
//! reported. Integer arithmetic only.

use std::collections::{BTreeMap, BTreeSet};

use super::rule_spec::{InputRef, RuleRefusal, RuleTrace};

/// The refusal code for input larger than [`MAX_NODES`] per set.
pub const RECONCILE_INPUT_TOO_LARGE: &str = "RECONCILE_INPUT_TOO_LARGE";
/// The refusal code for a search that would pass [`WORK_BUDGET`] group candidates.
pub const RECONCILE_WORK_EXCEEDED: &str = "RECONCILE_WORK_EXCEEDED";
/// The refusal code for a negative tolerance or window.
pub const RECONCILE_PARAMS_INVALID: &str = "RECONCILE_PARAMS_INVALID";

/// The most nodes in one input set.
pub const MAX_NODES: usize = 5_000;
/// The most nodes in one side of one match.
pub const MAX_GROUP: usize = 6;
/// The most candidates one search considers for one node.
pub const MAX_POOL: usize = 16;
/// The most group candidates one evaluation tries.
pub const WORK_BUDGET: u64 = 5_000_000;

/// One order, payment or payout, as the rule reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    /// The node id.
    pub id: String,
    /// The revision of the node that was read.
    pub revision: u64,
    /// The amount in minor units.
    pub amount: i64,
    /// The currency code.
    pub currency: String,
    /// The day, in days since 1970-01-01.
    pub day: i64,
}

/// A refund of one payment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refund {
    /// The refund node id.
    pub id: String,
    /// The revision of the node that was read.
    pub revision: u64,
    /// The payment it refunds.
    pub payment_id: String,
    /// The refunded amount in minor units.
    pub amount: i64,
}

/// A fee netted against one payout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fee {
    /// The fee node id.
    pub id: String,
    /// The revision of the node that was read.
    pub revision: u64,
    /// The payout it is netted against.
    pub payout_id: String,
    /// The fee in minor units.
    pub amount: i64,
}

/// The three node sets with their refunds and fees.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sets {
    /// The orders.
    pub orders: Vec<Node>,
    /// The payments.
    pub payments: Vec<Node>,
    /// The refunds of the payments.
    pub refunds: Vec<Refund>,
    /// The payouts, each paying out its payments less its fees.
    pub payouts: Vec<Node>,
    /// The fees of the payouts.
    pub fees: Vec<Fee>,
}

/// Which currencies may match.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CurrencyRule {
    /// Only nodes of one currency match each other.
    SameCurrency,
    /// Only nodes in this currency are considered; the others are unmatched.
    Only(String),
}

/// The rule parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Params {
    /// The most the two sides of a match may differ by, in minor units.
    pub tolerance_minor: i64,
    /// The most days a node may lie from the node it matches.
    pub window_days: i64,
    /// Which currencies may match.
    pub currency: CurrencyRule,
}

/// The kind of a match.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchKind {
    /// Orders matched to payments.
    OrderPayment,
    /// Payments matched to payouts.
    PaymentPayout,
}

/// What a match left over: the left side's total minus the right side's total.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Residual {
    /// Minor units; zero is an exact match.
    pub minor: i128,
}

/// One match: nodes of the left set against nodes of the right set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    /// The kind of match.
    pub kind: MatchKind,
    /// The left nodes (orders, or payments for a payout match), sorted.
    pub left: Vec<String>,
    /// The right nodes (payments, or payouts for a payout match), sorted.
    pub right: Vec<String>,
    /// The left total minus the right total.
    pub residual: Residual,
    /// The sum of the days between each node and the node it was matched from.
    pub date_gap: i64,
}

/// Why a node was left unmatched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// There is nothing on the other side.
    NoCounterpart,
    /// Counterparts exist, but none lies within the date window.
    OutsideWindow,
    /// Counterparts exist, but not in the currency the rule admits.
    CurrencyMismatch,
    /// Counterparts lie within the window, but no group sums to within the tolerance.
    NoMatchWithinTolerance,
}

/// A node no match took.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unmatched {
    /// The kind of match it was looked for in.
    pub kind: MatchKind,
    /// The node id.
    pub id: String,
    /// Why it was left.
    pub reason: Reason,
}

/// A choice between matches no rule could separate: the lower ids were taken.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tie {
    /// The kind of match.
    pub kind: MatchKind,
    /// The node the search started from.
    pub anchor: String,
    /// The group that was taken.
    pub chosen: Vec<String>,
    /// The group that tied with it.
    pub other: Vec<String>,
}

/// The result of one evaluation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reconciliation {
    /// The matches, by kind then by their first left node.
    pub matches: Vec<Match>,
    /// The nodes no match took.
    pub unmatched: Vec<Unmatched>,
    /// The ties that were broken by node id.
    pub ties: Vec<Tie>,
    /// The rule digest, the input nodes and the steps.
    pub trace: RuleTrace,
}

#[derive(Clone, Debug)]
struct Item {
    id: String,
    amount: i128,
    day: i64,
    currency: String,
}

struct Search<'a> {
    params: &'a Params,
    work: u64,
}

/// A candidate group paired with its optional alternative member set.
type GroupChoice = (Group, Option<Vec<usize>>);

struct Group {
    members: Vec<usize>,
    residual: i128,
    gap: i64,
}

fn admits(rule: &CurrencyRule, a: &str, b: &str) -> bool {
    match rule {
        CurrencyRule::SameCurrency => a == b,
        CurrencyRule::Only(code) => a == code && b == code,
    }
}

/// Visit every combination of `k` of `0..n` in lexicographic order.
fn combinations(
    n: usize,
    k: usize,
    visit: &mut impl FnMut(&[usize]) -> Result<(), RuleRefusal>,
) -> Result<(), RuleRefusal> {
    if k == 0 || k > n {
        return Ok(());
    }
    let mut chosen: Vec<usize> = (0..k).collect();
    loop {
        visit(&chosen)?;
        let mut position = k;
        loop {
            if position == 0 {
                return Ok(());
            }
            position -= 1;
            if chosen[position] != position + n - k {
                break;
            }
            if position == 0 {
                return Ok(());
            }
        }
        chosen[position] += 1;
        for next in position + 1..k {
            chosen[next] = chosen[next - 1] + 1;
        }
    }
}

impl Search<'_> {
    /// The best group of `pool` for `anchor`: the smallest group size that has a group
    /// within the tolerance, then the smallest date gap, the smallest absolute residual
    /// and the lowest ids. The second element is a group that tied with the best.
    fn best_group(
        &mut self,
        anchor: &Item,
        anchor_is_left: bool,
        pool: &[&Item],
    ) -> Result<Option<GroupChoice>, RuleRefusal> {
        let tolerance = i128::from(self.params.tolerance_minor);
        for size in 1..=MAX_GROUP.min(pool.len()) {
            let mut best: Option<Group> = None;
            let mut tied: Option<Vec<usize>> = None;
            combinations(pool.len(), size, &mut |indices| {
                self.work += 1;
                if self.work > WORK_BUDGET {
                    return Err(RuleRefusal::new(
                        RECONCILE_WORK_EXCEEDED,
                        format!("more than {WORK_BUDGET} group candidates"),
                    ));
                }
                let total: i128 = indices.iter().map(|&i| pool[i].amount).sum();
                let residual = if anchor_is_left {
                    anchor.amount - total
                } else {
                    total - anchor.amount
                };
                if residual.abs() > tolerance {
                    return Ok(());
                }
                let gap: i64 = indices
                    .iter()
                    .map(|&i| (pool[i].day - anchor.day).abs())
                    .sum();
                let candidate = Group {
                    members: indices.to_vec(),
                    residual,
                    gap,
                };
                let key = |g: &Group| (g.gap, g.residual.abs());
                let ordering = best
                    .as_ref()
                    .map(|current| key(&candidate).cmp(&key(current)));
                match ordering {
                    None | Some(std::cmp::Ordering::Less) => {
                        tied = None;
                        best = Some(candidate);
                    }
                    // Combinations come in lexicographic order of the pool, which is sorted
                    // by gap then id, so the first of equals has the lower ids.
                    Some(std::cmp::Ordering::Equal) => {
                        if tied.is_none() {
                            tied = Some(candidate.members);
                        }
                    }
                    Some(std::cmp::Ordering::Greater) => {}
                }
                Ok(())
            })?;
            if let Some(group) = best {
                return Ok(Some((group, tied)));
            }
        }
        Ok(None)
    }
}

struct Linked {
    matches: Vec<Match>,
    unmatched: Vec<Unmatched>,
    ties: Vec<Tie>,
}

/// Match `lefts` to `rights`: each left node, in day then id order, takes the best group of
/// still-free right nodes; then each right node still free takes the best group of
/// still-free left nodes.
fn link(
    kind: MatchKind,
    lefts: &[Item],
    rights: &[Item],
    search: &mut Search<'_>,
) -> Result<Linked, RuleRefusal> {
    let mut out = Linked {
        matches: Vec::new(),
        unmatched: Vec::new(),
        ties: Vec::new(),
    };
    let mut left_free: BTreeSet<usize> = (0..lefts.len()).collect();
    let mut right_free: BTreeSet<usize> = (0..rights.len()).collect();
    let window = search.params.window_days;
    let rule = search.params.currency.clone();

    for pass in 0..2 {
        let anchor_is_left = pass == 0;
        let (anchors, pool_items, anchor_free, pool_free) = if anchor_is_left {
            (lefts, rights, &mut left_free, &mut right_free)
        } else {
            (rights, lefts, &mut right_free, &mut left_free)
        };
        let mut order: Vec<usize> = anchor_free.iter().copied().collect();
        order.sort_by(|&a, &b| {
            (anchors[a].day, &anchors[a].id).cmp(&(anchors[b].day, &anchors[b].id))
        });
        for anchor_index in order {
            let anchor = &anchors[anchor_index];
            let mut pool: Vec<usize> = pool_free
                .iter()
                .copied()
                .filter(|&i| {
                    admits(&rule, &anchor.currency, &pool_items[i].currency)
                        && (pool_items[i].day - anchor.day).abs() <= window
                })
                .collect();
            pool.sort_by(|&a, &b| {
                let ka = ((pool_items[a].day - anchor.day).abs(), &pool_items[a].id);
                let kb = ((pool_items[b].day - anchor.day).abs(), &pool_items[b].id);
                ka.cmp(&kb)
            });
            pool.truncate(MAX_POOL);
            let candidates: Vec<&Item> = pool.iter().map(|&i| &pool_items[i]).collect();
            let Some((group, tied)) = search.best_group(anchor, anchor_is_left, &candidates)?
            else {
                continue;
            };
            let members: Vec<usize> = group.members.iter().map(|&i| pool[i]).collect();
            let mut member_ids: Vec<String> =
                members.iter().map(|&i| pool_items[i].id.clone()).collect();
            member_ids.sort();
            if let Some(other) = tied {
                let mut other_ids: Vec<String> = other
                    .iter()
                    .map(|&i| pool_items[pool[i]].id.clone())
                    .collect();
                other_ids.sort();
                out.ties.push(Tie {
                    kind,
                    anchor: anchor.id.clone(),
                    chosen: member_ids.clone(),
                    other: other_ids,
                });
            }
            let anchor_ids = vec![anchor.id.clone()];
            let (left_ids, right_ids) = if anchor_is_left {
                (anchor_ids, member_ids)
            } else {
                (member_ids, anchor_ids)
            };
            out.matches.push(Match {
                kind,
                left: left_ids,
                right: right_ids,
                residual: Residual {
                    minor: group.residual,
                },
                date_gap: group.gap,
            });
            anchor_free.remove(&anchor_index);
            for member in members {
                pool_free.remove(&member);
            }
        }
    }

    let reason = |item: &Item, others: &[Item]| -> Reason {
        if others.is_empty() {
            return Reason::NoCounterpart;
        }
        let in_currency: Vec<&Item> = others
            .iter()
            .filter(|o| admits(&rule, &item.currency, &o.currency))
            .collect();
        if in_currency.is_empty() {
            return Reason::CurrencyMismatch;
        }
        if in_currency
            .iter()
            .all(|o| (o.day - item.day).abs() > window)
        {
            return Reason::OutsideWindow;
        }
        Reason::NoMatchWithinTolerance
    };
    for &index in &left_free {
        out.unmatched.push(Unmatched {
            kind,
            id: lefts[index].id.clone(),
            reason: reason(&lefts[index], rights),
        });
    }
    for &index in &right_free {
        out.unmatched.push(Unmatched {
            kind,
            id: rights[index].id.clone(),
            reason: reason(&rights[index], lefts),
        });
    }
    out.matches
        .sort_by(|a, b| (a.kind, &a.left, &a.right).cmp(&(b.kind, &b.left, &b.right)));
    Ok(out)
}

fn check_size(what: &str, len: usize) -> Result<(), RuleRefusal> {
    if len > MAX_NODES {
        return Err(RuleRefusal::new(
            RECONCILE_INPUT_TOO_LARGE,
            format!("{what} holds {len} nodes; the ceiling is {MAX_NODES}"),
        ));
    }
    Ok(())
}

fn item(node: &Node, amount: i128) -> Item {
    Item {
        id: node.id.clone(),
        amount,
        day: node.day,
        currency: node.currency.clone(),
    }
}

/// Match `sets` under `params`. Refunds net against their payment and fees against their
/// payout before matching. `trace` carries the rule digest; every node read is added to its
/// inputs with its revision.
pub fn match_sets(
    sets: &Sets,
    params: &Params,
    mut trace: RuleTrace,
) -> Result<Reconciliation, RuleRefusal> {
    if params.tolerance_minor < 0 || params.window_days < 0 {
        return Err(RuleRefusal::new(
            RECONCILE_PARAMS_INVALID,
            "the tolerance and the window are not negative",
        ));
    }
    check_size("orders", sets.orders.len())?;
    check_size("payments", sets.payments.len())?;
    check_size("payouts", sets.payouts.len())?;
    check_size("refunds", sets.refunds.len())?;
    check_size("fees", sets.fees.len())?;

    let mut refunded: BTreeMap<&str, i128> = BTreeMap::new();
    for refund in &sets.refunds {
        *refunded.entry(refund.payment_id.as_str()).or_default() += i128::from(refund.amount);
        trace
            .inputs
            .push(InputRef::new(refund.id.clone(), refund.revision));
    }
    let mut fees: BTreeMap<&str, i128> = BTreeMap::new();
    for fee in &sets.fees {
        *fees.entry(fee.payout_id.as_str()).or_default() += i128::from(fee.amount);
        trace
            .inputs
            .push(InputRef::new(fee.id.clone(), fee.revision));
    }
    for node in sets
        .orders
        .iter()
        .chain(&sets.payments)
        .chain(&sets.payouts)
    {
        trace
            .inputs
            .push(InputRef::new(node.id.clone(), node.revision));
    }
    trace.inputs.sort();
    trace.inputs.dedup();

    let orders: Vec<Item> = sets
        .orders
        .iter()
        .map(|n| item(n, i128::from(n.amount)))
        .collect();
    let net_payments: Vec<Item> = sets
        .payments
        .iter()
        .map(|n| {
            let net = i128::from(n.amount) - refunded.get(n.id.as_str()).copied().unwrap_or(0);
            item(n, net)
        })
        .collect();
    // A payout settles its payments less its fees: the payments it covers total the payout
    // plus its fees.
    let payouts: Vec<Item> = sets
        .payouts
        .iter()
        .map(|n| {
            let gross = i128::from(n.amount) + fees.get(n.id.as_str()).copied().unwrap_or(0);
            item(n, gross)
        })
        .collect();

    trace.step(format!(
        "{} orders, {} payments ({} refunds), {} payouts ({} fees); tolerance {} window {}",
        orders.len(),
        net_payments.len(),
        sets.refunds.len(),
        payouts.len(),
        sets.fees.len(),
        params.tolerance_minor,
        params.window_days
    ));
    let mut search = Search { params, work: 0 };
    let first = link(MatchKind::OrderPayment, &orders, &net_payments, &mut search)?;
    let second = link(
        MatchKind::PaymentPayout,
        &net_payments,
        &payouts,
        &mut search,
    )?;
    trace.step(format!(
        "{} order matches, {} payout matches, {} group candidates tried",
        first.matches.len(),
        second.matches.len(),
        search.work
    ));
    let mut matches = first.matches;
    matches.extend(second.matches);
    let mut unmatched = first.unmatched;
    unmatched.extend(second.unmatched);
    let mut ties = first.ties;
    ties.extend(second.ties);
    Ok(Reconciliation {
        matches,
        unmatched,
        ties,
        trace,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::business::rule_spec::load;

    fn trace() -> RuleTrace {
        RuleTrace::new(&load("reconcile", 1, BTreeMap::new()).unwrap(), Vec::new())
    }

    fn node(id: &str, amount: i64, day: i64) -> Node {
        Node {
            id: id.to_string(),
            revision: 1,
            amount,
            currency: "USD".to_string(),
            day,
        }
    }

    fn params(tolerance_minor: i64, window_days: i64) -> Params {
        Params {
            tolerance_minor,
            window_days,
            currency: CurrencyRule::SameCurrency,
        }
    }

    fn ids(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    // spec: MEDULLA-BUSINESS-OPS-R002.4
    #[test]
    fn batch_payout_matches_many_payments_within_tolerance() {
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
            // The payout settles the three payments less the refund and a 300 fee, and is
            // short by one minor unit against the batch.
            payouts: vec![node("payout:1", 9_199, 103)],
            fees: vec![Fee {
                id: "fee:1".to_string(),
                revision: 1,
                payout_id: "payout:1".to_string(),
                amount: 300,
            }],
        };
        let result = match_sets(&sets, &params(1, 5), trace()).unwrap();

        // Many payments to one payout, with the refund and the fee netted and the one-unit
        // shortfall as the residual.
        let payout = result
            .matches
            .iter()
            .find(|m| m.kind == MatchKind::PaymentPayout)
            .expect("the payout matched");
        assert_eq!(payout.left, ids(&["pay:1", "pay:2", "pay:3"]));
        assert_eq!(payout.right, ids(&["payout:1"]));
        assert_eq!(payout.residual, Residual { minor: 1 });

        // Orders to payments: pay:2 was refunded 500, so order:2 (3,000) no longer matches it
        // exactly; the net 2,500 matches no order within the tolerance.
        let orders: Vec<&Match> = result
            .matches
            .iter()
            .filter(|m| m.kind == MatchKind::OrderPayment)
            .collect();
        assert!(orders
            .iter()
            .any(|m| m.left == ids(&["order:1"]) && m.right == ids(&["pay:1"])));
        assert!(orders
            .iter()
            .any(|m| m.left == ids(&["order:3"]) && m.right == ids(&["pay:3"])));
        assert!(result
            .unmatched
            .iter()
            .any(|u| u.kind == MatchKind::OrderPayment
                && u.id == "order:2"
                && u.reason == Reason::NoMatchWithinTolerance));
        assert!(result
            .unmatched
            .iter()
            .any(|u| u.kind == MatchKind::OrderPayment
                && u.id == "pay:2"
                && u.reason == Reason::NoMatchWithinTolerance));

        // Every node read is an input with its revision.
        for id in ["order:1", "pay:2", "payout:1", "refund:1", "fee:1"] {
            assert!(result.trace.inputs.contains(&InputRef::new(id, 1)), "{id}");
        }
        assert_eq!(
            result.trace.rule_digest,
            load("reconcile", 1, BTreeMap::new()).unwrap().digest
        );

        // A tolerance of zero leaves the payout unmatched, in its own reason.
        let strict = match_sets(&sets, &params(0, 5), trace()).unwrap();
        assert!(strict
            .matches
            .iter()
            .all(|m| m.kind != MatchKind::PaymentPayout));
        assert!(strict
            .unmatched
            .iter()
            .any(|u| u.kind == MatchKind::PaymentPayout
                && u.id == "payout:1"
                && u.reason == Reason::NoMatchWithinTolerance));

        // The same inputs give the same result.
        assert_eq!(match_sets(&sets, &params(1, 5), trace()).unwrap(), result);
    }

    #[test]
    fn one_payment_many_orders_windows_currencies_and_ties() {
        // One payment covering two orders.
        let sets = Sets {
            orders: vec![node("order:a", 1_000, 10), node("order:b", 2_000, 11)],
            payments: vec![node("pay:x", 3_000, 12)],
            ..Sets::default()
        };
        let result = match_sets(&sets, &params(0, 3), trace()).unwrap();
        assert_eq!(result.matches.len(), 1);
        assert_eq!(result.matches[0].left, ids(&["order:a", "order:b"]));
        assert_eq!(result.matches[0].right, ids(&["pay:x"]));
        assert_eq!(result.matches[0].residual, Residual { minor: 0 });
        assert_eq!(result.matches[0].date_gap, 3);

        // A payment outside the window is unmatched with that reason.
        let far = Sets {
            orders: vec![node("order:a", 1_000, 10)],
            payments: vec![node("pay:x", 1_000, 90)],
            ..Sets::default()
        };
        let result = match_sets(&far, &params(0, 3), trace()).unwrap();
        assert!(result.matches.is_empty());
        assert!(result
            .unmatched
            .iter()
            .filter(|u| u.kind == MatchKind::OrderPayment)
            .all(|u| u.reason == Reason::OutsideWindow));

        // A currency the rule does not admit is unmatched with that reason.
        let mut euro = node("pay:x", 1_000, 10);
        euro.currency = "EUR".to_string();
        let mixed = Sets {
            orders: vec![node("order:a", 1_000, 10)],
            payments: vec![euro],
            ..Sets::default()
        };
        let result = match_sets(&mixed, &params(0, 3), trace()).unwrap();
        assert!(result.matches.is_empty());
        assert!(result
            .unmatched
            .iter()
            .filter(|u| u.kind == MatchKind::OrderPayment)
            .all(|u| u.reason == Reason::CurrencyMismatch));

        // Equal candidates tie: the smaller date gap wins, then the lower id, and the tie
        // is reported when nothing separates them.
        let tied = Sets {
            orders: vec![node("order:a", 1_000, 10)],
            payments: vec![node("pay:2", 1_000, 11), node("pay:1", 1_000, 9)],
            ..Sets::default()
        };
        let result = match_sets(&tied, &params(0, 3), trace()).unwrap();
        assert_eq!(result.matches[0].right, ids(&["pay:1"]));
        assert_eq!(result.ties.len(), 1);
        assert_eq!(result.ties[0].chosen, ids(&["pay:1"]));
        assert_eq!(result.ties[0].other, ids(&["pay:2"]));
        let nearer = Sets {
            orders: vec![node("order:a", 1_000, 10)],
            payments: vec![node("pay:1", 1_000, 12), node("pay:2", 1_000, 11)],
            ..Sets::default()
        };
        let result = match_sets(&nearer, &params(0, 3), trace()).unwrap();
        assert_eq!(result.matches[0].right, ids(&["pay:2"]));
        assert!(result.ties.is_empty());

        // Refusals.
        assert_eq!(
            match_sets(&sets, &params(-1, 3), trace()).unwrap_err().code,
            RECONCILE_PARAMS_INVALID
        );
        let big = Sets {
            orders: (0..=MAX_NODES)
                .map(|i| node(&format!("order:{i}"), 1, 1))
                .collect(),
            ..Sets::default()
        };
        assert_eq!(
            match_sets(&big, &params(0, 1), trace()).unwrap_err().code,
            RECONCILE_INPUT_TOO_LARGE
        );
    }
}
