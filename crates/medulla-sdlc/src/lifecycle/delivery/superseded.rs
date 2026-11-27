//! A person's proposal that carries the work supersedes the delivery (MEDULLA-SDLC-R019.12).
//!
//! The verdict is `close_superseded` for a person's proposal that contains every changeset of
//! the delivery branch. Containment is equal changeset ids or equal per-path content digests
//! (the tree materialization of MEDULLA-SCM-R004). A proposal that is only related never
//! supersedes. The verdict becomes an input of the delivery.

use std::collections::{BTreeMap, BTreeSet};

use medulla_source_types::delivery::{Delivery, DeliveryInput, FeedbackKind, InputSource};

/// One changeset of the delivery branch: its id and the content digest of every path it
/// changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchChangeset {
    /// The changeset id.
    pub id: String,
    /// The content digest of every path the changeset changed, by path.
    pub path_digests: BTreeMap<String, String>,
}

/// Another proposal as the verdict reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CandidateProposal {
    /// The id of the proposal.
    pub proposal_id: String,
    /// A person opened it.
    pub by_person: bool,
    /// The ids of the changesets it holds.
    pub changeset_ids: BTreeSet<String>,
    /// The content digest of every path of its tree.
    pub path_digests: BTreeMap<String, String>,
}

/// The verdict of one candidate against one delivery branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SupersededVerdict {
    /// The candidate holds nothing the delivery branch lacks: close it as superseded.
    CloseSuperseded,
    /// The candidate holds work the delivery branch lacks: leave it open.
    Keep,
}

/// Whether `candidate` contains `changeset`: the same changeset id, or every path the
/// changeset changed holds the same content digest in the candidate's tree.
fn contains(candidate: &CandidateProposal, changeset: &BranchChangeset) -> bool {
    candidate.changeset_ids.contains(&changeset.id)
        || (!changeset.path_digests.is_empty()
            && changeset
                .path_digests
                .iter()
                .all(|(path, digest)| candidate.path_digests.get(path) == Some(digest)))
}

/// The verdict for `candidate`: `close_superseded` only when a person opened it, the delivery
/// branch holds at least one changeset, and the candidate contains every one of them.
pub fn superseded_verdict(
    branch: &[BranchChangeset],
    candidate: &CandidateProposal,
) -> SupersededVerdict {
    if candidate.by_person && !branch.is_empty() && branch.iter().all(|c| contains(candidate, c)) {
        SupersededVerdict::CloseSuperseded
    } else {
        SupersededVerdict::Keep
    }
}

/// Write a `close_superseded` verdict as an input of the delivery.
pub fn record_superseded(delivery: &Delivery, event_id: &str, by: &str) -> Delivery {
    let mut next = delivery.clone();
    let seq = next.next_seq();
    next.inputs.push(DeliveryInput {
        event_id: event_id.to_owned(),
        kind: FeedbackKind::HeadPushed,
        source: InputSource::Automation,
        seq,
        detail: Some(format!("close_superseded:{by}")),
    });
    next
}
