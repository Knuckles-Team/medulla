//! Topology analytics result model (MEDULLA-FEDERATED-QUERY-R607.1).
//!
//! The served `MineTopology` method returns one [`TopologyResult`]: spectral or
//! mincut partitions, cross-domain analogy matches, synergy bridges between
//! communities and inductive hyperedge scores, plus the run's evidence. Every id
//! is validated non-empty and at most [`MAX_TOPOLOGY_ID_BYTES`] bytes.

use serde::{Deserialize, Serialize};

/// Upper bound on any node or partition id carried by a topology result.
pub const MAX_TOPOLOGY_ID_BYTES: usize = 512;
/// Default and bounds of the request `k`.
pub const TOPOLOGY_K_DEFAULT: u32 = 8;
pub const TOPOLOGY_K_MIN: u32 = 2;
pub const TOPOLOGY_K_MAX: u32 = 256;

/// The five topology analytics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TopologyAnalytic {
    Spectral,
    Mincut,
    Analogy,
    Synergy,
    HyperedgeScore,
}

impl TopologyAnalytic {
    /// The span / evidence name of the analytic.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Spectral => "spectral",
            Self::Mincut => "mincut",
            Self::Analogy => "analogy",
            Self::Synergy => "synergy",
            Self::HyperedgeScore => "hyperedge_score",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Partition {
    pub id: String,
    pub members: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnalogyMatch {
    pub a: String,
    pub b: String,
    pub score: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SynergyBridge {
    pub node: String,
    pub communities: Vec<String>,
    pub score: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HyperedgeScore {
    pub members: Vec<String>,
    pub score: f64,
}

/// Run evidence (MEDULLA-FEDERATED-QUERY-R607.5); analytic-specific fields are
/// `None` when the analytic does not produce them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TopologyEvidence {
    pub nodes: u64,
    pub edges: u64,
    pub iterations: u32,
    #[serde(default)]
    pub modularity: Option<f64>,
    #[serde(default)]
    pub cut_weight: Option<f64>,
    #[serde(default)]
    pub match_scores: Vec<f64>,
    pub elapsed_ms: u64,
    pub seed: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TopologyResult {
    pub analytic: TopologyAnalytic,
    #[serde(default)]
    pub partitions: Vec<Partition>,
    #[serde(default)]
    pub matches: Vec<AnalogyMatch>,
    #[serde(default)]
    pub bridges: Vec<SynergyBridge>,
    #[serde(default)]
    pub scores: Vec<HyperedgeScore>,
    pub evidence: TopologyEvidence,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TopologyIdError {
    #[error("topology id is empty")]
    Empty,
    #[error("topology id exceeds {MAX_TOPOLOGY_ID_BYTES} bytes")]
    TooLong,
}

/// Validate one topology id: non-empty and at most [`MAX_TOPOLOGY_ID_BYTES`].
pub fn validate_topology_id(id: &str) -> Result<(), TopologyIdError> {
    if id.is_empty() {
        return Err(TopologyIdError::Empty);
    }
    if id.len() > MAX_TOPOLOGY_ID_BYTES {
        return Err(TopologyIdError::TooLong);
    }
    Ok(())
}

impl TopologyResult {
    /// Validate every id the result carries.
    pub fn validate(&self) -> Result<(), TopologyIdError> {
        for p in &self.partitions {
            validate_topology_id(&p.id)?;
            p.members.iter().try_for_each(|m| validate_topology_id(m))?;
        }
        for m in &self.matches {
            validate_topology_id(&m.a)?;
            validate_topology_id(&m.b)?;
        }
        for b in &self.bridges {
            validate_topology_id(&b.node)?;
            b.communities
                .iter()
                .try_for_each(|c| validate_topology_id(c))?;
        }
        for s in &self.scores {
            s.members.iter().try_for_each(|m| validate_topology_id(m))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> TopologyResult {
        TopologyResult {
            analytic: TopologyAnalytic::Spectral,
            partitions: vec![Partition {
                id: "0".into(),
                members: vec!["a".into(), "b".into()],
            }],
            matches: vec![AnalogyMatch {
                a: "a".into(),
                b: "x".into(),
                score: 0.75,
            }],
            bridges: vec![SynergyBridge {
                node: "b".into(),
                communities: vec!["0".into(), "1".into()],
                score: 1.5,
            }],
            scores: vec![HyperedgeScore {
                members: vec!["a".into(), "b".into()],
                score: 0.9,
            }],
            evidence: TopologyEvidence {
                nodes: 4,
                edges: 3,
                iterations: 100,
                modularity: Some(0.4),
                cut_weight: None,
                match_scores: vec![0.75],
                elapsed_ms: 2,
                seed: 7,
            },
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R607.1
    #[test]
    fn result_round_trips_and_rejects_bad_ids() {
        let r = sample();
        assert_eq!(r.validate(), Ok(()));
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<TopologyResult>(&json).unwrap(), r);
        let bytes = rmp_serde::to_vec_named(&r).unwrap();
        assert_eq!(rmp_serde::from_slice::<TopologyResult>(&bytes).unwrap(), r);
        for analytic in [
            TopologyAnalytic::Spectral,
            TopologyAnalytic::Mincut,
            TopologyAnalytic::Analogy,
            TopologyAnalytic::Synergy,
            TopologyAnalytic::HyperedgeScore,
        ] {
            let s = serde_json::to_string(&analytic).unwrap();
            assert_eq!(s, format!("\"{}\"", analytic.as_str()));
            assert_eq!(
                serde_json::from_str::<TopologyAnalytic>(&s).unwrap(),
                analytic
            );
        }

        let mut bad = sample();
        bad.partitions[0].id.clear();
        assert_eq!(bad.validate(), Err(TopologyIdError::Empty));
        let mut bad = sample();
        bad.matches[0].b = String::new();
        assert_eq!(bad.validate(), Err(TopologyIdError::Empty));
        let mut bad = sample();
        bad.bridges[0].node = "n".repeat(MAX_TOPOLOGY_ID_BYTES + 1);
        assert_eq!(bad.validate(), Err(TopologyIdError::TooLong));
        assert_eq!(
            validate_topology_id(&"n".repeat(MAX_TOPOLOGY_ID_BYTES)),
            Ok(())
        );
    }
}
