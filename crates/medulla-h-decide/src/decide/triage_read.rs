//! The facts a `defect_triage` question reads (MEDULLA-SDLC-R004.3): the stored defect, its
//! stack frames, the frames' symbols and blame, and the other open defects of the repository.
//! One read, before the executor, behind the ordinary read ACL; the family never reads the graph.
//! The graph is the tenant's own, so another tenant's defect is absent.

use std::collections::{BTreeMap, BTreeSet};

use medulla_decision_outcome_types::decision::defect_triage::{OpenDefect, SuspectInput};
use medulla_graph_core::graph::GraphCore;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_source_types::lifecycle::{
    defect_from_node, Defect, DefectStatus, DEFECT_TYPE,
};
use medulla_source_types::source_changeset::COMMIT_TYPE;
use medulla_source_types::stack_frame::{SymbolIndex, SymbolRef, SYMBOL_TYPE};

use super::{topology_read, SharedState};

/// The id prefix of a code symbol node.
const SYMBOL_ID_PREFIX: &str = "symbol:";

/// One stack frame of the defect's symptoms: its digest and the suspect it resolved to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FrameFact {
    pub digest: String,
    pub suspect: Option<SuspectInput>,
}

/// Everything one triage question reads.
#[derive(Clone, Debug, Default)]
pub(super) struct TriageFacts {
    pub defect: Option<Defect>,
    pub frames: Vec<FrameFact>,
    /// The author of each blame changeset, by changeset id.
    pub authors: BTreeMap<String, String>,
    pub open: Vec<OpenDefect>,
}

fn number(node: &serde_json::Value, key: &str) -> Option<u64> {
    let value = node.get(key)?;
    value.as_u64().or_else(|| value.as_str()?.parse().ok())
}

/// The blame of one symbol: the `Changeset` node with an edge to it that has the greatest
/// `timestamp_ms` (the lowest changeset id on a tie), with its author.
fn blame_of(core: &GraphCore, symbol: &str) -> Option<(SuspectInput, Option<String>)> {
    let mut best: Option<(u64, String, Option<String>)> = None;
    for predecessor in core.get_predecessors(symbol).unwrap_or_default() {
        let Some(blob) = core.get_node_properties(&predecessor) else {
            continue;
        };
        let Ok(node) = medulla_types::msgpack::decode_property_value(&blob) else {
            continue;
        };
        if node.get("type").and_then(|v| v.as_str()) != Some(COMMIT_TYPE) {
            continue;
        }
        let changeset = node
            .get("changeset_id")
            .and_then(|v| v.as_str())
            .map(str::to_owned)
            .or_else(|| predecessor.strip_prefix("commit:").map(str::to_owned));
        let (Some(changeset), Some(at)) = (changeset, number(&node, "timestamp_ms")) else {
            continue;
        };
        let author = node.get("author").and_then(|v| v.as_str()).map(str::to_owned);
        let better = match &best {
            None => true,
            Some((held_at, held_id, _)) => {
                at > *held_at || (at == *held_at && changeset < *held_id)
            }
        };
        if better {
            best = Some((at, changeset, author));
        }
    }
    best.map(|(at, changeset, author)| {
        (
            SuspectInput {
                symbol_id: symbol.to_owned(),
                blame_changeset: changeset,
                blame_at: at,
            },
            author,
        )
    })
}

fn is_open(status: DefectStatus) -> bool {
    !matches!(status, DefectStatus::Closed | DefectStatus::Rejected)
}

/// Read the facts of triaging `defect_id` from `core`, for `tenant`.
pub(super) fn facts_of(core: &GraphCore, tenant: &str, defect_id: &str) -> TriageFacts {
    let mut defects: Vec<Defect> = core
        .get_nodes_by_label(DEFECT_TYPE, 0)
        .into_iter()
        .filter_map(|(_, blob)| {
            let node = medulla_types::msgpack::decode_property_value(&blob).ok()?;
            defect_from_node(&node, tenant)
        })
        .collect();
    defects.sort_by(|a, b| (&a.repo, &a.id).cmp(&(&b.repo, &b.id)));
    let Some(defect) = defects.iter().find(|d| d.id == defect_id).cloned() else {
        return TriageFacts::default();
    };
    let index = SymbolIndex::new(
        core.get_nodes_by_label(SYMBOL_TYPE, 0)
            .into_iter()
            .filter_map(|(id, blob)| {
                let node = medulla_types::msgpack::decode_property_value(&blob).ok()?;
                SymbolRef::from_node(&id, &node)
            })
            .collect(),
    );
    let mut authors = BTreeMap::new();
    let frames: Vec<FrameFact> = defect
        .symptoms
        .iter()
        .flat_map(|symptom| symptom.text.split('\n'))
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            let suspect = index
                .resolve_frame(line)
                .and_then(|symbol| blame_of(core, &symbol.id))
                .map(|(suspect, author)| {
                    if let Some(author) = author {
                        authors.insert(suspect.blame_changeset.clone(), author);
                    }
                    suspect
                });
            FrameFact {
                digest: medulla_core::lifecycle::defect::signature_digest(line),
                suspect,
            }
        })
        .collect();
    let open: Vec<OpenDefect> = defects
        .iter()
        .filter(|other| other.repo == defect.repo && other.id != defect.id && is_open(other.status))
        .map(|other| {
            let suspects: BTreeSet<String> = core
                .get_successors(&medulla_source_types::lifecycle::defect_node_id(
                    &other.repo,
                    &other.id,
                ))
                .unwrap_or_default()
                .into_iter()
                .filter(|id| id.starts_with(SYMBOL_ID_PREFIX))
                .collect();
            OpenDefect {
                defect_id: other.id.clone(),
                repository: other.repo.clone(),
                suspects: suspects.into_iter().collect(),
            }
        })
        .collect();
    TriageFacts {
        defect: Some(defect),
        frames,
        authors,
        open,
    }
}

/// The facts of triaging `defect_id` in `graph`, read under the caller's read access. A graph
/// the caller may not read refuses as every other read does.
pub(super) async fn read_triage_facts(
    state: &SharedState,
    graph: &str,
    verified: &VerifiedRequestContext,
    defect_id: &str,
) -> Result<TriageFacts, String> {
    let core = topology_read::graph_core(state, graph, verified).await?;
    let (tenant, defect_id) = (verified.tenant().to_owned(), defect_id.to_owned());
    tokio::task::spawn_blocking(move || facts_of(&core, &tenant, &defect_id))
        .await
        .map_err(|error| format!("triage read task failed: {error}"))
}
