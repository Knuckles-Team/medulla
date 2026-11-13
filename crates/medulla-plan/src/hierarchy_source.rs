//! Graph-backed hierarchy sources for a served retrieval round
//! (MEDULLA-FEDERATED-QUERY-R081.9.3, R081.9.7.3, R081.9.7.4).
//!
//! [`GraphHierarchySource`] scores the stored `Community` hierarchy by the similarity of the
//! query vector to each summary's stored embedding. [`GraphSectionTopology`] reads stored
//! `SectionNode` graph nodes for the vector-free traversal and logs every record it reads so
//! the round can account for rejected branches. Both read only the snapshot the caller's
//! `PlanCtx` is bound to; the handler has already narrowed that snapshot to the caller's
//! tenant and row-level security, so a node outside the caller's scope is simply absent.
//!
//! Stored shapes (node id is the community or section id):
//! - `Community`: `summary_id`, `summary_revision`, `entity_ids`. The parent relation comes
//!   from the request policy's `parent_links`.
//! - `SectionNode`: `document_id`, `document_revision`, `parent_id`, `children`,
//!   `granularity` (`document` | `section` | `source_span`), `source_start`, `source_end`,
//!   `summary`, `source_digest`, `summary_digest`.

use std::cell::RefCell;

use medulla_graph_core::graph::GraphView;
use medulla_result_types::retrieval_verdict::{
    DescentStage, HierarchyCommunity, RetrievalRefusal, RETRIEVAL_HIERARCHY_INVALID,
};
use medulla_retrieval::leanrag::{
    SectionGranularity, SectionRecord, SectionSpan, SectionTopology, TraversalLimit,
    VectorFreeError,
};
use medulla_retrieval::retrieval_verdict::{HierarchySource, StageScore};
use medulla_semantic_index::semantic::SemanticStore;

/// The tenant label every visible community and entity carries: the snapshot is already
/// narrowed to the caller, so everything the source can see is the caller's.
pub const CALLER_SCOPE: &str = "caller";

/// Cost, in micro-units, one summary or entity score charges against the policy's budget.
pub const SCORE_COST_MICROS: u64 = 1;

fn text_of(view: &GraphView, id: &str) -> Option<String> {
    view.node_row_object(id)?
        .get("text")?
        .as_str()
        .map(str::to_string)
}

fn cosine(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na > 0.0 && nb > 0.0 {
        f64::from(dot / (na * nb))
    } else {
        0.0
    }
}

/// Scores stored summaries and entities against one query vector.
pub struct GraphHierarchySource<'a> {
    view: &'a GraphView,
    semantic: &'a SemanticStore,
    query: Vec<f32>,
}

impl<'a> GraphHierarchySource<'a> {
    pub fn new(view: &'a GraphView, semantic: &'a SemanticStore, query: Vec<f32>) -> Self {
        Self {
            view,
            semantic,
            query,
        }
    }
}

impl HierarchySource for GraphHierarchySource<'_> {
    fn score(&mut self, _stage: DescentStage, id: &str) -> StageScore {
        let score = self
            .semantic
            .get_embedding(id)
            .map_or(0.0, |stored| cosine(&self.query, &stored));
        let tokens = text_of(self.view, id).map_or(0, |t| t.split_whitespace().count() as u64);
        StageScore {
            score,
            tokens,
            cost: SCORE_COST_MICROS,
        }
    }

    fn entity_tenant(&self, entity_id: &str) -> Option<String> {
        self.view
            .has_node(entity_id)
            .then(|| CALLER_SCOPE.to_string())
    }
}

fn invalid(detail: String) -> RetrievalRefusal {
    RetrievalRefusal::new(RETRIEVAL_HIERARCHY_INVALID, detail)
}

