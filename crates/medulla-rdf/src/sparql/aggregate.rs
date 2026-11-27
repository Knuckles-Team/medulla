use super::*;

// ── GROUP BY + aggregates (CONCEPT:MEDULLA-KG.query.sparql-completeness) ────────────────────────────────────

/// Evaluate `GROUP BY group_vars` + the `aggregates` over `rows`. Returns one
/// solution per distinct group-key, binding each group-by var to its value AND each
/// aggregate-result var (the internal name spargebra assigns) to the computed scalar.
/// The wrapping `Extend` re-binds those internal vars to the user's projected names.
pub(super) fn eval_group(
    ctx: &Ctx,
    rows: Vec<Solution>,
    group_vars: &[spargebra::term::Variable],
    aggregates: &[(spargebra::term::Variable, AggregateExpression)],
) -> Vec<Solution> {
    use std::collections::BTreeMap;

    // Bucket rows by the tuple of group-by values (a stable string key keeps the
    // result deterministic). With no GROUP BY var, ALL rows fall in one "" group.
    let mut groups: BTreeMap<String, Vec<Solution>> = BTreeMap::new();
    for row in rows {
        let key = group_vars
            .iter()
            .map(|v| {
                row.get(v.as_str())
                    .map(Binding::to_ntriples)
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join("\u{1f}");
        groups.entry(key).or_default().push(row);
    }

    let mut out = Vec::new();
    for (_key, members) in groups {
        let mut sol = Solution::new();
        // Carry the group-by var values (taken from the first member of the group).
        if let Some(first) = members.first() {
            for gv in group_vars {
                if let Some(b) = first.get(gv.as_str()) {
                    sol.insert(gv.as_str().to_string(), b.clone());
                }
            }
        }
        // Compute each aggregate over the group's members.
        for (out_var, agg) in aggregates {
            if let Some(value) = compute_aggregate(ctx, agg, &members) {
                sol.insert(out_var.as_str().to_string(), value);
            }
        }
        out.push(sol);
    }
    out
}

/// Compute ONE aggregate over a group's member solutions, returning its typed term. An
/// aggregate with no value (`MIN` of nothing, an unsupported custom aggregate) is unbound.
pub(super) fn compute_aggregate(
    ctx: &Ctx,
    agg: &AggregateExpression,
    members: &[Solution],
) -> Option<Binding> {
    match agg {
        // COUNT(*) — count solutions (DISTINCT counts distinct whole solutions).
        AggregateExpression::CountSolutions { distinct } => {
            let n = if *distinct {
                let mut seen = std::collections::HashSet::new();
                members
                    .iter()
                    .filter(|s| seen.insert(canonical_solution(s)))
                    .count()
            } else {
                members.len()
            };
            Some(Binding::integer(n as i64))
        }
        AggregateExpression::FunctionCall {
            name,
            expr,
            distinct,
        } => {
            // The per-row terms of the aggregated expression (skipping unbound rows).
            let mut vals: Vec<Binding> = members
                .iter()
                .filter_map(|s| eval_term(ctx, expr, s))
                .collect();
            if *distinct {
                let mut seen = std::collections::HashSet::new();
                vals.retain(|v| seen.insert(v.to_ntriples()));
            }
            agg_over(name, &vals)
        }
    }
}

/// Apply an aggregate function to the already-collected per-row terms.
pub(super) fn agg_over(func: &AggregateFunction, vals: &[Binding]) -> Option<Binding> {
    let nums: Vec<Numeric> = vals.iter().filter_map(Binding::numeric).collect();
    match func {
        AggregateFunction::Count => Some(Binding::integer(vals.len() as i64)),
        AggregateFunction::Sum => Some(agg_sum(&nums).into_binding()),
        AggregateFunction::Avg => Some(agg_avg(&nums).into_binding()),
        AggregateFunction::Min => agg_extreme(vals, Extreme::Min),
        AggregateFunction::Max => agg_extreme(vals, Extreme::Max),
        AggregateFunction::GroupConcat { separator } => {
            let texts: Vec<String> = vals.iter().filter_map(Binding::str_value).collect();
            Some(Binding::string(
                texts.join(separator.as_deref().unwrap_or(" ")),
            ))
        }
        AggregateFunction::Sample => vals.first().cloned(),
        AggregateFunction::Custom(_) => None,
    }
}

/// SUM over the numeric values under type promotion; `0` (an integer) when there are none.
pub(super) fn agg_sum(nums: &[Numeric]) -> Numeric {
    nums.iter().fold(Numeric::Integer(0), |acc, n| {
        acc.arith(ArithOp::Add, *n).unwrap_or(acc)
    })
}

/// AVG over the numeric values: the sum divided by the count; `0` when there are none.
pub(super) fn agg_avg(nums: &[Numeric]) -> Numeric {
    if nums.is_empty() {
        return Numeric::Integer(0);
    }
    agg_sum(nums)
        .arith(ArithOp::Div, Numeric::Integer(nums.len() as i64))
        .unwrap_or(Numeric::Integer(0))
}

/// Which end of the ordering a MIN/MAX aggregate takes.
#[derive(Clone, Copy)]
pub(super) enum Extreme {
    Min,
    Max,
}

/// MIN/MAX under the SPARQL term ordering (the one `ORDER BY` uses).
pub(super) fn agg_extreme(vals: &[Binding], end: Extreme) -> Option<Binding> {
    vals.iter().cloned().reduce(|best, next| {
        let order = cmp_binding(&Some(next.clone()), &Some(best.clone()));
        match (end, order) {
            (Extreme::Min, std::cmp::Ordering::Less)
            | (Extreme::Max, std::cmp::Ordering::Greater) => next,
            _ => best,
        }
    })
}
