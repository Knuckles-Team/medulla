//! MEDULLA-DECISION-ENGINE-R813.3: the score rows a `ProgramOptimize` submit
//! builds its corpus from.
//!
//! The rows are the `:Score` nodes the verified caller can see in the graph the
//! request names. They are read here, on the caller's own row-level-security
//! projection (`GraphReadAuthority::project_core`), at submit time. The client
//! supplies the graph name only: the examples, their grader kinds and the corpus
//! snapshot version all come from the store. An unknown graph and a graph the
//! caller may not read refuse with the same text.

use super::prelude_jobs::*;
use super::prelude_server::*;
use super::prelude_std::*;
use super::*;

use medulla_graph_core::graph::GraphCore;
use medulla_program::corpus::{StoredScore, StoredScoreSet};
use medulla_program::{ExampleSplit, GraderKind, MAX_EXAMPLES};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::carrier::GraphReadAuthority;

/// The node label of one stored score.
const SCORE_LABEL: &str = "Score";
/// The edge relation that links a score to the run it evaluates.
const SCORES_RUN: &str = "scoresRun";
/// The node property that links a score to the output it observed.
const OBSERVED_OUTPUT: &str = "observed_output";

fn unknown_graph(req_id: u64, graph: &str) -> Response {
    Response::err(req_id, format!("INVALID_ARGUMENT: unknown graph '{graph}'"))
}

/// Read the caller's visible score rows when the request names a score source;
/// `None` for every other submit.
pub(super) async fn program_score_rows(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    verified: &VerifiedRequestContext,
    kind: &JobKind,
) -> Result<Option<StoredScoreSet>, Response> {
    let JobKind::ProgramOptimize { request_msgpack } = kind else {
        return Ok(None);
    };
    let Some(request) = decode_optimization_request(request_msgpack) else {
        // The kind-governance step refuses an undecodable request.
        return Ok(None);
    };
    let Some(source) = request.score_source else {
        return Ok(None);
    };
    let (core, authority) = {
        let guard = state.read().await;
        let Some(entry) = guard.registry.get(&source.graph) else {
            return Err(unknown_graph(req_id, &source.graph));
        };
        if check_graph_access(
            &guard.isolation,
            Some(verified.agent_id()),
            &source.graph,
            entry.graph_type,
            entry.owner.as_deref(),
            AccessLevel::Read,
        )
        .is_err()
        {
            return Err(unknown_graph(req_id, &source.graph));
        }
        let authority = GraphReadAuthority::from_verified(verified, &guard.isolation)
            .map_err(|error| Response::err(req_id, error))?;
        (Arc::clone(&entry.core), authority)
    };
    let snapshot_version = core.version();
    let projected = authority.project_core(&core);
    let scores = read_stored_scores(&projected)
        .map_err(|error| Response::err(req_id, format!("INVALID_ARGUMENT: {error}")))?;
    Ok(Some(StoredScoreSet {
        snapshot_version,
        scores,
    }))
}

