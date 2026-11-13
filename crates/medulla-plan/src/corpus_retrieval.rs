//! Corpus retrieval over the caller's projected core
//! (MEDULLA-FEDERATED-QUERY-R653.1 to R653.3, R653.8 to R653.10).
//!
//! One served `CorpusRetrieve` request selects a mode (grep, entity, session or
//! lineage) and returns typed rows. The wire types live in
//! `medulla_types::corpus_retrieval` and are re-exported here; the mode functions
//! read the core only through [`CorpusView`], which the served handler fills from
//! the row-projected core of the caller, so no mode can reach a row the caller
//! may not read. Nothing here writes.

use std::collections::{BTreeSet, VecDeque};

use regex::RegexBuilder;

pub use medulla_types::corpus_retrieval::*;

/// Largest snippet a hit carries, in bytes.
pub const SNIPPET_BYTES: usize = 200;
/// Compiled-program size limit of a grep pattern (1 MiB).
pub const REGEX_SIZE_LIMIT: usize = 1 << 20;
/// Lazy-DFA cache size limit of a grep pattern (2 MiB).
pub const REGEX_DFA_SIZE_LIMIT: usize = 2 << 20;
/// Hops the entity mode walks from the named entity.
pub const ENTITY_HOPS: u32 = 2;
/// Relationships the lineage mode follows from an answer to its sources.
pub const LINEAGE_RELATIONSHIPS: [&str; 3] = ["DERIVED_FROM", "GENERATED_BY", "CITES"];

/// Why a corpus request could not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorpusError {
    /// The request is outside its bounds.
    Request(CorpusRequestError),
    /// The mode needs a field the request left out.
    MissingArgument(&'static str),
    /// The pattern does not compile within the size limits.
    RegexTooComplex(String),
}

/// The text fields of one node, in field-name order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextNode {
    pub node_id: String,
    pub fields: Vec<(String, String)>,
}

/// One transcript turn read from the core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnRecord {
    pub session_id: String,
    pub ordinal: u32,
    pub role: String,
    pub text: String,
}

/// A node reached by the entity walk and the edge that reached it.
#[derive(Debug, Clone, PartialEq)]
pub struct WeightedNeighbour {
    pub node_id: String,
    pub relationship: String,
    pub weight: f64,
}

/// What the mode functions read of the caller's projected core.
pub trait CorpusView {
    /// Text nodes of `label` (all labels when `None`), any order.
    fn text_nodes(&self, label: Option<&str>) -> Vec<TextNode>;
    /// Nodes within `hops` of `entity`, the entity itself excluded; the one
    /// neighbour walk of `MEDULLA-FEDERATED-QUERY-R090.8`.
    fn neighbours(&self, entity: &str, hops: u32) -> Vec<WeightedNeighbour>;
    /// `TranscriptTurn` nodes of `session_id`, any order.
    fn session_turns(&self, session_id: &str) -> Vec<TurnRecord>;
    /// Edges leaving `node_id` over [`LINEAGE_RELATIONSHIPS`]; `node_id` of the
    /// hop is the target.
    fn sources(&self, node_id: &str) -> Vec<LineageHop>;
}

