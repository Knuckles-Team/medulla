//! The nearest-neighbour probe and ranking of a `NEAREST` column read
//! (MEDULLA-FEDERATED-QUERY-R077.5).
//!
//! The semantic binding of a text column answers a probe: the nearest row identities of one
//! live generation. The authorized read keeps the rows of the caller's own visible table whose
//! identity is a hit, and hands the SQL executor a ranking: the distance of each kept row's
//! column text. The executor then runs the statement unchanged.

use std::collections::HashMap;

/// What the live generation of a column's semantic binding answered for one query text.
#[derive(Debug, Clone, PartialEq)]
pub struct NearestProbe {
    /// The table the probed column belongs to, as the caller's schema spells it.
    pub table: String,
    /// The probed column, as the table schema spells it.
    pub column: String,
    /// The binding the probe read, `binding:{table}.{column}`.
    pub binding_id: String,
    /// The live generation the hits come from.
    pub generation: u64,
    /// The model digest of the binding's space.
    pub space: String,
    /// `(source entity id, distance)`, nearest first. The entity id is the row's
    /// `SemanticSqlSourceIdentity::source_entity_id`.
    pub hits: Vec<(String, f32)>,
}

/// The distance of each kept row's column text, and the plan line `EXPLAIN` reports.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NearestRanking {
    /// Column text to hit distance; equal texts keep the smaller distance.
    pub distances: HashMap<String, f64>,
    /// The `vector_scan` plan line an `EXPLAIN` of the statement appends.
    pub explain: String,
}

impl NearestRanking {
    /// Record `text` at `distance`, keeping the smaller distance for a repeated text.
    pub fn record(&mut self, text: String, distance: f64) {
        self.distances
            .entry(text)
            .and_modify(|kept| {
                if distance < *kept {
                    *kept = distance;
                }
            })
            .or_insert(distance);
    }
}

impl NearestProbe {
    /// The `EXPLAIN` plan line of this probe.
    pub fn explain_line(&self) -> String {
        format!(
            "VectorScan binding={} space={} generation={}",
            self.binding_id, self.space, self.generation
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-FEDERATED-QUERY-R077.5
    #[test]
    fn equal_texts_keep_the_smaller_distance() {
        let mut ranking = NearestRanking::default();
        ranking.record("a".into(), 0.5);
        ranking.record("a".into(), 0.2);
        ranking.record("a".into(), 0.9);
        assert_eq!(ranking.distances["a"], 0.2);
    }
}
