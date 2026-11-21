//! MEDULLA-FEDERATED-QUERY-R040.1.2: hard retrieval constraints compiled to an id set.
//!
//! [`compile_constraints`] applies every declared entity and relationship predicate
//! of a [`RetrievalConstraints`] to the caller's (already authorized) snapshot and
//! answers the set of eligible node ids. It does no ranking and never truncates:
//! a ranking step takes the set as a filter, so a candidate that fails a constraint
//! can never reach a ranked result, however high it would have scored. Predicates
//! are conjunctive (`require_all`): the eligible set is the intersection of every
//! predicate's matches, so a contradictory conjunction yields an empty set.

use std::collections::BTreeSet;
use std::sync::Arc;

use medulla_graph_core::graph::GraphView;
use medulla_query_types::wire::{
    CmpOp, EntityPredicate, Pred, RelationshipPredicate, RelationshipQuantifier,
    RetrievalConstraints,
};

use medulla_types::contract::ServerErrorCode;

use crate::exec::expand::{admitted_edges, EdgeFilter};

/// The ids that satisfy a [`RetrievalConstraints`], bound to the graph revision the
/// constraints were declared against. Cheap to clone: the ids are shared.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConstraintSet {
    graph_revision: u64,
    eligible: Arc<BTreeSet<String>>,
}

impl ConstraintSet {
    /// The graph revision the constraints named.
    pub fn graph_revision(&self) -> u64 {
        self.graph_revision
    }

    /// Is `id` eligible to be ranked?
    pub fn contains(&self, id: &str) -> bool {
        self.eligible.contains(id)
    }

    /// The eligible ids, sorted.
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.eligible.iter().map(String::as_str)
    }

    /// How many ids are eligible.
    pub fn len(&self) -> usize {
        self.eligible.len()
    }

    /// Does no id satisfy the constraints?
    pub fn is_empty(&self) -> bool {
        self.eligible.is_empty()
    }
}

/// MEDULLA-FEDERATED-QUERY-R040.1.4: why a constraint set was refused. A refusal ranks
/// nothing, and a hard exclusion never becomes a score penalty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConstraintError {
    /// A malformed or unsupported predicate (`CONSTRAINT_INVALID`).
    Invalid(String),
    /// The constraints pin a graph revision other than the one the query reads
    /// (`GRAPH_REVISION_STALE`).
    RevisionStale {
        /// The revision the constraints named.
        pinned: u64,
        /// The revision the query reads.
        current: u64,
    },
    /// The constraints pin a revision but the query binds none to compare against
    /// (`CONSTRAINT_UNEVALUABLE`).
    Unevaluable(String),
}

impl ConstraintError {
    /// The registered refusal code.
    pub fn code(&self) -> ServerErrorCode {
        match self {
            Self::Invalid(_) => ServerErrorCode::ConstraintInvalid,
            Self::RevisionStale { .. } => ServerErrorCode::GraphRevisionStale,
            Self::Unevaluable(_) => ServerErrorCode::ConstraintUnevaluable,
        }
    }
}

impl std::fmt::Display for ConstraintError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let code = self.code();
        match self {
            Self::Invalid(message) | Self::Unevaluable(message) => write!(f, "{code}: {message}"),
            Self::RevisionStale { pinned, current } => write!(
                f,
                "{code}: the constraints pin graph revision {pinned}, the query reads revision {current}"
            ),
        }
    }
}

impl std::error::Error for ConstraintError {}

impl From<ConstraintError> for String {
    fn from(error: ConstraintError) -> Self {
        error.to_string()
    }
}

