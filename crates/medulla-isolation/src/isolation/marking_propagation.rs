//! Marking propagation along lineage (MEDULLA-TYPED-PACKS-R652.2).
//!
//! A promoted classification claim propagates its markings to every asset
//! downstream of it along `cat:lineageTo` edges, as `proposed` claims whose
//! provenance is `propagated`. [`propagate`] is a breadth-first search with a
//! visited set, so the closure is monotone (a node reached once is never
//! reached again, whatever the number of paths), costs O(V + E), is bounded by
//! `classification.propagation_depth_max` hops and is deterministic: neighbours
//! are visited in sorted order.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use medulla_identity_types::classification::{
    ClaimKind, ClaimProvenance, ClaimState, ClassificationClaim,
};
use medulla_identity_types::tenancy::Marking;

/// The `cat:lineageTo` adjacency: each node and the nodes directly downstream.
pub type LineageEdges = BTreeMap<String, BTreeSet<String>>;

/// The claims a promotion of `categories` on `source` proposes downstream.
///
/// `evidence` is copied onto each propagated claim. A node in `already_claimed`
/// (it holds a claim for these categories already) and the source itself are not
/// proposed again, though the walk continues through them. Nodes further than
/// `depth_max` hops from the source are not reached.
pub fn propagate(
    edges: &LineageEdges,
    source: &str,
    categories: &[Marking],
    evidence: &[String],
    depth_max: u32,
    already_claimed: &BTreeSet<String>,
) -> Vec<ClassificationClaim> {
    let mut visited: BTreeSet<&str> = BTreeSet::from([source]);
    let mut queue: VecDeque<(&str, u32)> = VecDeque::from([(source, 0)]);
    let mut claims = Vec::new();
    while let Some((node, depth)) = queue.pop_front() {
        if depth >= depth_max {
            continue;
        }
        let Some(downstream) = edges.get(node) else {
            continue;
        };
        for next in downstream {
            if !visited.insert(next.as_str()) {
                continue;
            }
            if !already_claimed.contains(next) {
                claims.push(ClassificationClaim {
                    asset: next.clone(),
                    categories: categories.to_vec(),
                    provenance: ClaimProvenance {
                        kind: ClaimKind::Propagated,
                        detector_id: None,
                        sample_size: None,
                        hit_ratio: None,
                        scanned_at_ms: None,
                    },
                    state: ClaimState::Proposed,
                    evidence: evidence.to_vec(),
                });
            }
            queue.push_back((next.as_str(), depth + 1));
        }
    }
    claims
}

/// Whether a claim may move from `from` to `to`: only `proposed` moves, to
/// `promoted` or `rejected`.
pub fn legal_move(from: ClaimState, to: ClaimState) -> bool {
    from == ClaimState::Proposed && matches!(to, ClaimState::Promoted | ClaimState::Rejected)
}
