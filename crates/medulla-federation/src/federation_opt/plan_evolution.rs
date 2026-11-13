//! The plan-evolution model (MEDULLA-FEDERATED-QUERY-R091.1): what federated query planning
//! observes about each query shape, and the candidate plans it may keep.
//!
//! A [`PlanShape`] is a query with its literals removed. Every execution of a shape records
//! one [`PlanObservation`]; a [`PlanCandidate`] is an alternative plan measured against the
//! baseline. The [`ObservationStore`] is in memory only and bounded twice: each shape keeps
//! at most `observations_per_shape` observations (the oldest is dropped first) and each
//! tenant keeps at most `shapes_per_tenant` shapes (the least recently seen shape is evicted,
//! as `stats::observe` evicts the stalest source). Kept plans persist as `medulla-jobs` job
//! results, never here.
//!
//! The two limits are the registry knobs `federation.observations_per_shape` and
//! `federation.shapes_per_tenant`. This leaf crate cannot see the registry, so the composition
//! root reads the resolved knob values and passes them in as [`StoreLimits`].
//!
//! Every entry is keyed by the verified tenant and the shape digest, never by the digest alone,
//! so one tenant's activity is never visible through another tenant's lookups.

use std::collections::{HashMap, VecDeque};

use serde::{Deserialize, Serialize};

/// A query with its literals removed, identified by the digest of its template.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanShape {
    pub shape_digest: String,
    pub query_template: String,
    pub language: String,
    /// The read authority captured when the shape was recorded.
    pub authority_ref: String,
}

/// What one execution of a shape did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanObservation {
    pub shape_digest: String,
    pub plan_digest: String,
    pub sources: Vec<String>,
    pub pushdowns: Vec<String>,
    pub latency_us: u64,
    pub rows_fetched: u64,
    pub bytes_fetched: u64,
    pub cache_hit: bool,
    pub graph_revision: u64,
}

/// The closed set of candidate kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateKind {
    Rewrite,
    PushdownChoice,
    CacheSubgraph,
    MaterializeSubgraph,
    IndexSuggestion,
}

/// An alternative plan for a shape, named against the baseline it must beat.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanCandidate {
    pub shape_digest: String,
    pub kind: CandidateKind,
    pub plan_digest: String,
    pub baseline_digest: String,
}

/// The bounds of an [`ObservationStore`], resolved from the registry knobs by the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreLimits {
    /// `knob federation.observations_per_shape`, items.
    pub observations_per_shape: usize,
    /// `knob federation.shapes_per_tenant`, shapes.
    pub shapes_per_tenant: usize,
}

struct ShapeEntry {
    shape: PlanShape,
    observations: VecDeque<PlanObservation>,
    /// Position of the latest observation on the tenant's logical clock.
    last_seen: u64,
}

#[derive(Default)]
struct TenantShapes {
    shapes: HashMap<String, ShapeEntry>,
    clock: u64,
}

/// The bounded in-memory observations of every tenant's query shapes.
pub struct ObservationStore {
    limits: StoreLimits,
    tenants: HashMap<String, TenantShapes>,
}

impl ObservationStore {
    pub fn new(limits: StoreLimits) -> Self {
        Self {
            limits,
            tenants: HashMap::new(),
        }
    }

    /// Record one execution of `shape` for `tenant`. A new shape in a full tenant evicts the
    /// least recently seen shape first; a shape at its observation bound drops its oldest.
    pub fn observe(&mut self, tenant: &str, shape: PlanShape, observation: PlanObservation) {
        let limits = self.limits;
        let state = self.tenants.entry(tenant.to_string()).or_default();
        state.clock += 1;
        let now = state.clock;
        if !state.shapes.contains_key(&shape.shape_digest)
            && state.shapes.len() >= limits.shapes_per_tenant
        {
            let stalest = state
                .shapes
                .iter()
                .min_by_key(|(_, entry)| entry.last_seen)
                .map(|(digest, _)| digest.clone());
            if let Some(digest) = stalest {
                state.shapes.remove(&digest);
            }
        }
        let entry = state
            .shapes
            .entry(shape.shape_digest.clone())
            .or_insert_with(|| ShapeEntry {
                shape,
                observations: VecDeque::new(),
                last_seen: now,
            });
        entry.last_seen = now;
        while entry.observations.len() >= limits.observations_per_shape {
            entry.observations.pop_front();
        }
        entry.observations.push_back(observation);
    }

    /// The shapes `tenant` has recorded.
    pub fn shape_count(&self, tenant: &str) -> usize {
        self.tenants.get(tenant).map_or(0, |t| t.shapes.len())
    }

