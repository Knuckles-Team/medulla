//! Ordered route stages (MEDULLA-DECISION-ENGINE-R035.750, R035.750.1,
//! R035.750.2).
//!
//! A route question runs its stages in a fixed order and the first stage that
//! answers wins; the learned policy is the shared ladder behind the rule
//! stages. The kernel here is pure: a stage reads the declared options and the
//! typed turn size, and the thresholds it needs arrive as arguments from the
//! pinned decision policy.

use super::refusal::{Refusal, RefusalResult};
use medulla_decision_types::decision::statistical::StatisticalErrorCode;

/// The classification that marks an option as the fast-path route.
pub const FAST_PATH_CLASS: &str = "eg:route/fast-path";
/// The stage name the fast path records in its premise.
pub const FAST_PATH_STAGE: &str = "fast_path";

/// The stage name team reuse records in its premise.
pub const TEAM_REUSE_STAGE: &str = "team_reuse";

/// One visible route option with the classification it carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteOption<'a> {
    pub option_id: &'a str,
    pub classification: &'a [String],
}

/// What a stage reads: the turn size when the caller stated it, and the
/// visible options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteInput<'a> {
    pub turn_tokens: Option<i64>,
    pub options: Vec<RouteOption<'a>>,
}

/// The option a stage chose and the stage that chose it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteChoice {
    pub option_id: String,
    pub stage: &'static str,
}

/// One stage of the pipeline: `Some(choice)` answers, `None` passes on.
pub trait RouteStage {
    fn choose(&self, input: &RouteInput<'_>) -> Option<RouteChoice>;
}

/// The fast path: a short turn goes to the visible option classified
/// [`FAST_PATH_CLASS`], lowest option id first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FastPath {
    /// The largest turn, in tokens, the fast path answers
    /// (`route.fast_path_max_tokens`).
    pub max_tokens: i64,
}

impl RouteStage for FastPath {
    fn choose(&self, input: &RouteInput<'_>) -> Option<RouteChoice> {
        let tokens = input.turn_tokens?;
        if tokens > self.max_tokens {
            return None;
        }
        input
            .options
            .iter()
            .filter(|option| option.classification.iter().any(|c| c == FAST_PATH_CLASS))
            .map(|option| option.option_id)
            .min()
            .map(|option_id| RouteChoice {
                option_id: option_id.to_string(),
                stage: FAST_PATH_STAGE,
            })
    }
}

/// What the evaluated history says about one option: how many of the earlier
/// decisions that shared this context executed it and were judged, and how
/// many of those judgements were successes. Censored judgements are not
/// counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OptionHistory<'a> {
    pub option_id: &'a str,
    pub evaluated: u64,
    pub successes: u64,
}

/// Team reuse: the visible option the team already ran in this context, when
/// it has been judged often enough and well enough. Among qualifying options
/// the best success rate wins, then the most judgements, then the lowest id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamReuse<'a> {
    pub history: Vec<OptionHistory<'a>>,
    /// Fewest judgements an option needs (`route.team_reuse_min_samples`).
    pub min_samples: u64,
    /// Lowest success rate an option needs, in micros
    /// (`route.team_reuse_min_rate`).
    pub min_rate_micros: u64,
}

impl TeamReuse<'_> {
    fn qualifies(&self, entry: &OptionHistory<'_>) -> bool {
        entry.evaluated >= self.min_samples.max(1)
            && u128::from(entry.successes) * 1_000_000
                >= u128::from(entry.evaluated) * u128::from(self.min_rate_micros)
    }
}

impl RouteStage for TeamReuse<'_> {
    fn choose(&self, input: &RouteInput<'_>) -> Option<RouteChoice> {
        self.history
            .iter()
            .filter(|entry| {
                input
                    .options
                    .iter()
                    .any(|option| option.option_id == entry.option_id)
                    && self.qualifies(entry)
            })
            .min_by(|a, b| {
                // a before b when a's rate is higher: successes_a / n_a > successes_b / n_b.
                let left = u128::from(a.successes) * u128::from(b.evaluated);
                let right = u128::from(b.successes) * u128::from(a.evaluated);
                right
                    .cmp(&left)
                    .then_with(|| b.evaluated.cmp(&a.evaluated))
                    .then_with(|| a.option_id.cmp(b.option_id))
            })
            .map(|entry| RouteChoice {
                option_id: entry.option_id.to_string(),
                stage: TEAM_REUSE_STAGE,
            })
    }
}

