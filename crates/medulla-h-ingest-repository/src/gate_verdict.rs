//! Gate verdicts are records (MEDULLA-SCM-R008, MEDULLA-SDLC-R012.7).
//!
//! `record` decides one `RecordGateVerdict`: it checks the bounds, that the changeset node is
//! in the repository graph, that the verdict judged the changeset's own tree, and that no
//! different verdict is stored for the same gate and changeset. It returns the sealed record
//! and the one `judges` edge from the verdict node to the changeset node; the served caller
//! writes both in ONE `BatchUpdate`, so no verdict exists without its edge.

use medulla_source_types::gate_verdict::{
    gate_verdict_node_id, gate_verdict_seal, GateVerdictRecord,
};
use medulla_source_types::lifecycle::LifecycleRefusal;
use medulla_source_types::source_changeset::commit_node_id;

/// What one accepted verdict writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerdictWrite {
    /// The record to seal.
    pub record: GateVerdictRecord,
    /// The seal over the record.
    pub seal: String,
    /// The verdict node id, `gate_verdict:<changeset>:<gate_id>`.
    pub node_id: String,
    /// True when the same record was already stored, so the write replays.
    pub replayed: bool,
}

impl VerdictWrite {
    /// The one `judges` edge of the batch: the verdict node to `commit:<changeset>`.
    pub fn judges(&self) -> (String, String) {
        (self.node_id.clone(), commit_node_id(&self.record.changeset))
    }
}

/// Decide one verdict. `changeset_known` is whether `commit:<changeset>` is in the repository
/// graph; `tree_digest` is the digest of the changeset's canonical tree listing, `None` when
/// the tree cannot be read; `stored` is the verdict already held for the same gate and
/// changeset.
///
/// Refusals, in order: `INVALID_ARGUMENT`, `LIFECYCLE_ENDPOINT_UNKNOWN`,
/// `GATE_INPUT_DIGEST_MISMATCH`, `GATE_VERDICT_SEALED` (a different verdict is stored; an
/// identical one replays).
pub fn record(
    request: GateVerdictRecord,
    changeset_known: bool,
    tree_digest: Option<&str>,
    stored: Option<&GateVerdictRecord>,
) -> Result<VerdictWrite, LifecycleRefusal> {
    request.validate()?;
    let node_id = gate_verdict_node_id(&request.changeset, &request.gate_id);
    if !changeset_known {
        return Err(LifecycleRefusal::EndpointUnknown(commit_node_id(
            &request.changeset,
        )));
    }
    if tree_digest != Some(request.input_digest.as_str()) {
        return Err(LifecycleRefusal::GateInputDigestMismatch(format!(
            "{node_id}: input_digest is not the digest of the changeset tree"
        )));
    }
    let mut replayed = false;
    if let Some(held) = stored {
        if *held != request {
            return Err(LifecycleRefusal::GateVerdictSealed(node_id));
        }
        replayed = true;
    }
    Ok(VerdictWrite {
        seal: gate_verdict_seal(&request),
        record: request,
        node_id,
        replayed,
    })
}