/// Build the community list a policy names from the stored `Community` nodes.
pub fn load_communities(
    view: &GraphView,
    community_ids: &[String],
    summary_ids: &[String],
    parents: &[(String, String)],
) -> Result<Vec<HierarchyCommunity>, RetrievalRefusal> {
    community_ids
        .iter()
        .map(|id| {
            let props = view
                .node_row_object(id)
                .ok_or_else(|| invalid(format!("community `{id}` is not stored")))?;
            let text = |key: &str| {
                props
                    .get(key)
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .ok_or_else(|| invalid(format!("community `{id}` has no `{key}`")))
            };
            let summary_id = text("summary_id")?;
            if !summary_ids.contains(&summary_id) {
                return Err(invalid(format!(
                    "community `{id}` summary `{summary_id}` is not in the policy"
                )));
            }
            let entity_ids = props
                .get("entity_ids")
                .and_then(|v| v.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            Ok(HierarchyCommunity {
                community_id: id.clone(),
                parent_id: parents
                    .iter()
                    .find(|(_, child)| child == id)
                    .map(|(parent, _)| parent.clone()),
                summary_id,
                summary_revision: text("summary_revision")?,
                tenant: CALLER_SCOPE.to_string(),
                entity_ids,
            })
        })
        .collect()
}

/// One stored record read by a traversal, in read order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionRead {
    pub id: String,
    pub parent_id: Option<String>,
    pub summary_digest: String,
}

/// Reads stored `SectionNode` graph nodes and logs each read.
pub struct GraphSectionTopology<'a> {
    view: &'a GraphView,
    reads: RefCell<Vec<SectionRead>>,
}

impl<'a> GraphSectionTopology<'a> {
    pub fn new(view: &'a GraphView) -> Self {
        Self {
            view,
            reads: RefCell::new(Vec::new()),
        }
    }

    /// Every record the traversal read, in read order, without repeats.
    pub fn reads(&self) -> Vec<SectionRead> {
        self.reads.borrow().clone()
    }
}

fn granularity(name: &str) -> Option<SectionGranularity> {
    match name {
        "document" => Some(SectionGranularity::Document),
        "section" => Some(SectionGranularity::Section),
        "source_span" => Some(SectionGranularity::SourceSpan),
        _ => None,
    }
}

fn parse_section(view: &GraphView, id: &str) -> Option<SectionRecord> {
    let props = view.node_row_object(id)?;
    if props.get("type")?.as_str()? != "SectionNode" {
        return None;
    }
    let text = |key: &str| props.get(key)?.as_str().map(str::to_string);
    Some(SectionRecord {
        document_id: text("document_id")?,
        document_revision: text("document_revision")?,
        section_id: id.to_string(),
        parent_id: text("parent_id"),
        children: props
            .get("children")?
            .as_array()?
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        granularity: granularity(&text("granularity")?)?,
        source_span: SectionSpan {
            start: props.get("source_start")?.as_u64()?,
            end: props.get("source_end")?.as_u64()?,
        },
        summary: text("summary")?,
        source_digest: text("source_digest")?,
        summary_digest: text("summary_digest")?,
    })
}

impl SectionTopology for GraphSectionTopology<'_> {
    fn section(&self, section_id: &str) -> Option<SectionRecord> {
        let record = parse_section(self.view, section_id)?;
        let mut reads = self.reads.borrow_mut();
        if !reads.iter().any(|r| r.id == section_id) {
            reads.push(SectionRead {
                id: section_id.to_string(),
                parent_id: record.parent_id.clone(),
                summary_digest: record.summary_digest.clone(),
            });
        }
        Some(record)
    }
}

/// Map a vector-free traversal failure to its typed refusal (R081.9.7.4). A malformed tree
/// is `RETRIEVAL_HIERARCHY_INVALID`; an exhausted limit is `RETRIEVAL_BUDGET_EXCEEDED` and
/// names the limit. A limit refusal stops before the next record is read and returns no
/// receipt, so its detail states the walk is incomplete.
pub fn section_refusal(error: VectorFreeError) -> RetrievalRefusal {
    use medulla_result_types::retrieval_verdict::RETRIEVAL_BUDGET_EXCEEDED;
    match error {
        VectorFreeError::TraversalLimitExceeded(limit) => {
            let name = match limit {
                TraversalLimit::Depth => "depth",
                TraversalLimit::Visited => "visited",
            };
            RetrievalRefusal::new(
                RETRIEVAL_BUDGET_EXCEEDED,
                format!("section traversal {name} limit exhausted; incomplete: true"),
            )
        }
        other => invalid(format!("section tree is invalid: {other:?}")),
    }
}