fn number(props: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<f64> {
    props.get(key).and_then(serde_json::Value::as_f64)
}

fn text(props: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<String> {
    props
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// Every `:Score` node of `core`, in node-id order, with its `:scoresRun` link.
/// A score whose properties do not decode, whose grader kind is outside the five
/// kinds, or whose value or range is missing refuses the read; a score over the
/// corpus ceiling refuses it too, so a corpus is never silently truncated.
pub(super) fn read_stored_scores(core: &GraphCore) -> Result<Vec<StoredScore>, String> {
    let nodes = core.get_nodes_by_label(SCORE_LABEL, MAX_EXAMPLES + 1);
    if nodes.len() > MAX_EXAMPLES {
        return Err(
            "SCORE_CORPUS_TOO_LARGE: the graph holds more scores than a corpus carries".to_string(),
        );
    }
    let mut runs: BTreeMap<String, String> = BTreeMap::new();
    for (source, target, blob) in core.get_edges() {
        let relation = medulla_types::msgpack::decode_property_object(&blob)
            .ok()
            .and_then(|props| text(&props, "relationship"));
        if relation.as_deref() == Some(SCORES_RUN) {
            runs.entry(source).or_insert(target);
        }
    }
    let mut scores = Vec::with_capacity(nodes.len());
    for (node_id, blob) in nodes {
        let props = medulla_types::msgpack::decode_property_object(&blob)
            .map_err(|_| format!("SCORE_INVALID: score '{node_id}' does not decode"))?;
        let (Some(value), Some(range_min), Some(range_max)) = (
            number(&props, "value"),
            number(&props, "range_min"),
            number(&props, "range_max"),
        ) else {
            return Err(format!(
                "SCORE_OUT_OF_RANGE: score '{node_id}' has no value or range"
            ));
        };
        let grader_kind: GraderKind = serde_json::from_value(
            props
                .get("grader_kind")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        )
        .map_err(|_| format!("SCORE_GRADER_UNKNOWN: score '{node_id}' has no known grader kind"))?;
        let split =
            match props.get("split") {
                None | Some(serde_json::Value::Null) => None,
                Some(raw) => Some(serde_json::from_value::<ExampleSplit>(raw.clone()).map_err(
                    |_| format!("SCORE_INVALID: score '{node_id}' has an unknown split"),
                )?),
            };
        let run_id = runs.get(&node_id).cloned();
        scores.push(StoredScore {
            input_ids: run_id.iter().cloned().collect(),
            run_id,
            value,
            range_min,
            range_max,
            grader_kind,
            author: text(&props, "author"),
            split,
            observed_output_id: text(&props, OBSERVED_OUTPUT),
            node_id,
        });
    }
    Ok(scores)
}

fn token(label: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(label.as_bytes()))
}

fn minted(namespace: &str, id: &str) -> Result<OpaqueRef, String> {
    OpaqueRef::new(native_opaque_ref(namespace, id)).map_err(|error| error.to_string())
}

fn score_evidence(
    score_ref: &OpaqueRef,
    node_id: &str,
    snapshot_version: u64,
    envelope: &PolicyEnvelope,
) -> Result<medulla_program::EvidenceBinding, String> {
    use medulla_modality::{
        ArtifactId, DerivationId, EvidenceAddress, EvidenceLocus, EvidenceLocusId, ResourceId,
    };
    let invalid = |error: medulla_modality::ProtocolError| error.to_string();
    Ok(medulla_program::EvidenceBinding {
        modality: ProgramModality::Graph,
        locus: EvidenceLocus {
            id: EvidenceLocusId::from_token(&token(&format!("locus:{node_id}")))
                .map_err(invalid)?,
            subject: ResourceId::Artifact(
                ArtifactId::from_token(&token(&format!("artifact:{node_id}"))).map_err(invalid)?,
            ),
            address: EvidenceAddress::RowVersion {
                row_ref: score_ref.clone(),
                version: snapshot_version,
            },
            policy_ref: envelope.access_policy_ref.clone(),
            derivation_ref: DerivationId::from_token(&token(&format!("derivation:{node_id}")))
                .map_err(invalid)?,
        },
        policy: envelope.clone(),
    })
}

/// The score rows of one read, each bound to the verified caller's policy
/// envelope with a graph evidence locus at the read's snapshot version.
pub(super) fn corpus_rows(
    set: &StoredScoreSet,
    envelope: &PolicyEnvelope,
) -> Result<Vec<medulla_program::corpus::ScoreRow>, String> {
    let mut rows = Vec::with_capacity(set.scores.len());
    for stored in &set.scores {
        let score_ref = minted("score", &stored.node_id)?;
        let input_refs = stored
            .input_ids
            .iter()
            .map(|id| minted("run", id))
            .collect::<Result<Vec<_>, _>>()?;
        rows.push(medulla_program::corpus::ScoreRow {
            evidence: vec![score_evidence(
                &score_ref,
                &stored.node_id,
                set.snapshot_version,
                envelope,
            )?],
            run_ref: stored
                .run_id
                .as_deref()
                .map(|id| minted("run", id))
                .transpose()?,
            author: stored
                .author
                .as_deref()
                .map(|id| minted("author", id))
                .transpose()?,
            observed_output_ref: stored
                .observed_output_id
                .as_deref()
                .map(|id| minted("output", id))
                .transpose()?,
            score_ref,
            value: stored.value,
            range_min: stored.range_min,
            range_max: stored.range_max,
            grader_kind: stored.grader_kind,
            split: stored.split,
            input_refs,
        });
    }
    Ok(rows)
}