fn floor_boundary(text: &str, mut at: usize) -> usize {
    at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// At most [`SNIPPET_BYTES`] of `text`, starting a little before `start`.
fn snippet_around(text: &str, start: usize) -> String {
    let begin = floor_boundary(text, start.saturating_sub(40));
    let end = floor_boundary(text, (begin + SNIPPET_BYTES).min(text.len()));
    text[begin..end].to_string()
}

/// Every match of `pattern` in the text fields of `nodes`, in node id order,
/// stopping at `limit`. `line` counts from 1; `start` and `end` are byte
/// offsets inside that line.
pub fn grep_corpus(
    pattern: &str,
    nodes: &[TextNode],
    limit: usize,
) -> Result<Vec<GrepMatch>, CorpusError> {
    if pattern.len() > MAX_PATTERN_BYTES {
        return Err(CorpusError::RegexTooComplex(format!(
            "pattern is {} bytes; the limit is {MAX_PATTERN_BYTES}",
            pattern.len()
        )));
    }
    let regex = RegexBuilder::new(pattern)
        .size_limit(REGEX_SIZE_LIMIT)
        .dfa_size_limit(REGEX_DFA_SIZE_LIMIT)
        .build()
        .map_err(|e| CorpusError::RegexTooComplex(e.to_string()))?;
    let mut ordered: Vec<&TextNode> = nodes.iter().collect();
    ordered.sort_by(|a, b| a.node_id.cmp(&b.node_id));
    let mut out = Vec::new();
    for node in ordered {
        for (field, text) in &node.fields {
            for (index, line) in text.lines().enumerate() {
                for found in regex.find_iter(line) {
                    if out.len() >= limit {
                        return Ok(out);
                    }
                    out.push(GrepMatch {
                        node_id: node.node_id.clone(),
                        field: field.clone(),
                        line: index as u32 + 1,
                        start: found.start() as u32,
                        end: found.end() as u32,
                        snippet: snippet_around(line, found.start()),
                    });
                }
            }
        }
    }
    Ok(out)
}

/// The neighbourhood of `entity` within `hops`, ordered by edge weight
/// descending then node id, at most `limit` neighbours.
pub fn entity_context(
    entity: &str,
    hops: u32,
    view: &dyn CorpusView,
    limit: usize,
) -> EntityContext {
    let mut found = view.neighbours(entity, hops);
    found.retain(|n| n.node_id != entity);
    found.sort_by(|a, b| {
        b.weight
            .partial_cmp(&a.weight)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.node_id.cmp(&b.node_id))
    });
    found.truncate(limit);
    EntityContext {
        entity: entity.to_string(),
        hops,
        neighbours: found
            .into_iter()
            .map(|n| Neighbour {
                relationship: n.relationship,
                node_id: n.node_id,
            })
            .collect(),
    }
}

/// Turns of `session_id` ordered by ordinal; with `pattern`, only those whose
/// text holds it as a case-insensitive literal. Snippets are at most
/// [`SNIPPET_BYTES`].
pub fn session_hits(
    session_id: &str,
    pattern: Option<&str>,
    turns: &[TurnRecord],
    limit: usize,
) -> Vec<SessionHit> {
    let needle = pattern.map(str::to_lowercase).filter(|p| !p.is_empty());
    let mut chosen: Vec<&TurnRecord> = turns
        .iter()
        .filter(|t| t.session_id == session_id)
        .collect();
    chosen.sort_by(|a, b| a.ordinal.cmp(&b.ordinal));
    let mut out = Vec::new();
    for turn in chosen {
        let at = match &needle {
            None => 0,
            Some(n) => {
                let lowered = turn.text.to_lowercase();
                match lowered.find(n.as_str()) {
                    // Offsets of the lowered text only carry over when casing kept lengths.
                    Some(i) if lowered.len() == turn.text.len() => i,
                    Some(_) => 0,
                    None => continue,
                }
            }
        };
        if out.len() >= limit {
            break;
        }
        out.push(SessionHit {
            session_id: turn.session_id.clone(),
            ordinal: turn.ordinal,
            role: turn.role.clone(),
            snippet: snippet_around(&turn.text, at),
        });
    }
    out
}

/// Shortest paths from `answer_ref` to every source it reaches over
/// [`LINEAGE_RELATIONSHIPS`], breadth first, at most `depth` hops, at most
/// `limit` paths. A visited node is never repeated.
pub fn lineage_path(
    answer_ref: &str,
    depth: u32,
    limit: usize,
    view: &dyn CorpusView,
) -> Vec<LineagePath> {
    let mut seen: BTreeSet<String> = BTreeSet::from([answer_ref.to_string()]);
    let mut queue: VecDeque<(String, Vec<LineageHop>)> =
        VecDeque::from([(answer_ref.to_string(), Vec::new())]);
    let mut out = Vec::new();
    while let Some((node, path)) = queue.pop_front() {
        if path.len() as u32 >= depth {
            continue;
        }
        let mut edges = view.sources(&node);
        edges.sort_by(|a, b| {
            a.node_id
                .cmp(&b.node_id)
                .then(a.relationship.cmp(&b.relationship))
        });
        for edge in edges {
            if !seen.insert(edge.node_id.clone()) {
                continue;
            }
            let mut hops = path.clone();
            hops.push(edge.clone());
            out.push(LineagePath {
                answer_ref: answer_ref.to_string(),
                hops: hops.clone(),
            });
            if out.len() >= limit {
                return out;
            }
            queue.push_back((edge.node_id, hops));
        }
    }
    out
}

