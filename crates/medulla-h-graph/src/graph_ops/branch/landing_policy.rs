//! The landing policy check of the merge publish step (MEDULLA-BRANCH-MERGE-R009).
//!
//! Before a merge plans anything, `check` reads the `LandingPolicy` of the guarded branch
//! (MEDULLA-SCM-R008) and the sealed gate verdicts of the changeset at the source head. A
//! required gate with no verdict, a failed verdict, or a verdict whose `input_digest` is not the
//! digest of the head's tree blocks the merge with `LANDING_GATE_BLOCKED`. A branch with no
//! policy lands unchecked. The check writes nothing.

use std::sync::Arc;

use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::carrier::GraphReadAuthority;
use medulla_server_api::state::ServerState;
use medulla_source_types::gate_verdict::{
    gate_verdict_node_id, landing_gate_check, landing_policy_node_id, tree_listing_digest,
    GateVerdictRecord, LandingPolicy, GATE_VERDICT_RECORD_PROPERTY, LANDING_POLICY_RECORD_PROPERTY,
};
use medulla_source_types::source_changeset::{decode_changeset, Changeset};
use medulla_work_types::step_verdict::VerificationVerdict;
use tokio::sync::RwLock;
use {
    medulla_compute_assemble::source_tree::tree_at,
    medulla_compute_assemble::source_tree::TreeChain,
};

use super::publish::PublishError;
use super::time_travel::ChainEntry;

/// The source chain as a tree read sees it: the changesets of the commit payloads, no checkpoints.
struct MemoryChain<'a>(&'a [ChainEntry]);

impl TreeChain for MemoryChain<'_> {
    fn changeset_at(&self, rev: &str) -> Result<Option<Changeset>, String> {
        Ok(self
            .0
            .iter()
            .find(|(commit, _)| commit.id == rev)
            .and_then(|(_, payload)| payload.as_deref())
            .and_then(|bytes| decode_changeset(bytes).ok()))
    }

    fn checkpoint_at(&self, _rev: &str) -> Result<Option<Vec<u8>>, String> {
        Ok(None)
    }
}

/// The stored node `id` of `graph`, decoded, when the caller may see it.
fn visible_node(
    core: &medulla_graph_core::graph::GraphCore,
    authority: &GraphReadAuthority,
    id: &str,
) -> Option<serde_json::Value> {
    let blob = core.get_node_properties(id)?;
    if !authority.can_see_node(&blob, core.is_schema_node(id)) {
        return None;
    }
    medulla_types::msgpack::decode_property_value(&blob).ok()
}

/// Decide whether `source_head` of `source_chain` may land on `branch` of `repo`.
pub async fn check(
    state: &Arc<RwLock<ServerState>>,
    verified: &VerifiedRequestContext,
    repo: &str,
    branch: &str,
    source_chain: &[ChainEntry],
    source_head: &str,
) -> Result<(), PublishError> {
    medulla_server_api::api::reentry::ensure_graph_resident(state, repo).await;
    let (core, authority) = {
        let current = state.read().await;
        let Some(entry) = current.registry.get(repo) else {
            return Ok(());
        };
        let authority = GraphReadAuthority::from_verified(verified, &current.isolation)
            .map_err(PublishError::Failed)?;
        (Arc::clone(&entry.core), authority)
    };
    let Some(policy) = visible_node(&core, &authority, &landing_policy_node_id(repo, branch))
        .and_then(|node| {
            serde_json::from_str::<LandingPolicy>(
                node.get(LANDING_POLICY_RECORD_PROPERTY)?.as_str()?,
            )
            .ok()
        })
    else {
        return Ok(());
    };
    let head_changeset = source_chain
        .iter()
        .find(|(commit, _)| commit.id == source_head)
        .and_then(|(_, payload)| payload.as_deref())
        .and_then(|bytes| decode_changeset(bytes).ok());
    let mut verdicts: Vec<(String, VerificationVerdict)> = Vec::new();
    if let Some(changeset) = head_changeset {
        let tree = tree_at(&MemoryChain(source_chain), source_head, "", 0)
            .map_err(|error| PublishError::Failed(format!("{error:?}")))?;
        let head_digest = tree_listing_digest(
            tree.iter()
                .map(|entry| (entry.path.as_str(), entry.blob_digest.as_str(), entry.mode)),
        );
        for gate in &policy.required_gates {
            let stored: Option<GateVerdictRecord> = visible_node(
                &core,
                &authority,
                &gate_verdict_node_id(&changeset.id, gate),
            )
            .and_then(|node| {
                serde_json::from_str(node.get(GATE_VERDICT_RECORD_PROPERTY)?.as_str()?).ok()
            });
            // A verdict of another tree does not judge this head.
            if let Some(record) = stored.filter(|record| record.input_digest == head_digest) {
                verdicts.push((record.gate_id, record.verdict));
            }
        }
    }
    landing_gate_check(&policy, &verdicts)
        .map_err(|refusal| PublishError::Failed(refusal.to_string()))
}