/// Compile `constraints` over `view` into the set of eligible ids. A malformed
/// predicate is refused before any node is read. Property comparison follows the
/// engine's SQL three-valued logic: a missing property or a differently typed value
/// is unknown and does not satisfy a predicate.
///
/// `current_revision` is the graph revision the query reads (`None`: a plan run
/// outside a served read binds none). A `graph_revision` of `0` declares no pin;
/// any other value must equal it, else `GRAPH_REVISION_STALE`, and a pin with no
/// revision to compare against is `CONSTRAINT_UNEVALUABLE`. An unsatisfiable but
/// valid conjunction is not an error: it is an empty set.
pub fn compile_constraints(
    view: &GraphView,
    constraints: &RetrievalConstraints,
    current_revision: Option<u64>,
) -> Result<ConstraintSet, ConstraintError> {
    constraints
        .validate()
        .map_err(ConstraintError::Invalid)?;
    if constraints.graph_revision != 0 {
        match current_revision {
            Some(current) if current == constraints.graph_revision => {}
            Some(current) => {
                return Err(ConstraintError::RevisionStale {
                    pinned: constraints.graph_revision,
                    current,
                })
            }
            None => {
                return Err(ConstraintError::Unevaluable(format!(
                    "the constraints pin graph revision {} but the query binds no revision to compare against",
                    constraints.graph_revision
                )))
            }
        }
    }
    let entity: Vec<Pred> = constraints
        .entity_predicates
        .iter()
        .map(entity_pred)
        .collect();
    let relationships: Vec<(&RelationshipPredicate, Pred)> = constraints
        .relationship_predicates
        .iter()
        .map(|predicate| (predicate, entity_pred(&predicate.target)))
        .collect();
    let mut eligible = BTreeSet::new();
    for (id, &index) in &view.node_map {
        let Some(props) = view.node_row_object(id) else {
            continue;
        };
        if !crate::pred_eval::all_hold(&props, &entity) {
            continue;
        }
        let holds = relationships.iter().all(|(predicate, target)| {
            let filter = EdgeFilter {
                rel: Some(predicate.edge_kind.as_str()),
                preds: &[],
                pin: None,
            };
            let reached = admitted_edges(view, index, predicate.direction, &filter)
                .into_iter()
                .any(|(other, _)| {
                    view.node_row_object(&view.graph[other])
                        .is_some_and(|neighbour| {
                            crate::pred_eval::all_hold(&neighbour, &[target.clone()])
                        })
                });
            match predicate.quantifier {
                RelationshipQuantifier::Exists => reached,
                RelationshipQuantifier::NotExists => !reached,
            }
        });
        if holds {
            eligible.insert(id.clone());
        }
    }
    Ok(ConstraintSet {
        graph_revision: constraints.graph_revision,
        eligible: Arc::new(eligible),
    })
}