/// Run one request: validate it, then call the one function of its mode.
/// `lineage_depth` is `knob retrieval.lineage_depth`.
pub fn dispatch(
    request: &CorpusRequest,
    view: &dyn CorpusView,
    lineage_depth: u32,
) -> Result<CorpusResult, CorpusError> {
    request.validate().map_err(CorpusError::Request)?;
    let limit = request.limit as usize;
    let mut result = CorpusResult::default();
    match request.mode {
        CorpusMode::Grep => {
            let pattern = request
                .pattern
                .as_deref()
                .ok_or(CorpusError::MissingArgument("pattern"))?;
            let nodes = view.text_nodes(request.label.as_deref());
            result.matches = grep_corpus(pattern, &nodes, limit)?;
        }
        CorpusMode::Entity => {
            let entity = request
                .entity
                .as_deref()
                .ok_or(CorpusError::MissingArgument("entity"))?;
            result.entity = Some(entity_context(entity, ENTITY_HOPS, view, limit));
        }
        CorpusMode::Session => {
            let session = request
                .session_id
                .as_deref()
                .ok_or(CorpusError::MissingArgument("session_id"))?;
            let turns = view.session_turns(session);
            result.session = session_hits(session, request.pattern.as_deref(), &turns, limit);
        }
        CorpusMode::Lineage => {
            let answer = request
                .answer_ref
                .as_deref()
                .ok_or(CorpusError::MissingArgument("answer_ref"))?;
            result.lineage = lineage_path(answer, lineage_depth, limit, view);
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-FEDERATED-QUERY-R653.1
    #[test]
    fn types_round_trip_and_reject_oversize_pattern() {
        let request: CorpusRequest =
            serde_json::from_str(r#"{"mode":"grep","pattern":"fn main","label":"File"}"#).unwrap();
        assert_eq!(request.limit, DEFAULT_LIMIT);
        assert_eq!(request.validate(), Ok(()));
        let text = serde_json::to_string(&request).unwrap();
        assert_eq!(
            text,
            r#"{"mode":"grep","pattern":"fn main","label":"File","limit":50}"#
        );
        assert_eq!(
            serde_json::from_str::<CorpusRequest>(&text).unwrap(),
            request
        );
        assert!(serde_json::from_str::<CorpusRequest>(r#"{"mode":"guess"}"#).is_err());
        assert!(serde_json::from_str::<CorpusRequest>(r#"{"mode":"grep","tenant":"t"}"#).is_err());

        let edge = CorpusRequest {
            pattern: Some("a".repeat(MAX_PATTERN_BYTES)),
            limit: MAX_LIMIT,
            ..request.clone()
        };
        assert_eq!(edge.validate(), Ok(()));
        let oversize = CorpusRequest {
            pattern: Some("a".repeat(MAX_PATTERN_BYTES + 1)),
            ..request.clone()
        };
        assert_eq!(
            oversize.validate(),
            Err(CorpusRequestError::PatternTooLong(MAX_PATTERN_BYTES + 1))
        );
        for limit in [0, MAX_LIMIT + 1] {
            let bad = CorpusRequest {
                limit,
                ..request.clone()
            };
            assert_eq!(
                bad.validate(),
                Err(CorpusRequestError::LimitOutOfRange(limit))
            );
        }

        let grep = GrepMatch {
            node_id: "n1".into(),
            field: "body".into(),
            line: 3,
            start: 4,
            end: 11,
            snippet: "fn main()".into(),
        };
        let entity = EntityContext {
            entity: "e1".into(),
            hops: 1,
            neighbours: vec![Neighbour {
                relationship: "MENTIONS".into(),
                node_id: "n2".into(),
            }],
        };
        let session = SessionHit {
            session_id: "s1".into(),
            ordinal: 2,
            role: "user".into(),
            snippet: "hello".into(),
        };
        let lineage = LineagePath {
            answer_ref: "a1".into(),
            hops: vec![LineageHop {
                node_id: "doc-1".into(),
                relationship: "CITES".into(),
            }],
        };
        let back: GrepMatch = serde_json::from_str(&serde_json::to_string(&grep).unwrap()).unwrap();
        assert_eq!(back, grep);
        let back: EntityContext =
            serde_json::from_str(&serde_json::to_string(&entity).unwrap()).unwrap();
        assert_eq!(back, entity);
        let back: SessionHit =
            serde_json::from_str(&serde_json::to_string(&session).unwrap()).unwrap();
        assert_eq!(back, session);
        let back: LineagePath =
            serde_json::from_str(&serde_json::to_string(&lineage).unwrap()).unwrap();
        assert_eq!(back, lineage);
    }
}

#[cfg(test)]
mod mode_tests {
    use super::*;

    fn node(id: &str, field: &str, text: &str) -> TextNode {
        TextNode {
            node_id: id.into(),
            fields: vec![(field.into(), text.into())],
        }
    }

    struct Fixture {
        nodes: Vec<TextNode>,
        neighbours: Vec<WeightedNeighbour>,
        turns: Vec<TurnRecord>,
        edges: Vec<(String, LineageHop)>,
    }

    impl CorpusView for Fixture {
        fn text_nodes(&self, _label: Option<&str>) -> Vec<TextNode> {
            self.nodes.clone()
        }
        fn neighbours(&self, _entity: &str, _hops: u32) -> Vec<WeightedNeighbour> {
            self.neighbours.clone()
        }
        fn session_turns(&self, _session_id: &str) -> Vec<TurnRecord> {
            self.turns.clone()
        }
        fn sources(&self, node_id: &str) -> Vec<LineageHop> {
            self.edges
                .iter()
                .filter(|(from, _)| from == node_id)
                .map(|(_, hop)| hop.clone())
                .collect()
        }
    }

    fn fixture() -> Fixture {
        let neighbour = |id: &str, w: f64| WeightedNeighbour {
            node_id: id.into(),
            relationship: "MENTIONS".into(),
            weight: w,
        };
        let turn = |s: &str, o: u32, text: &str| TurnRecord {
            session_id: s.into(),
            ordinal: o,
            role: "user".into(),
            text: text.into(),
        };
        let hop = |from: &str, to: &str, rel: &str| {
            (
                from.to_string(),
                LineageHop {
                    node_id: to.into(),
                    relationship: rel.into(),
                },
            )
        };
        Fixture {
            nodes: vec![
                node("n2", "body", "alpha\nfoo bar foo"),
                node("n1", "body", "foo first"),
            ],
            neighbours: vec![
                neighbour("b", 0.5),
                neighbour("e", 1.0),
                neighbour("a", 0.5),
                neighbour("e0", 2.0),
            ],
            turns: vec![
                turn("s", 3, "third Needle"),
                turn("s", 1, "first"),
                turn("other", 2, "needle elsewhere"),
                turn("s", 2, "second needle here"),
            ],
            edges: vec![
                hop("ans", "src1", "DERIVED_FROM"),
                hop("ans", "src2", "CITES"),
                hop("src1", "deep", "GENERATED_BY"),
                hop("deep", "ans", "CITES"),
                hop("deep", "deeper", "CITES"),
            ],
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R653.2
    #[test]
    fn grep_order_limit_and_snippet() {
        let view = fixture();
        let all = grep_corpus("foo", &view.nodes, 10).unwrap();
        let ids: Vec<(&str, u32, u32, u32)> = all
            .iter()
            .map(|m| (m.node_id.as_str(), m.line, m.start, m.end))
            .collect();
        assert_eq!(
            ids,
            vec![("n1", 1, 0, 3), ("n2", 2, 0, 3), ("n2", 2, 8, 11)]
        );
        assert_eq!(grep_corpus("foo", &view.nodes, 2).unwrap().len(), 2);

        let long = node(
            "big",
            "body",
            &format!("{}needle{}", "x".repeat(300), "y".repeat(300)),
        );
        let hit = &grep_corpus("needle", &[long], 5).unwrap()[0];
        assert!(hit.snippet.len() <= SNIPPET_BYTES && hit.snippet.contains("needle"));

        // Linear-time engine: look-around is refused, an oversize program is refused.
        assert!(matches!(
            grep_corpus("(?=a)a", &view.nodes, 5),
            Err(CorpusError::RegexTooComplex(_))
        ));
        assert!(matches!(
            grep_corpus("(((a{100}){100}){100})", &view.nodes, 5),
            Err(CorpusError::RegexTooComplex(_))
        ));
        let too_long = "a".repeat(MAX_PATTERN_BYTES + 1);
        assert!(matches!(
            grep_corpus(&too_long, &view.nodes, 5),
            Err(CorpusError::RegexTooComplex(_))
        ));
    }

    // spec: MEDULLA-FEDERATED-QUERY-R653.8
    #[test]
    fn entity_context_order() {
        let view = fixture();
        let ctx = entity_context("e", ENTITY_HOPS, &view, 10);
        let ids: Vec<&str> = ctx.neighbours.iter().map(|n| n.node_id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["e0", "a", "b"],
            "weight descending, then id; entity excluded"
        );
        assert_eq!(ctx.hops, ENTITY_HOPS);
        assert_eq!(entity_context("e", 1, &view, 2).neighbours.len(), 2);
    }

    // spec: MEDULLA-FEDERATED-QUERY-R653.9
    #[test]
    fn session_hits_order() {
        let view = fixture();
        let all = session_hits("s", None, &view.turns, 10);
        let ordinals: Vec<u32> = all.iter().map(|h| h.ordinal).collect();
        assert_eq!(ordinals, vec![1, 2, 3]);
        let hits = session_hits("s", Some("NEEDLE"), &view.turns, 10);
        let ordinals: Vec<u32> = hits.iter().map(|h| h.ordinal).collect();
        assert_eq!(
            ordinals,
            vec![2, 3],
            "case-insensitive literal, this session only"
        );
        let long = TurnRecord {
            session_id: "s".into(),
            ordinal: 9,
            role: "assistant".into(),
            text: "z".repeat(500),
        };
        assert!(session_hits("s", None, &[long], 1)[0].snippet.len() <= SNIPPET_BYTES);
        assert_eq!(session_hits("s", None, &view.turns, 1).len(), 1);
    }

    // spec: MEDULLA-FEDERATED-QUERY-R653.10
    #[test]
    fn lineage_depth_and_cycles() {
        let view = fixture();
        let paths = lineage_path("ans", 8, 50, &view);
        let reached: Vec<&str> = paths
            .iter()
            .map(|p| p.hops.last().unwrap().node_id.as_str())
            .collect();
        // The cycle back to `ans` is never repeated; breadth-first, id order.
        assert_eq!(reached, vec!["src1", "src2", "deep", "deeper"]);
        assert_eq!(paths[2].hops.len(), 2, "shortest hop list");
        assert_eq!(paths[2].hops[1].relationship, "GENERATED_BY");
        let shallow = lineage_path("ans", 1, 50, &view);
        assert_eq!(shallow.len(), 2);
        assert_eq!(lineage_path("ans", 8, 3, &view).len(), 3);
    }

    // spec: MEDULLA-FEDERATED-QUERY-R653.3
    #[test]
    fn dispatch_calls_the_one_function_of_each_mode() {
        let view = fixture();
        let request = |mode, f: fn(&mut CorpusRequest)| {
            let mut r = CorpusRequest {
                mode,
                pattern: None,
                label: None,
                entity: None,
                session_id: None,
                answer_ref: None,
                limit: 50,
            };
            f(&mut r);
            r
        };
        let grep = request(CorpusMode::Grep, |r| r.pattern = Some("foo".into()));
        assert_eq!(
            dispatch(&grep, &view, 8).unwrap().matches,
            grep_corpus("foo", &view.nodes, 50).unwrap()
        );
        let entity = request(CorpusMode::Entity, |r| r.entity = Some("e".into()));
        assert_eq!(
            dispatch(&entity, &view, 8).unwrap().entity,
            Some(entity_context("e", ENTITY_HOPS, &view, 50))
        );
        let session = request(CorpusMode::Session, |r| r.session_id = Some("s".into()));
        assert_eq!(
            dispatch(&session, &view, 8).unwrap().session,
            session_hits("s", None, &view.turns, 50)
        );
        let lineage = request(CorpusMode::Lineage, |r| r.answer_ref = Some("ans".into()));
        assert_eq!(
            dispatch(&lineage, &view, 8).unwrap().lineage,
            lineage_path("ans", 8, 50, &view)
        );
        let bare = request(CorpusMode::Grep, |_| {});
        assert_eq!(
            dispatch(&bare, &view, 8),
            Err(CorpusError::MissingArgument("pattern"))
        );
        let bad = request(CorpusMode::Grep, |r| r.limit = 0);
        assert!(matches!(
            dispatch(&bad, &view, 8),
            Err(CorpusError::Request(CorpusRequestError::LimitOutOfRange(0)))
        ));
    }
}
