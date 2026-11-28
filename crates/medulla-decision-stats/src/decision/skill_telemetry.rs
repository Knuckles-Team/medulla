//! Pure skill-telemetry kernels (MEDULLA-DECISION-ENGINE-R713.2, R713.5, R713.6).
//!
//! No I/O and no clock. [`measures`] sums the invocations of one skill over a window into the
//! twelve measures; [`quality_ppm`] and [`rank_skills`] order skills by their evaluations;
//! [`peer_skills`] lists what a cohort used and the caller did not, without naming a peer. All
//! shares are integer parts per million and every percentile is nearest-rank.

use std::collections::{BTreeMap, BTreeSet};

use medulla_types::refusal_code::RefusalCode;
use {
    medulla_decision_statistical_types::skill_telemetry::InvocationOutcome,
    medulla_decision_statistical_types::skill_telemetry::PeerSkill,
    medulla_decision_statistical_types::skill_telemetry::RankedSkill,
    medulla_decision_statistical_types::skill_telemetry::SkillInvocation,
    medulla_decision_statistical_types::skill_telemetry::SkillMeasures,
    medulla_decision_statistical_types::skill_telemetry::Window,
    medulla_decision_statistical_types::skill_telemetry::PEER_COHORT_TOO_SMALL,
    medulla_decision_statistical_types::skill_telemetry::RETENTION_WINDOW_MS,
};

const PPM: u64 = 1_000_000;

/// The nearest-rank percentile of an ascending `sorted` slice: the value at rank
/// `ceil(percent * n / 100)`. `None` for an empty slice.
pub fn nearest_rank<T: Copy>(sorted: &[T], percent: u64) -> Option<T> {
    if sorted.is_empty() {
        return None;
    }
    let n = sorted.len() as u64;
    let rank = (percent * n).div_ceil(100).clamp(1, n);
    Some(sorted[(rank - 1) as usize])
}

fn share(part: u64, whole: u64) -> u64 {
    if whole == 0 {
        0
    } else {
        (u128::from(part) * u128::from(PPM) / u128::from(whole)) as u64
    }
}

/// The twelve measures of `invocations` inside `window`. `eval` is the pass rate in ppm and the
/// case count of the skill's latest suite (the `SuiteReport` of R711), when it has one. A window
/// with no invocation answers zero counts and no percentile.
pub fn measures(
    invocations: &[SkillInvocation],
    eval: Option<(u64, u64)>,
    window: Window,
) -> SkillMeasures {
    let inside: Vec<&SkillInvocation> = invocations
        .iter()
        .filter(|row| window.contains(row.started_ms))
        .collect();
    let (eval_pass_rate_ppm, eval_runs) = eval.unwrap_or((0, 0));
    if inside.is_empty() {
        return SkillMeasures {
            eval_pass_rate_ppm,
            eval_runs,
            ..SkillMeasures::default()
        };
    }
    let total = inside.len() as u64;
    let users: BTreeSet<&str> = inside
        .iter()
        .map(|row| row.principal_pseudonym.as_str())
        .collect();
    let first: BTreeSet<&str> = inside
        .iter()
        .filter(|row| row.started_ms < window.start_ms.saturating_add(RETENTION_WINDOW_MS))
        .map(|row| row.principal_pseudonym.as_str())
        .collect();
    let last: BTreeSet<&str> = inside
        .iter()
        .filter(|row| row.started_ms >= window.end_ms.saturating_sub(RETENTION_WINDOW_MS))
        .map(|row| row.principal_pseudonym.as_str())
        .collect();
    let retained = first.iter().filter(|user| last.contains(*user)).count() as u64;
    let agent = inside
        .iter()
        .filter(|row| {
            row.chosen_by == medulla_decision_statistical_types::skill_telemetry::ChosenBy::Agent
        })
        .count() as u64;
    let errors = inside
        .iter()
        .filter(|row| matches!(row.outcome, InvocationOutcome::Error { .. }))
        .count() as u64;
    let mut latencies: Vec<u64> = inside
        .iter()
        .map(|row| row.ended_ms.saturating_sub(row.started_ms))
        .collect();
    latencies.sort_unstable();
    let mut costs: Vec<i64> = inside
        .iter()
        .filter_map(|row| row.cost_delta_micro_usd)
        .collect();
    costs.sort_unstable();
    let cost_mean = (!costs.is_empty()).then(|| {
        let sum: i128 = costs.iter().map(|cost| i128::from(*cost)).sum();
        i64::try_from(sum.div_euclid(costs.len() as i128)).unwrap_or(i64::MAX)
    });
    SkillMeasures {
        invocations: total,
        distinct_users: users.len() as u64,
        retained_users_ppm: share(retained, first.len() as u64),
        agent_chosen_ppm: share(agent, total),
        user_chosen_ppm: share(total - agent, total),
        error_rate_ppm: share(errors, total),
        latency_p50_ms: nearest_rank(&latencies, 50),
        latency_p95_ms: nearest_rank(&latencies, 95),
        cost_mean_micro_usd: cost_mean,
        cost_p95_micro_usd: nearest_rank(&costs, 95),
        eval_pass_rate_ppm,
        eval_runs,
    }
}