    /// The shape `tenant` recorded under `shape_digest`.
    pub fn shape(&self, tenant: &str, shape_digest: &str) -> Option<&PlanShape> {
        self.tenants
            .get(tenant)?
            .shapes
            .get(shape_digest)
            .map(|entry| &entry.shape)
    }

    /// The observations `tenant` holds for `shape_digest`, oldest first.
    pub fn observations(
        &self,
        tenant: &str,
        shape_digest: &str,
    ) -> impl Iterator<Item = &PlanObservation> {
        self.tenants
            .get(tenant)
            .and_then(|t| t.shapes.get(shape_digest))
            .into_iter()
            .flat_map(|entry| entry.observations.iter())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: StoreLimits = StoreLimits {
        observations_per_shape: 3,
        shapes_per_tenant: 2,
    };

    fn shape(digest: &str) -> PlanShape {
        PlanShape {
            shape_digest: digest.into(),
            query_template: format!("SELECT ?x WHERE {{ ?x a <{digest}> }}"),
            language: "sparql".into(),
            authority_ref: "authority:1".into(),
        }
    }

    fn observation(digest: &str, plan: &str) -> PlanObservation {
        PlanObservation {
            shape_digest: digest.into(),
            plan_digest: plan.into(),
            sources: vec!["source-a".into(), "source-b".into()],
            pushdowns: vec!["filter".into()],
            latency_us: 1_500,
            rows_fetched: 20,
            bytes_fetched: 4_096,
            cache_hit: false,
            graph_revision: 7,
        }
    }

    fn plans(store: &ObservationStore, tenant: &str, digest: &str) -> Vec<String> {
        store
            .observations(tenant, digest)
            .map(|o| o.plan_digest.clone())
            .collect()
    }

    // spec: MEDULLA-FEDERATED-QUERY-R091.1
    #[test]
    fn observation_store_bounds() {
        // Every type round-trips and keeps every field.
        let shape_value = shape("s1");
        let encoded = serde_json::to_string(&shape_value).unwrap();
        assert_eq!(
            serde_json::from_str::<PlanShape>(&encoded).unwrap(),
            shape_value
        );
        let observed = observation("s1", "p1");
        let encoded = serde_json::to_string(&observed).unwrap();
        assert_eq!(
            serde_json::from_str::<PlanObservation>(&encoded).unwrap(),
            observed
        );
        for kind in [
            CandidateKind::Rewrite,
            CandidateKind::PushdownChoice,
            CandidateKind::CacheSubgraph,
            CandidateKind::MaterializeSubgraph,
            CandidateKind::IndexSuggestion,
        ] {
            let candidate = PlanCandidate {
                shape_digest: "s1".into(),
                kind,
                plan_digest: "p2".into(),
                baseline_digest: "p1".into(),
            };
            let encoded = serde_json::to_string(&candidate).unwrap();
            assert_eq!(
                serde_json::from_str::<PlanCandidate>(&encoded).unwrap(),
                candidate
            );
        }

        // An unknown kind and an unknown field are refused.
        let unknown_kind =
            r#"{"shape_digest":"s1","kind":"reorder","plan_digest":"p2","baseline_digest":"p1"}"#;
        assert!(serde_json::from_str::<PlanCandidate>(unknown_kind).is_err());
        let unknown_field = r#"{"shape_digest":"s1","query_template":"q","language":"uql","authority_ref":"a","extra":1}"#;
        assert!(serde_json::from_str::<PlanShape>(unknown_field).is_err());

        // A shape keeps its newest observations only.
        let mut store = ObservationStore::new(LIMITS);
        for plan in ["p1", "p2", "p3", "p4"] {
            store.observe("tenant-a", shape("s1"), observation("s1", plan));
        }
        assert_eq!(plans(&store, "tenant-a", "s1"), ["p2", "p3", "p4"]);

        // A tenant keeps its most recently seen shapes: s2 is the stalest when s3 arrives.
        store.observe("tenant-a", shape("s2"), observation("s2", "q1"));
        store.observe("tenant-a", shape("s1"), observation("s1", "p5"));
        store.observe("tenant-a", shape("s3"), observation("s3", "r1"));
        assert_eq!(store.shape_count("tenant-a"), 2);
        assert!(store.shape("tenant-a", "s2").is_none());
        assert!(store.shape("tenant-a", "s1").is_some());
        assert!(store.shape("tenant-a", "s3").is_some());

        // Another tenant's shapes are separate: nothing is visible across tenants.
        assert_eq!(store.shape_count("tenant-b"), 0);
        assert_eq!(store.observations("tenant-b", "s1").count(), 0);
        store.observe("tenant-b", shape("s1"), observation("s1", "b1"));
        assert_eq!(plans(&store, "tenant-b", "s1"), ["b1"]);
        assert_eq!(plans(&store, "tenant-a", "s1"), ["p3", "p4", "p5"]);
    }
}
