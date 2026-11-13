//! Ordering of the tool steps of a connector usage guidance answer
//! (MEDULLA-DECISION-ENGINE-R177.2).
//!
//! Pure and deterministic: no I/O, no clock, no hash-map iteration order. The
//! steps are the `:TaskStep` objects of one job class and the `:precedes`
//! edges between them. A step always follows every step that precedes it. Two
//! steps with no `:precedes` path between them may swap, and then the pooled
//! success rate of their tools decides, but only when both tools have at least
//! `usage.min_trials` outcomes; otherwise the lower tool component id wins.

use std::collections::{BTreeMap, BTreeSet};

/// One step of a job class: its id, the component id of its tool and the ids
/// of the steps it directly precedes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanStep {
    pub step_id: String,
    pub tool: String,
    pub precedes: Vec<String>,
}

/// The pooled outcomes of one tool: option id equal to the tool component id
/// in the `aggregate_log` rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolRate {
    pub successes: u64,
    pub trials: u64,
}

/// The registry knobs the ordering reads (`usage.min_trials`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageKnobs {
    pub min_trials: u64,
}

/// Why the steps cannot be ordered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderError {
    /// The `:precedes` edges form a cycle; the ids are the steps on it.
    Cyclic(Vec<String>),
    /// Two steps carry the same id.
    DuplicateStep(String),
}

/// The ordered step ids, and whether a success rate decided any placement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepOrder {
    pub steps: Vec<String>,
    pub rate_ordered: bool,
}

/// How `left` ranks against `right` among steps that may swap: `Less` puts
/// `left` first. The second value is true when a rate decided it.
fn compare(
    left: &PlanStep,
    right: &PlanStep,
    rates: &BTreeMap<String, ToolRate>,
    knobs: &UsageKnobs,
) -> (std::cmp::Ordering, bool) {
    let usable = |step: &PlanStep| {
        rates
            .get(&step.tool)
            .filter(|r| r.trials >= knobs.min_trials && r.trials > 0)
    };
    if let (Some(a), Some(b)) = (usable(left), usable(right)) {
        // a.successes / a.trials versus b.successes / b.trials, exactly.
        let a_side = u128::from(a.successes) * u128::from(b.trials);
        let b_side = u128::from(b.successes) * u128::from(a.trials);
        if a_side != b_side {
            return (b_side.cmp(&a_side), true);
        }
    }
    let by_id = left
        .tool
        .cmp(&right.tool)
        .then_with(|| left.step_id.cmp(&right.step_id));
    (by_id, false)
}

/// Step ids that lie on a cycle among `remaining`.
fn cycle_members(remaining: &BTreeSet<&str>, edges: &BTreeMap<&str, Vec<&str>>) -> Vec<String> {
    let reaches_itself = |start: &str| {
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        let mut stack: Vec<&str> = edges.get(start).cloned().unwrap_or_default();
        while let Some(next) = stack.pop() {
            if next == start {
                return true;
            }
            if remaining.contains(next) && seen.insert(next) {
                stack.extend(edges.get(next).into_iter().flatten().copied());
            }
        }
        false
    };
    remaining
        .iter()
        .filter(|id| reaches_itself(id))
        .map(|id| (*id).to_string())
        .collect()
}

