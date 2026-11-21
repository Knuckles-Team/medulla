//! Team scoring (MEDULLA-ORCH-VOCAB-R751.751): a shrunk mean, a closed form
//! with no iteration.
//!
//! A team's score is the mean of its own recorded outcomes pulled toward the
//! prior, which is the mean over every outcome of every team in the same
//! snapshot. `prior_strength` is the weight of the prior as a count of
//! outcomes (`knob team_score.prior_strength`). All arithmetic is `u64`
//! integer arithmetic rounded down, so the result is deterministic and does
//! not depend on the order of the outcomes.

use medulla_work_types::orchestration::teams::{PopulationOutcome, TeamConfig};

/// The prior of a snapshot that holds no outcome, in parts per million.
pub const EMPTY_SNAPSHOT_PRIOR_PPM: u32 = 500_000;

/// One scored team.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamScore {
    pub team_id: String,
    pub score_ppm: u32,
    /// The number of the team's own outcomes the score is computed over.
    pub samples: u32,
}

/// `(prior_ppm * prior_strength + sum(outcomes)) / (prior_strength + outcomes.len())`,
/// rounded down; `prior_ppm` when there is no outcome.
pub fn team_score(prior_ppm: u32, outcomes: &[u32], prior_strength: u32) -> u32 {
    if outcomes.is_empty() {
        return prior_ppm;
    }
    let weight = u64::from(prior_strength);
    let sum: u64 = outcomes.iter().map(|outcome| u64::from(*outcome)).sum();
    let numerator = u64::from(prior_ppm) * weight + sum;
    let denominator = weight + outcomes.len() as u64;
    // A mean of `u32` values is at most `u32::MAX`.
    u32::try_from(numerator / denominator).unwrap_or(u32::MAX)
}

/// The mean `outcome_ppm` over every outcome of every team in the snapshot;
/// [`EMPTY_SNAPSHOT_PRIOR_PPM`] when it holds none.
fn snapshot_prior_ppm(outcomes: &[PopulationOutcome]) -> u32 {
    if outcomes.is_empty() {
        return EMPTY_SNAPSHOT_PRIOR_PPM;
    }
    let sum: u64 = outcomes.iter().map(|o| u64::from(o.outcome_ppm)).sum();
    u32::try_from(sum / outcomes.len() as u64).unwrap_or(u32::MAX)
}

/// Every team scored, by `score_ppm` descending then `team_id` ascending.
pub fn rank_teams(
    teams: &[TeamConfig],
    outcomes: &[PopulationOutcome],
    prior_strength: u32,
) -> Vec<TeamScore> {
    let prior_ppm = snapshot_prior_ppm(outcomes);
    let mut scored: Vec<TeamScore> = teams
        .iter()
        .map(|team| {
            let own: Vec<u32> = outcomes
                .iter()
                .filter(|o| o.team_id == team.team_id)
                .map(|o| o.outcome_ppm)
                .collect();
            TeamScore {
                team_id: team.team_id.clone(),
                score_ppm: team_score(prior_ppm, &own, prior_strength),
                samples: u32::try_from(own.len()).unwrap_or(u32::MAX),
            }
        })
        .collect();
    scored.sort_by(|a, b| {
        b.score_ppm
            .cmp(&a.score_ppm)
            .then_with(|| a.team_id.cmp(&b.team_id))
    });
    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(team: &str, trace: &str, ppm: u32) -> PopulationOutcome {
        PopulationOutcome {
            team_id: team.into(),
            trace_id: trace.into(),
            outcome_ppm: ppm,
        }
    }

    // spec: MEDULLA-ORCH-VOCAB-R751.751
    #[test]
    fn shrunk_mean_is_deterministic_and_order_free() {
        // The closed form: (500_000 * 5 + 900_000 + 700_000) / (5 + 2), rounded down.
        assert_eq!(team_score(500_000, &[900_000, 700_000], 5), 585_714);
        // No outcome: the prior.
        assert_eq!(team_score(420_000, &[], 5), 420_000);

        let teams = vec![
            TeamConfig::new("team:b", vec!["agent:x".into()]),
            TeamConfig::new("team:a", vec!["agent:y".into()]),
            TeamConfig::new("team:c", vec!["agent:z".into()]),
        ];
        let mut outcomes = vec![
            outcome("team:a", "t1", 900_000),
            outcome("team:a", "t2", 700_000),
            outcome("team:b", "t3", 900_000),
            outcome("team:b", "t4", 700_000),
            outcome("team:c", "t5", 100_000),
        ];
        let first = rank_teams(&teams, &outcomes, 5);
        outcomes.reverse();
        assert_eq!(first, rank_teams(&teams, &outcomes, 5));
        // Equal scores order by team id; the lowest score is last.
        assert_eq!(
            first.iter().map(|t| t.team_id.as_str()).collect::<Vec<_>>(),
            ["team:a", "team:b", "team:c"]
        );
        assert_eq!(first[0].score_ppm, first[1].score_ppm);
        assert_eq!(first[0].samples, 2);
        assert_eq!(first[2].samples, 1);
        // The prior is the mean of every outcome in the snapshot (660_000).
        assert_eq!(first[2].score_ppm, (660_000 * 5 + 100_000) / 6);
        // A snapshot without outcomes scores every team at the empty prior.
        let empty = rank_teams(&teams, &[], 5);
        assert!(empty.iter().all(|t| t.score_ppm == EMPTY_SNAPSHOT_PRIOR_PPM && t.samples == 0));
    }
}
