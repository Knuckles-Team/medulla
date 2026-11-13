//! `SOURCE RELIABILITY <id>` (EH-525): re-weight every row by how reliable a source is.
//!
//! The PRIOR is the source's propagated belief confidence in the graph
//! (`medulla_epistemic::propagate_confidence` under the plan's `AuthorityPolicy`), held with
//! the policy's `prior_strength` pseudo-counts. When the plan has the caller's decision
//! log bound and that log holds independently evaluated outcomes of the source, the
//! reliability is LEARNED: the prior updated with those (discounted) outcomes, read
//! from the log's `reputation` view — visibility is the log's, so an outcome the caller
//! cannot see never moves the number. Without a log, or with no outcome of the source,
//! the prior is used unchanged (the pre-EH-525 behaviour). A source with outcomes but fewer
//! than `reputation.min_support` ABSTAINS: its rows carry no reliability and no score (the
//! prior is never substituted where the log has spoken but not loudly enough). The multiplier
//! is uniform over the rows; an unscored row counts as `1.0`. The learned interval is reported
//! as the `reliability_lo` / `reliability_hi` channels, the value as `reliability`, the
//! abstention as `reliability_abstained` (1 or 0) and where the number came from as the
//! `reliability_source` text (`materialized`, `prior` or `abstained`).

use super::{belief_policy, PlanCtx};
use crate::rowset::RowSet;
use medulla_decision_types::decision::reputation::SourceReliability;

/// Where a source's reliability number came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReliabilitySource {
    /// The estimate learned from the visible decision log.
    Materialized,
    /// The graph's propagated belief confidence (the log holds no outcome of the source).
    Prior,
    /// The log holds fewer outcomes than `reputation.min_support`: no number.
    Abstained,
}

impl ReliabilitySource {
    /// The `reliability_source` text a row carries.
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Materialized => "materialized",
            Self::Prior => "prior",
            Self::Abstained => "abstained",
        }
    }
}

/// A source's reliability (none when it abstained) and, when it was learned, its credible
/// interval.
pub(super) struct Reliability {
    pub(super) mean: Option<f64>,
    pub(super) interval: Option<(f64, f64)>,
    pub(super) source: ReliabilitySource,
}

/// The reliability the typed read `learned` yields over the graph `prior`.
pub(super) fn from_learned(prior: f64, learned: SourceReliability) -> Reliability {
    match learned {
        SourceReliability::Estimated {
            mean, lower, upper, ..
        } => Reliability {
            mean: Some(mean),
            interval: Some((lower, upper)),
            source: ReliabilitySource::Materialized,
        },
        SourceReliability::Absent => Reliability {
            mean: Some(prior),
            interval: None,
            source: ReliabilitySource::Prior,
        },
        SourceReliability::InsufficientHistory { .. } => Reliability {
            mean: None,
            interval: None,
            source: ReliabilitySource::Abstained,
        },
    }
}

/// The reliability of `source_id` under `ctx`.
pub(super) fn reliability_of(ctx: &PlanCtx, source_id: &str) -> Result<Reliability, String> {
    let graph = medulla_epistemic::BeliefGraph::from_graph_view(ctx.view);
    let policy = belief_policy(ctx);
    let prior = medulla_epistemic::propagate_confidence(&graph, source_id, &policy).confidence;
    let learned = match ctx.decisions {
        Some(log) => log.learned_reliability(source_id, prior, policy.prior_strength)?,
        None => SourceReliability::Absent,
    };
    Ok(from_learned(prior, learned))
}

/// `input` re-weighted by `reliability`; an abstained source leaves every row unscored.
pub(super) fn reweighted(input: &RowSet, reliability: &Reliability) -> RowSet {
    let Some(mean) = reliability.mean else {
        return RowSet::from_rows(input.rows().iter().map(|row| (row.id.clone(), None)));
    };
    let r = mean as f32;
    RowSet::from_scored(
        input
            .rows()
            .iter()
            .map(|row| (row.id.clone(), row.score.unwrap_or(1.0) * r)),
    )
}

/// `SourceReliability { source_id }` as a plain stage.
pub(super) fn source_reliability_op(
    ctx: &PlanCtx,
    input: RowSet,
    source_id: &str,
) -> Result<RowSet, String> {
    Ok(reweighted(&input, &reliability_of(ctx, source_id)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::DecisionSource;
    use medulla_graph_core::graph::GraphCore;
    use medulla_semantic_index::semantic::SemanticStore;
    use serde_json::{Map, Value};

    /// A log whose reputation view answers one fixed typed read.
    struct Reads(SourceReliability);

    impl DecisionSource for Reads {
        fn decision_rows(&self) -> Result<Vec<Map<String, Value>>, String> {
            Ok(Vec::new())
        }

        fn learned_reliability(
            &self,
            _subject: &str,
            _prior_mean: f64,
            _prior_strength: f64,
        ) -> Result<SourceReliability, String> {
            Ok(self.0)
        }
    }

    fn reliability(read: SourceReliability) -> Reliability {
        let core = GraphCore::new();
        let graph = core.analysis_snapshot();
        let semantic = SemanticStore::new();
        let log = Reads(read);
        let ctx = PlanCtx::new(&graph, &semantic).with_decisions(&log);
        reliability_of(&ctx, "src").expect("reliability")
    }

    // spec: MEDULLA-DECISION-ENGINE-R118.1.2.2
    #[test]
    fn insufficient_history_abstains_instead_of_using_the_prior() {
        let sparse = reliability(SourceReliability::InsufficientHistory {
            trials: 2,
            min_support: 5,
        });
        assert_eq!(sparse.mean, None, "no number where the log is too sparse");
        assert_eq!(sparse.interval, None);
        assert_eq!(sparse.source, ReliabilitySource::Abstained);
        let rows = reweighted(&RowSet::from_scored([("a".to_string(), 0.5)]), &sparse);
        assert_eq!(rows.rows()[0].score, None, "the abstained row is unscored");

        let absent = reliability(SourceReliability::Absent);
        assert_eq!(absent.source, ReliabilitySource::Prior);
        assert!(absent.mean.is_some(), "Absent keeps the graph prior");

        let dense = reliability(SourceReliability::Estimated {
            mean: 0.8,
            lower: 0.7,
            upper: 0.9,
            trials: 40,
        });
        assert_eq!(dense.source, ReliabilitySource::Materialized);
        assert_eq!(
            dense.mean,
            Some(0.8),
            "the materialized estimate, not the prior"
        );
        assert_eq!(dense.interval, Some((0.7, 0.9)));
    }
}
