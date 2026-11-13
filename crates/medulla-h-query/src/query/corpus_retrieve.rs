//! `Method::CorpusRetrieve` (MEDULLA-FEDERATED-QUERY-R653.3): grep, entity,
//! session and lineage reads over the caller's row-projected core.
//!
//! The handler only fills a [`CorpusView`] from the projected core and calls
//! `medulla_plan::corpus_retrieval::dispatch`; every mode lives there once. The
//! entity mode walks with the one `expand_hops` of `medulla-graph-core`
//! (MEDULLA-FEDERATED-QUERY-R090.8). Rows the caller cannot read are not in the
//! projected core, so no mode reaches them. Nothing here writes.

use super::*;
use medulla_graph_core::graph::{edge_weight, expand_hops};
use medulla_plan::corpus_retrieval::{
    dispatch, CorpusError, CorpusRequestError, CorpusView, LineageHop, TextNode, TurnRecord,
    WeightedNeighbour, LINEAGE_RELATIONSHIPS,
};
use medulla_protocol_types::result_contract::query::CorpusRetrieve;
use medulla_types::corpus_retrieval::CorpusRequest;

/// Node type of one ingested transcript turn.
const TRANSCRIPT_TURN: &str = "TranscriptTurn";
/// Most nodes one entity walk returns before the request `limit` applies.
const WALK_CAP: usize = 10_000;

fn object_of(core: &GraphCore, id: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    let blob = core.get_node_properties(id)?;
    Some(medulla_types::msgpack::decode_property_object(&blob).unwrap_or_default())
}

fn str_of(object: &serde_json::Map<String, serde_json::Value>, key: &str) -> String {
    object
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn node_type(object: &serde_json::Map<String, serde_json::Value>) -> String {
    match str_of(object, "type") {
        t if t.is_empty() => str_of(object, "node_type"),
        t => t,
    }
}

/// The relationship names of the edges from `from` to `to`.
fn relationships(core: &GraphCore, from: &str, to: &str) -> Vec<String> {
    core.get_edge_properties(from, to)
        .iter()
        .filter_map(|blob| {
            let object = medulla_types::msgpack::decode_property_object(blob).unwrap_or_default();
            ["relationship", "type", "edge_type"]
                .iter()
                .map(|key| str_of(&object, key))
                .find(|v| !v.is_empty())
        })
        .collect()
}

/// The caller's projected core seen through the mode functions.
struct CoreView<'a> {
    core: &'a GraphCore,
}

impl CorpusView for CoreView<'_> {
    fn text_nodes(&self, label: Option<&str>) -> Vec<TextNode> {
        let mut ids = self.core.node_ids();
        ids.sort();
        ids.into_iter()
            .filter_map(|id| {
                let object = object_of(self.core, &id)?;
                if label.is_some_and(|l| node_type(&object) != l) {
                    return None;
                }
                let mut fields: Vec<(String, String)> = object
                    .iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                    .collect();
                fields.sort();
                Some(TextNode {
                    node_id: id,
                    fields,
                })
            })
            .collect()
    }

    fn neighbours(&self, entity: &str, hops: u32) -> Vec<WeightedNeighbour> {
        let knobs = medulla_server_api::knob_config::knobs();
        let fanout = usize::try_from(knobs.discover_hop_fanout).unwrap_or(usize::MAX);
        let hits = expand_hops(
            self.core,
            &[(entity.to_string(), 1.0)],
            hops,
            fanout,
            knobs.discover_hop_decay,
            WALK_CAP,
        );
        hits.into_iter()
            .filter(|hit| hit.id != entity)
            .map(|hit| {
                let parent = hit
                    .path
                    .len()
                    .checked_sub(2)
                    .and_then(|i| hit.path.get(i))
                    .map_or(entity, String::as_str);
                let mut names = relationships(self.core, parent, &hit.id);
                names.extend(relationships(self.core, &hit.id, parent));
                WeightedNeighbour {
                    relationship: names.into_iter().next().unwrap_or_default(),
                    weight: edge_weight(self.core, parent, &hit.id),
                    node_id: hit.id,
                }
            })
            .collect()
    }

    fn session_turns(&self, session_id: &str) -> Vec<TurnRecord> {
        self.core
            .node_ids()
            .into_iter()
            .filter_map(|id| {
                let object = object_of(self.core, &id)?;
                if node_type(&object) != TRANSCRIPT_TURN
                    || str_of(&object, "session_id") != session_id
                {
                    return None;
                }
                let text = match str_of(&object, "content") {
                    t if t.is_empty() => str_of(&object, "text"),
                    t => t,
                };
                Some(TurnRecord {
                    session_id: session_id.to_string(),
                    ordinal: object
                        .get("ordinal")
                        .and_then(serde_json::Value::as_u64)
                        .and_then(|o| u32::try_from(o).ok())?,
                    role: str_of(&object, "role"),
                    text,
                })
            })
            .collect()
    }

    fn sources(&self, node_id: &str) -> Vec<LineageHop> {
        let mut out = Vec::new();
        for (from, neighbours) in self.core.get_neighbors_batch(vec![node_id.to_string()]) {
            for to in neighbours {
                for relationship in relationships(self.core, &from, &to) {
                    if LINEAGE_RELATIONSHIPS.contains(&relationship.as_str()) {
                        out.push(LineageHop {
                            node_id: to.clone(),
                            relationship,
                        });
                    }
                }
            }
        }
        out
    }
}

fn refusal(error: CorpusError) -> String {
    match error {
        CorpusError::Request(CorpusRequestError::LimitOutOfRange(limit)) => {
            format!("INVALID_ARGUMENT: limit {limit} is outside [1, 500]")
        }
        CorpusError::Request(CorpusRequestError::PatternTooLong(len)) => {
            format!("CORPUS_REGEX_TOO_COMPLEX: pattern is {len} bytes; the limit is 1024")
        }
        CorpusError::MissingArgument(field) => {
            format!("INVALID_ARGUMENT: this mode needs `{field}`")
        }
        CorpusError::RegexTooComplex(detail) => format!("CORPUS_REGEX_TOO_COMPLEX: {detail}"),
    }
}

/// Serve one `CorpusRetrieve`.
pub(crate) async fn handle_corpus_retrieve(
    ctx: &QueryHandlerCtx<'_>,
    request: CorpusRequest,
) -> Response {
    let req_id = ctx.req_id;
    let core = Arc::clone(ctx.core);
    let depth = u32::try_from(medulla_server_api::knob_config::knobs().retrieval_lineage_depth)
        .unwrap_or(u32::MAX);
    let outcome = compute_off_lock(req_id, move || {
        dispatch(&request, &CoreView { core: &core }, depth)
    })
    .await;
    match outcome {
        Err(response) => response,
        Ok(Err(error)) => Response::err(req_id, refusal(error)),
        Ok(Ok(result)) => served_response::<CorpusRetrieve>(
            req_id,
            Ok(Ok(result)),
            "CorpusRetrieve",
            #[cfg(feature = "result-cache")]
            ctx.core,
            #[cfg(feature = "result-cache")]
            None,
        ),
    }
}