/// Order `steps` by their `:precedes` edges, breaking ties between steps that
/// may swap by the pooled success rate of their tools, then by tool id.
///
/// An edge naming a step that is not in `steps` is ignored: the step belongs
/// to another job class.
pub fn order_steps(
    steps: &[PlanStep],
    rates: &BTreeMap<String, ToolRate>,
    knobs: &UsageKnobs,
) -> Result<StepOrder, OrderError> {
    let mut by_id: BTreeMap<&str, &PlanStep> = BTreeMap::new();
    for step in steps {
        if by_id.insert(step.step_id.as_str(), step).is_some() {
            return Err(OrderError::DuplicateStep(step.step_id.clone()));
        }
    }
    let mut edges: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut waiting_on: BTreeMap<&str, usize> = by_id.keys().map(|id| (*id, 0)).collect();
    for step in by_id.values() {
        let targets: BTreeSet<&str> = step
            .precedes
            .iter()
            .map(String::as_str)
            .filter(|t| by_id.contains_key(t))
            .collect();
        for target in &targets {
            if let Some(count) = waiting_on.get_mut(target) {
                *count += 1;
            }
        }
        edges.insert(step.step_id.as_str(), targets.into_iter().collect());
    }

    let mut ordered = Vec::with_capacity(steps.len());
    let mut rate_ordered = false;
    let mut remaining: BTreeSet<&str> = by_id.keys().copied().collect();
    while !remaining.is_empty() {
        // The ready set, in a canonical order that does not depend on the
        // order the caller listed the steps in.
        let mut ready: Vec<&PlanStep> = remaining
            .iter()
            .filter(|id| waiting_on.get(*id).copied() == Some(0))
            .filter_map(|id| by_id.get(id).copied())
            .collect();
        if ready.is_empty() {
            return Err(OrderError::Cyclic(cycle_members(&remaining, &edges)));
        }
        ready.sort_by(|a, b| a.tool.cmp(&b.tool).then_with(|| a.step_id.cmp(&b.step_id)));
        let mut best = ready[0];
        let mut decided_by_rate = false;
        for candidate in &ready[1..] {
            let (order, by_rate) = compare(candidate, best, rates, knobs);
            if order == std::cmp::Ordering::Less {
                best = candidate;
                decided_by_rate = by_rate;
            } else if by_rate {
                decided_by_rate = true;
            }
        }
        rate_ordered |= decided_by_rate && ready.len() > 1;
        remaining.remove(best.step_id.as_str());
        for next in edges.get(best.step_id.as_str()).into_iter().flatten() {
            if let Some(count) = waiting_on.get_mut(next) {
                *count -= 1;
            }
        }
        ordered.push(best.step_id.clone());
    }
    Ok(StepOrder {
        steps: ordered,
        rate_ordered,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const KNOBS: UsageKnobs = UsageKnobs { min_trials: 5 };

    fn step(id: &str, tool: &str, precedes: &[&str]) -> PlanStep {
        PlanStep {
            step_id: id.into(),
            tool: tool.into(),
            precedes: precedes.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    fn rate(successes: u64, trials: u64) -> ToolRate {
        ToolRate { successes, trials }
    }

    fn fixture() -> Vec<PlanStep> {
        vec![
            step("open", "tool.open", &["a", "b"]),
            step("a", "tool.a", &["close"]),
            step("b", "tool.b", &["close"]),
            step("close", "tool.close", &[]),
        ]
    }

    // spec: MEDULLA-DECISION-ENGINE-R177.2
    #[test]
    fn steps_ordered_by_outcome() {
        // Missing rates fall back to the lower tool id: a before b.
        let none = BTreeMap::new();
        let base = order_steps(&fixture(), &none, &KNOBS).unwrap();
        assert_eq!(base.steps, ["open", "a", "b", "close"]);
        assert!(!base.rate_ordered);

        // Too few trials on one side is the same as no rate.
        let thin: BTreeMap<_, _> = [
            ("tool.a".to_string(), rate(1, 10)),
            ("tool.b".to_string(), rate(4, 4)),
        ]
        .into();
        let thin_order = order_steps(&fixture(), &thin, &KNOBS).unwrap();
        assert_eq!(thin_order.steps, ["open", "a", "b", "close"]);
        assert!(!thin_order.rate_ordered);

        // With enough trials on both, the higher success rate breaks the tie.
        let rated: BTreeMap<_, _> = [
            ("tool.a".to_string(), rate(2, 10)),
            ("tool.b".to_string(), rate(9, 10)),
        ]
        .into();
        let order = order_steps(&fixture(), &rated, &KNOBS).unwrap();
        assert_eq!(order.steps, ["open", "b", "a", "close"]);
        assert!(order.rate_ordered);

        // Equal rates are a tie again, and the lower tool id goes first.
        let equal: BTreeMap<_, _> = [
            ("tool.a".to_string(), rate(5, 10)),
            ("tool.b".to_string(), rate(50, 100)),
        ]
        .into();
        let tied = order_steps(&fixture(), &equal, &KNOBS).unwrap();
        assert_eq!(tied.steps, ["open", "a", "b", "close"]);
        assert!(!tied.rate_ordered);

        // A rate never moves a step ahead of one that must precede it.
        let chain = vec![step("one", "tool.z", &["two"]), step("two", "tool.a", &[])];
        let chained: BTreeMap<_, _> = [
            ("tool.z".to_string(), rate(0, 50)),
            ("tool.a".to_string(), rate(50, 50)),
        ]
        .into();
        assert_eq!(
            order_steps(&chain, &chained, &KNOBS).unwrap().steps,
            ["one", "two"]
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R177.2
    #[test]
    fn a_cycle_names_its_steps_and_a_duplicate_is_refused() {
        let cyclic = vec![
            step("x", "t.x", &["y"]),
            step("y", "t.y", &["z"]),
            step("z", "t.z", &["y"]),
        ];
        assert_eq!(
            order_steps(&cyclic, &BTreeMap::new(), &KNOBS),
            Err(OrderError::Cyclic(vec!["y".into(), "z".into()]))
        );
        let twice = vec![step("x", "t.x", &[]), step("x", "t.y", &[])];
        assert_eq!(
            order_steps(&twice, &BTreeMap::new(), &KNOBS),
            Err(OrderError::DuplicateStep("x".into()))
        );
    }

    fn permutations(items: &[PlanStep]) -> Vec<Vec<PlanStep>> {
        if items.len() <= 1 {
            return vec![items.to_vec()];
        }
        let mut out = Vec::new();
        for i in 0..items.len() {
            let mut rest = items.to_vec();
            let head = rest.remove(i);
            for mut tail in permutations(&rest) {
                tail.insert(0, head.clone());
                out.push(tail);
            }
        }
        out
    }

    // spec: MEDULLA-DECISION-ENGINE-R177.2
    #[test]
    fn order_is_deterministic() {
        let rates: BTreeMap<_, _> = [
            ("tool.a".to_string(), rate(2, 10)),
            ("tool.b".to_string(), rate(9, 10)),
        ]
        .into();
        let expected = order_steps(&fixture(), &rates, &KNOBS).unwrap();
        let all = permutations(&fixture());
        assert_eq!(all.len(), 24);
        for shuffled in all {
            assert_eq!(order_steps(&shuffled, &rates, &KNOBS).unwrap(), expected);
        }
    }
}