/// Run the stages in order; the first `Some` wins. When every stage passes,
/// the pipeline refuses with `ROUTE_NO_STAGE`.
pub fn route_pipeline(
    stages: &[&dyn RouteStage],
    input: &RouteInput<'_>,
) -> RefusalResult<RouteChoice> {
    stages
        .iter()
        .find_map(|stage| stage.choose(input))
        .ok_or_else(|| {
            Refusal::new(
                StatisticalErrorCode::RouteNoStage,
                "no route stage answered the question",
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classes(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| v.to_string()).collect()
    }

    fn input<'a>(tokens: Option<i64>, options: &'a [(&'a str, Vec<String>)]) -> RouteInput<'a> {
        RouteInput {
            turn_tokens: tokens,
            options: options
                .iter()
                .map(|(id, classification)| RouteOption {
                    option_id: id,
                    classification,
                })
                .collect(),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R035.750.1
    #[test]
    fn fast_path_wins_below_the_token_bound() {
        let options = vec![
            ("zeta", classes(&[FAST_PATH_CLASS])),
            ("alpha", classes(&[FAST_PATH_CLASS])),
            ("beta", classes(&["eg:route/team"])),
        ];
        let fast = FastPath { max_tokens: 256 };
        // At the bound the fast path answers, lowest option id first.
        let at_bound = route_pipeline(&[&fast], &input(Some(256), &options)).unwrap();
        assert_eq!(
            at_bound,
            RouteChoice {
                option_id: "alpha".to_string(),
                stage: FAST_PATH_STAGE
            }
        );
        // One token over the bound passes, and with no stage left the
        // pipeline refuses with the registered code.
        let over = route_pipeline(&[&fast], &input(Some(257), &options)).unwrap_err();
        assert_eq!(over.code, "ROUTE_NO_STAGE");
        // A turn with no stated size passes too.
        assert!(fast.choose(&input(None, &options)).is_none());
        // No visible option carries the fast-path class: pass.
        let plain = vec![("only", classes(&["eg:route/team"]))];
        assert!(fast.choose(&input(Some(1), &plain)).is_none());
        // A zero bound admits only an empty turn.
        let strict = FastPath { max_tokens: 0 };
        assert!(strict.choose(&input(Some(0), &options)).is_some());
        assert!(strict.choose(&input(Some(1), &options)).is_none());
    }

    struct Always(&'static str);
    impl RouteStage for Always {
        fn choose(&self, _input: &RouteInput<'_>) -> Option<RouteChoice> {
            Some(RouteChoice {
                option_id: "later".to_string(),
                stage: self.0,
            })
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R035.750.1
    #[test]
    fn the_first_stage_that_answers_wins() {
        let options = vec![("alpha", classes(&[FAST_PATH_CLASS]))];
        let fast = FastPath { max_tokens: 10 };
        let later = Always("team_reuse");
        let first = route_pipeline(&[&fast, &later], &input(Some(5), &options)).unwrap();
        assert_eq!(first.stage, FAST_PATH_STAGE);
        let second = route_pipeline(&[&fast, &later], &input(Some(50), &options)).unwrap();
        assert_eq!(second.stage, "team_reuse");
    }

    // spec: MEDULLA-DECISION-ENGINE-R035.750.2
    #[test]
    fn team_reuse_needs_enough_judgements_at_a_high_enough_rate() {
        let options = vec![
            ("alpha", classes(&[])),
            ("beta", classes(&[])),
            ("zeta", classes(&[])),
        ];
        let reuse = |history: Vec<OptionHistory<'static>>| TeamReuse {
            history,
            min_samples: 5,
            min_rate_micros: 800_000,
        };
        let entry = |option_id, evaluated, successes| OptionHistory {
            option_id,
            evaluated,
            successes,
        };
        let choose = |stage: &TeamReuse<'_>| stage.choose(&input(None, &options));
        // 4 of 5 is exactly the rate and exactly the sample floor: reused.
        let chosen = choose(&reuse(vec![entry("beta", 5, 4)])).unwrap();
        assert_eq!(
            chosen,
            RouteChoice {
                option_id: "beta".to_string(),
                stage: TEAM_REUSE_STAGE
            }
        );
        // One judgement short of the floor, or one success short of the rate: pass.
        assert!(choose(&reuse(vec![entry("beta", 4, 4)])).is_none());
        assert!(choose(&reuse(vec![entry("beta", 6, 4)])).is_none());
        // An option that is not visible is never reused.
        assert!(choose(&reuse(vec![entry("gone", 9, 9)])).is_none());
        // Best rate first, then most judgements, then lowest id.
        let best = reuse(vec![
            entry("zeta", 10, 9),
            entry("beta", 5, 5),
            entry("alpha", 10, 9),
        ]);
        assert_eq!(choose(&best).unwrap().option_id, "beta");
        let tied = reuse(vec![
            entry("zeta", 10, 10),
            entry("beta", 5, 5),
            entry("alpha", 10, 10),
        ]);
        assert_eq!(choose(&tied).unwrap().option_id, "alpha");
        // A zero floor still needs one judgement.
        let floorless = TeamReuse {
            history: vec![entry("beta", 0, 0)],
            min_samples: 0,
            min_rate_micros: 0,
        };
        assert!(choose(&floorless).is_none());
    }
}