/// `eval_pass_rate_ppm * eval_runs / (eval_runs + prior_runs)` (MEDULLA-DECISION-ENGINE-R713.5):
/// a pass rate over few runs is pulled toward zero by the prior weight.
pub fn quality_ppm(eval_pass_rate_ppm: u64, eval_runs: u64, prior_runs: u64) -> u64 {
    let denominator = u128::from(eval_runs) + u128::from(prior_runs);
    if denominator == 0 {
        return 0;
    }
    (u128::from(eval_pass_rate_ppm) * u128::from(eval_runs) / denominator) as u64
}

/// One skill's measures with its reference, the input of [`rank_skills`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RankInput {
    /// The skill revision.
    pub skill_ref: String,
    /// Its measures over the window.
    pub measures: SkillMeasures,
}

/// Skills by quality descending, then eval runs descending, then invocations descending, then
/// `skill_ref`. A skill with no eval run ranks below every skill with eval runs.
pub fn rank_skills(inputs: &[RankInput], prior_runs: u64) -> Vec<RankedSkill> {
    let mut ranked: Vec<RankedSkill> = inputs
        .iter()
        .map(|input| RankedSkill {
            skill_ref: input.skill_ref.clone(),
            quality_ppm: quality_ppm(
                input.measures.eval_pass_rate_ppm,
                input.measures.eval_runs,
                prior_runs,
            ),
            eval_runs: input.measures.eval_runs,
            invocations: input.measures.invocations,
        })
        .collect();
    ranked.sort_by(|a, b| {
        (a.eval_runs == 0)
            .cmp(&(b.eval_runs == 0))
            .then_with(|| b.quality_ppm.cmp(&a.quality_ppm))
            .then_with(|| b.eval_runs.cmp(&a.eval_runs))
            .then_with(|| b.invocations.cmp(&a.invocations))
            .then_with(|| a.skill_ref.cmp(&b.skill_ref))
    });
    ranked
}

/// The cohort of `caller` (MEDULLA-DECISION-ENGINE-R713.6): every other principal that used, in
/// the window, at least one skill revision of the class the caller used. `uses` are the
/// `(principal_pseudonym, skill_ref)` pairs of the window. The engine computes the cohort from its
/// own telemetry; the caller never supplies one.
pub fn cohort_of(uses: &[(String, String)], caller: &str) -> BTreeSet<String> {
    let class: BTreeSet<&str> = uses
        .iter()
        .filter(|(user, _)| user == caller)
        .map(|(_, skill)| skill.as_str())
        .collect();
    uses.iter()
        .filter(|(user, skill)| user != caller && class.contains(skill.as_str()))
        .map(|(user, _)| user.clone())
        .collect()
}