/// The row predicate an entity predicate means.
fn entity_pred(predicate: &EntityPredicate) -> Pred {
    let cmp = |property: &String, op: CmpOp, value: &medulla_query_types::wire::PredLiteral| Pred::Cmp {
        prop: property.clone(),
        op,
        value: value.clone(),
    };
    match predicate {
        EntityPredicate::Eq { property, value } => cmp(property, CmpOp::Eq, value),
        EntityPredicate::Ne { property, value } => cmp(property, CmpOp::Ne, value),
        EntityPredicate::Lt { property, value } => cmp(property, CmpOp::Lt, value),
        EntityPredicate::Le { property, value } => cmp(property, CmpOp::Le, value),
        EntityPredicate::Gt { property, value } => cmp(property, CmpOp::Gt, value),
        EntityPredicate::Ge { property, value } => cmp(property, CmpOp::Ge, value),
        EntityPredicate::In { property, values } => Pred::In {
            prop: property.clone(),
            values: values.iter().cloned().collect(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_graph_core::graph::GraphCore;
    use medulla_types::contract::BoundedVec;
    use medulla_query_types::wire::{EdgeDir, PredLiteral};
    use serde_json::json;
    use std::collections::BTreeMap;

    fn blob(value: serde_json::Value) -> Vec<u8> {
        rmp_serde::to_vec_named(&value).unwrap()
    }

    /// The fixture, kept as plain data so the oracle never touches the engine.
    struct Fixture {
        /// id -> (kind, embedding)
        nodes: BTreeMap<&'static str, (&'static str, [f32; 2])>,
        /// (from, relationship, to)
        edges: Vec<(&'static str, &'static str, &'static str)>,
    }

    fn fixture() -> Fixture {
        Fixture {
            nodes: BTreeMap::from([
                ("best", ("doc", [1.0, 0.0])),
                ("good", ("doc", [0.9, 0.1])),
                ("fair", ("doc", [0.5, 0.5])),
                ("poor", ("doc", [0.0, 1.0])),
                ("banned", ("source", [0.0, 0.0])),
                ("trusted", ("source", [0.0, 0.0])),
            ]),
            edges: vec![
                ("best", "cites", "banned"),
                ("good", "cites", "trusted"),
                ("fair", "cites", "trusted"),
                ("poor", "cites", "banned"),
            ],
        }
    }

    fn graph(fixture: &Fixture) -> GraphCore {
        let core = GraphCore::new();
        for (id, (kind, embedding)) in &fixture.nodes {
            let banned = *id == "banned";
            core.add_node(
                (*id).into(),
                blob(json!({"type": kind, "kind": kind, "embedding": embedding, "banned": banned})),
            );
        }
        for (from, relationship, to) in &fixture.edges {
            core.add_edge(
                (*from).into(),
                (*to).into(),
                blob(json!({"relationship": relationship})),
            )
            .unwrap();
        }
        core
    }

    /// Independent ranking oracle: cosine similarity to the query, best first.
    fn oracle_ranking(fixture: &Fixture, query: [f32; 2], among: &[&str]) -> Vec<String> {
        let cosine = |v: [f32; 2]| {
            let norm = (v[0] * v[0] + v[1] * v[1]).sqrt()
                * (query[0] * query[0] + query[1] * query[1]).sqrt();
            if norm == 0.0 {
                0.0
            } else {
                (v[0] * query[0] + v[1] * query[1]) / norm
            }
        };
        let mut ranked: Vec<(&str, f32)> = among
            .iter()
            .map(|id| (*id, cosine(fixture.nodes[id].1)))
            .collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(b.0)));
        ranked.into_iter().map(|(id, _)| id.to_string()).collect()
    }

    /// Independent eligibility oracle: documents that do not cite a banned source.
    fn oracle_eligible(fixture: &Fixture) -> BTreeSet<String> {
        fixture
            .nodes
            .iter()
            .filter(|(_, (kind, _))| *kind == "doc")
            .filter(|(id, _)| {
                !fixture
                    .edges
                    .iter()
                    .any(|(from, rel, to)| from == *id && *rel == "cites" && *to == "banned")
            })
            .map(|(id, _)| id.to_string())
            .collect()
    }

    fn constraints(relationship: Vec<RelationshipPredicate>) -> RetrievalConstraints {
        RetrievalConstraints {
            graph_revision: 4,
            entity_predicates: BoundedVec::new(vec![EntityPredicate::Eq {
                property: "kind".into(),
                value: PredLiteral::Str("doc".into()),
            }])
            .unwrap(),
            relationship_predicates: BoundedVec::new(relationship).unwrap(),
        }
    }

    fn cites_banned(quantifier: RelationshipQuantifier) -> RelationshipPredicate {
        RelationshipPredicate {
            edge_kind: "cites".into(),
            direction: EdgeDir::Out,
            target: EntityPredicate::Eq {
                property: "banned".into(),
                value: PredLiteral::Bool(true),
            },
            quantifier,
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R040.1.2
    #[test]
    fn failed_constraint_excludes_top_candidate() {
        let fixture = fixture();
        let core = graph(&fixture);
        let view = core.analysis_snapshot();
        let query = [1.0, 0.0];

        // Without constraints the unfiltered top candidate is `best`.
        let everything: Vec<&str> = fixture
            .nodes
            .iter()
            .filter(|(_, (kind, _))| *kind == "doc")
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(oracle_ranking(&fixture, query, &everything)[0], "best");

        // `best` cites a banned source: the forbidden-edge constraint excludes it
        // before ranking, so the ranking over the eligible set starts at `good`.
        let set = compile_constraints(
            &view,
            &constraints(vec![cites_banned(RelationshipQuantifier::NotExists)]),
            Some(4),
        )
        .unwrap();
        assert_eq!(set.graph_revision(), 4);
        let expected = oracle_eligible(&fixture);
        assert_eq!(
            set.ids().map(str::to_string).collect::<BTreeSet<_>>(),
            expected
        );
        assert!(!set.contains("best") && !set.contains("poor"));
        assert!(!set.contains("banned") && !set.contains("trusted"));
        let eligible: Vec<&str> = set.ids().collect();
        assert_eq!(oracle_ranking(&fixture, query, &eligible)[0], "good");

        // The required form selects exactly the complement among documents.
        let required = compile_constraints(
            &view,
            &constraints(vec![cites_banned(RelationshipQuantifier::Exists)]),
            Some(4),
        )
        .unwrap();
        assert_eq!(
            required.ids().collect::<Vec<_>>(),
            vec!["best", "poor"],
            "required edge keeps only documents that cite a banned source"
        );

        // A contradictory conjunction is an empty eligible set, not an error.
        let contradictory = compile_constraints(
            &view,
            &constraints(vec![
                cites_banned(RelationshipQuantifier::Exists),
                cites_banned(RelationshipQuantifier::NotExists),
            ]),
            Some(4),
        )
        .unwrap();
        assert!(contradictory.is_empty());

        // An entity predicate narrows further: kind = source matches no document.
        let none = RetrievalConstraints {
            graph_revision: 4,
            entity_predicates: BoundedVec::new(vec![
                EntityPredicate::Eq {
                    property: "kind".into(),
                    value: PredLiteral::Str("doc".into()),
                },
                EntityPredicate::Eq {
                    property: "kind".into(),
                    value: PredLiteral::Str("source".into()),
                },
            ])
            .unwrap(),
            relationship_predicates: BoundedVec::default(),
        };
        assert!(compile_constraints(&view, &none, Some(4))
            .unwrap()
            .is_empty());

        // A malformed predicate is refused before any node is read.
        let malformed = RetrievalConstraints {
            graph_revision: 4,
            entity_predicates: BoundedVec::new(vec![EntityPredicate::Eq {
                property: "".into(),
                value: PredLiteral::Bool(true),
            }])
            .unwrap(),
            relationship_predicates: BoundedVec::default(),
        };
        assert!(compile_constraints(&view, &malformed, Some(4)).is_err());
    }

    // spec: MEDULLA-FEDERATED-QUERY-R040.1.4
    #[test]
    fn refusals_are_typed_and_registered() {
        let fixture = fixture();
        let core = graph(&fixture);
        let view = core.analysis_snapshot();
        let pinned = constraints(vec![cites_banned(RelationshipQuantifier::NotExists)]);
        assert_eq!(pinned.graph_revision, 4);

        // The pin matches the revision the query reads: compiled.
        assert!(compile_constraints(&view, &pinned, Some(4)).is_ok());
        // An older or newer revision than the pin is stale.
        for current in [3, 5] {
            let stale = compile_constraints(&view, &pinned, Some(current)).unwrap_err();
            assert_eq!(
                stale,
                ConstraintError::RevisionStale {
                    pinned: 4,
                    current
                }
            );
            assert_eq!(stale.code().as_str(), "GRAPH_REVISION_STALE");
            assert!(stale.to_string().starts_with("GRAPH_REVISION_STALE"));
        }
        // A pin with no revision to compare against cannot be evaluated.
        let unevaluable = compile_constraints(&view, &pinned, None).unwrap_err();
        assert_eq!(unevaluable.code().as_str(), "CONSTRAINT_UNEVALUABLE");
        // Revision 0 declares no pin: it compiles whatever revision the query reads.
        let unpinned = RetrievalConstraints {
            graph_revision: 0,
            ..pinned.clone()
        };
        assert!(compile_constraints(&view, &unpinned, None).is_ok());
        assert!(compile_constraints(&view, &unpinned, Some(9)).is_ok());
        // A malformed predicate is invalid before the revision is compared.
        let malformed = RetrievalConstraints {
            graph_revision: 4,
            entity_predicates: BoundedVec::new(vec![EntityPredicate::Eq {
                property: " ".into(),
                value: PredLiteral::Bool(true),
            }])
            .unwrap(),
            relationship_predicates: BoundedVec::default(),
        };
        let invalid = compile_constraints(&view, &malformed, Some(5)).unwrap_err();
        assert_eq!(invalid.code().as_str(), "CONSTRAINT_INVALID");
        assert!(matches!(invalid, ConstraintError::Invalid(_)));
        // A contradictory but valid conjunction is an empty set, never an error.
        let contradictory = constraints(vec![
            cites_banned(RelationshipQuantifier::Exists),
            cites_banned(RelationshipQuantifier::NotExists),
        ]);
        assert!(compile_constraints(&view, &contradictory, Some(4))
            .unwrap()
            .is_empty());
    }
}