/// Skills that at least `min_peers` principals of `cohort` used and `caller` did not, by distinct
/// cohort users descending then `skill_ref` (MEDULLA-DECISION-ENGINE-R713.6). `uses` are the
/// `(principal_pseudonym, skill_ref)` pairs of the window. A cohort of fewer than `min_peers`
/// principals other than the caller refuses `PEER_COHORT_TOO_SMALL`; the answer holds counts and
/// skills, never a pseudonym.
pub fn peer_skills(
    uses: &[(String, String)],
    cohort: &BTreeSet<String>,
    caller: &str,
    min_peers: u64,
) -> Result<Vec<PeerSkill>, RefusalCode> {
    let peers: BTreeSet<&String> = cohort
        .iter()
        .filter(|user| user.as_str() != caller)
        .collect();
    if (peers.len() as u64) < min_peers {
        return Err(PEER_COHORT_TOO_SMALL);
    }
    let caller_used: BTreeSet<&str> = uses
        .iter()
        .filter(|(user, _)| user == caller)
        .map(|(_, skill)| skill.as_str())
        .collect();
    let mut users_of: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (user, skill) in uses {
        if peers.contains(user) && !caller_used.contains(skill.as_str()) {
            users_of
                .entry(skill.as_str())
                .or_default()
                .insert(user.as_str());
        }
    }
    let mut skills: Vec<PeerSkill> = users_of
        .into_iter()
        .filter(|(_, users)| users.len() as u64 >= min_peers)
        .map(|(skill, users)| PeerSkill {
            skill_ref: skill.to_string(),
            cohort_users: users.len() as u64,
        })
        .collect();
    skills.sort_by(|a, b| {
        b.cohort_users
            .cmp(&a.cohort_users)
            .then_with(|| a.skill_ref.cmp(&b.skill_ref))
    });
    Ok(skills)
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_decision_statistical_types::skill_telemetry::ChosenBy;

    const DAY: u64 = 24 * 60 * 60 * 1000;

    fn call(
        user: &str,
        day: u64,
        chosen_by: ChosenBy,
        latency: u64,
        error: bool,
        cost: Option<i64>,
    ) -> SkillInvocation {
        SkillInvocation {
            invocation_id: format!("{user}-{day}-{latency}"),
            tenant_id: "t".into(),
            skill_ref: "skill:a@1".into(),
            skill_version: "1".into(),
            chosen_by,
            principal_pseudonym: user.into(),
            run_id: "r".into(),
            started_ms: day * DAY,
            ended_ms: day * DAY + latency,
            outcome: if error {
                InvocationOutcome::Error {
                    code: "TOOL_TIMEOUT".into(),
                }
            } else {
                InvocationOutcome::Ok
            },
            cost_delta_micro_usd: cost,
            tokens_in: 1,
            tokens_out: 1,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R713.2
    #[test]
    fn twelve_measures_match_golden_vectors() {
        let window = Window {
            start_ms: 0,
            end_ms: 30 * DAY,
        };
        let rows = vec![
            call("u1", 1, ChosenBy::Agent, 100, false, Some(10)),
            call("u1", 29, ChosenBy::User, 200, false, Some(20)),
            call("u2", 2, ChosenBy::Agent, 300, true, None),
            call("u3", 20, ChosenBy::Agent, 400, false, Some(30)),
            call("u3", 29, ChosenBy::User, 500, false, Some(-5)),
            call("u4", 40, ChosenBy::User, 900, false, None), // outside the window
        ];
        let got = measures(&rows, Some((800_000, 40)), window);
        assert_eq!(
            got,
            SkillMeasures {
                invocations: 5,
                distinct_users: 3,
                // u1 and u2 were active in the first 14 days; only u1 is active in the last 14.
                retained_users_ppm: 500_000,
                agent_chosen_ppm: 600_000,
                user_chosen_ppm: 400_000,
                error_rate_ppm: 200_000,
                // Nearest rank over [100, 200, 300, 400, 500]: rank 3 and rank 5.
                latency_p50_ms: Some(300),
                latency_p95_ms: Some(500),
                // Over [-5, 10, 20, 30]: mean 13.75 rounds down to 13; rank 4 is 30.
                cost_mean_micro_usd: Some(13),
                cost_p95_micro_usd: Some(30),
                eval_pass_rate_ppm: 800_000,
                eval_runs: 40,
            }
        );
        // A window with no invocation: zero counts and no percentile.
        let empty = measures(
            &rows,
            None,
            Window {
                start_ms: 100 * DAY,
                end_ms: 130 * DAY,
            },
        );
        assert_eq!(empty, SkillMeasures::default());
        assert_eq!(empty.latency_p50_ms, None);
        assert_eq!(nearest_rank::<u64>(&[], 50), None);
        assert_eq!(nearest_rank(&[7], 95), Some(7));
    }

    // spec: MEDULLA-DECISION-ENGINE-R713.5
    #[test]
    fn quality_orders_by_evals_then_runs() {
        assert_eq!(quality_ppm(1_000_000, 20, 20), 500_000);
        assert_eq!(quality_ppm(900_000, 0, 20), 0);
        let input = |skill: &str, pass: u64, runs: u64, calls: u64| RankInput {
            skill_ref: skill.into(),
            measures: SkillMeasures {
                invocations: calls,
                eval_pass_rate_ppm: pass,
                eval_runs: runs,
                ..SkillMeasures::default()
            },
        };
        let ranked = rank_skills(
            &[
                input("c", 0, 0, 900),
                input("b", 1_000_000, 20, 5),
                input("a", 1_000_000, 80, 1),
                input("d", 0, 10, 1),
            ],
            20,
        );
        let order: Vec<&str> = ranked.iter().map(|row| row.skill_ref.as_str()).collect();
        // a: 800000; b: 500000; d: pass 0 with runs; c: no runs, last.
        assert_eq!(order, ["a", "b", "d", "c"]);
    }

    // spec: MEDULLA-DECISION-ENGINE-R713.6
    #[test]
    fn the_cohort_is_computed_from_the_callers_class() {
        let uses: Vec<(String, String)> = [
            ("me", "s1"),
            ("p1", "s1"),
            ("p2", "s1"),
            ("p2", "s9"),
            ("q", "s9"),
        ]
        .iter()
        .map(|(u, s)| (u.to_string(), s.to_string()))
        .collect();
        let cohort = cohort_of(&uses, "me");
        assert_eq!(
            cohort,
            ["p1", "p2"]
                .iter()
                .map(|u| u.to_string())
                .collect::<BTreeSet<_>>()
        );
        // A caller with no telemetry has an empty cohort.
        assert!(cohort_of(&uses, "nobody").is_empty());
    }

    // spec: MEDULLA-DECISION-ENGINE-R713.6
    #[test]
    fn peers_are_counted_not_named() {
        let uses: Vec<(String, String)> = [
            ("me", "s1"),
            ("p1", "s1"),
            ("p1", "s2"),
            ("p2", "s2"),
            ("p3", "s2"),
            ("p3", "s3"),
            ("outsider", "s2"),
        ]
        .iter()
        .map(|(u, s)| (u.to_string(), s.to_string()))
        .collect();
        let cohort: BTreeSet<String> = ["me", "p1", "p2", "p3"]
            .iter()
            .map(|u| u.to_string())
            .collect();
        // Three peers use s2 and the caller does not; s1 is the caller's; s3 has one user.
        assert_eq!(
            peer_skills(&uses, &cohort, "me", 3).unwrap(),
            vec![PeerSkill {
                skill_ref: "s2".into(),
                cohort_users: 3
            }]
        );
        // A cohort of two peers is too small for a floor of three.
        let small: BTreeSet<String> = ["me", "p1", "p2"].iter().map(|u| u.to_string()).collect();
        assert_eq!(
            peer_skills(&uses, &small, "me", 3),
            Err(PEER_COHORT_TOO_SMALL)
        );
    }
}
