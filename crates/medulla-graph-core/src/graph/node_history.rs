//! Version-indexed node history (MEDULLA-DECISION-ENGINE-R121.2).
//!
//! The live property map holds only the newest image of a node. A decision that
//! records "this row, read at graph version V" must be replayable later, after the
//! graph has moved on, so the graph keeps the image a write replaced. A write that
//! replaces or removes a node records the prior image (or its absence); the next
//! version bump stamps it with the version the replacement became visible at. The
//! image of a node at version `v` is then the prior image of the first replacement
//! stamped after `v`, or the live image when no replacement is stamped after `v`.
//!
//! Retention is bounded by the registry knob `graph.node_history_revisions`
//! (installed once at startup through [`install_node_history_limit`]). Revisions
//! past the bound are dropped oldest first and raise the history floor, so a read
//! below the floor refuses [`NodeHistoryError::Retired`] instead of answering from
//! a history that no longer reaches back that far. A whole-image replacement
//! (clear, snapshot replace, adopted materialization) also raises the floor: the
//! history before it describes a different image.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;

/// The default retention, equal to the registry knob's default.
pub const DEFAULT_NODE_HISTORY_REVISIONS: usize = 200_000;

static NODE_HISTORY_LIMIT: AtomicUsize = AtomicUsize::new(DEFAULT_NODE_HISTORY_REVISIONS);

/// Install the resolved retention bound (the knob `graph.node_history_revisions`).
/// Called once at startup by the knob installer; a limit of zero keeps no history.
pub fn install_node_history_limit(revisions: usize) {
    NODE_HISTORY_LIMIT.store(revisions, Ordering::Release);
}

/// Why a read at a version could not be answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeHistoryError {
    /// The history no longer reaches back to the asked version.
    Retired { asked: u64, floor: u64 },
    /// The asked version is newer than the graph's current version.
    Future { asked: u64, current: u64 },
}

impl std::fmt::Display for NodeHistoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Retired { asked, floor } => write!(
                f,
                "graph version {asked} is older than the retained node history (floor {floor})"
            ),
            Self::Future { asked, current } => write!(
                f,
                "graph version {asked} is newer than the graph's current version {current}"
            ),
        }
    }
}

type Image = Option<Arc<Vec<u8>>>;

#[derive(Debug, Default)]
struct Inner {
    /// Prior images of writes not yet stamped with a version, oldest first.
    pending: Vec<(String, Image)>,
    /// Per node: `(stamp, prior image)` ascending by stamp. The prior image is the
    /// node's image through `stamp - 1`; `None` means the node did not exist.
    revisions: HashMap<String, Vec<(u64, Image)>>,
    /// Every stamped revision, oldest first, for bounded eviction.
    order: VecDeque<(u64, String)>,
    /// The lowest version a read may ask for.
    floor: u64,
}

/// The per-graph node history.
#[derive(Debug, Default)]
pub struct NodeHistory {
    inner: Mutex<Inner>,
}

impl NodeHistory {
    /// Record the image a write is about to replace or remove (`None` for a node
    /// that does not exist yet). Call before the write, under the topology guard.
    pub fn record_prior(&self, node_id: &str, prior: Option<Arc<Vec<u8>>>) {
        if NODE_HISTORY_LIMIT.load(Ordering::Acquire) == 0 {
            return;
        }
        self.inner.lock().pending.push((node_id.to_string(), prior));
    }

    /// Stamp every pending prior image with `version`, the version the writes became
    /// visible at, then enforce the retention bound.
    pub fn stamp(&self, version: u64) {
        let limit = NODE_HISTORY_LIMIT.load(Ordering::Acquire);
        let mut inner = self.inner.lock();
        let pending = std::mem::take(&mut inner.pending);
        let mut seen = HashSet::new();
        for (id, prior) in pending {
            // The first prior of a node within one version is its image at the
            // previous version; later ones are intermediate states of one write.
            if !seen.insert(id.clone()) {
                continue;
            }
            inner
                .revisions
                .entry(id.clone())
                .or_default()
                .push((version, prior));
            inner.order.push_back((version, id));
        }
        while inner.order.len() > limit {
            let Some((stamp, id)) = inner.order.pop_front() else {
                break;
            };
            if let Some(list) = inner.revisions.get_mut(&id) {
                list.remove(0);
                if list.is_empty() {
                    inner.revisions.remove(&id);
                }
            }
            inner.floor = inner.floor.max(stamp);
        }
    }

    /// Forget all history: the image was replaced wholesale at `version`.
    pub fn reset(&self, version: u64) {
        let mut inner = self.inner.lock();
        inner.pending.clear();
        inner.revisions.clear();
        inner.order.clear();
        inner.floor = inner.floor.max(version);
    }

    /// The image of `node_id` at `version`, falling back to `live` (the current
    /// image) when no replacement is stamped after `version`. The outer `Err` is a
    /// refusal; `Ok(None)` is a node that did not exist at `version`.
    pub fn image_at(
        &self,
        node_id: &str,
        version: u64,
        current: u64,
        live: impl FnOnce() -> Option<Arc<Vec<u8>>>,
    ) -> Result<Option<Arc<Vec<u8>>>, NodeHistoryError> {
        let inner = self.inner.lock();
        if version < inner.floor {
            return Err(NodeHistoryError::Retired {
                asked: version,
                floor: inner.floor,
            });
        }
        if version > current {
            return Err(NodeHistoryError::Future {
                asked: version,
                current,
            });
        }
        // The current version is the live image, writes not yet stamped included:
        // that is what a reader at the current version saw.
        if version == current {
            return Ok(live());
        }
        if let Some(list) = inner.revisions.get(node_id) {
            if let Some((_, prior)) = list.iter().find(|(stamp, _)| *stamp > version) {
                return Ok(prior.clone());
            }
        }
        // A write not yet stamped is newer than every stamped revision and than
        // every version below the current one.
        if let Some((_, prior)) = inner.pending.iter().find(|(id, _)| id == node_id) {
            return Ok(prior.clone());
        }
        Ok(live())
    }

    /// The lowest version [`Self::image_at`] answers.
    pub fn floor(&self) -> u64 {
        self.inner.lock().floor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blob(text: &str) -> Arc<Vec<u8>> {
        Arc::new(text.as_bytes().to_vec())
    }

    #[test]
    fn image_at_returns_the_prior_image_of_the_first_later_replacement() {
        let history = NodeHistory::default();
        history.record_prior("n", None);
        history.stamp(1);
        history.record_prior("n", Some(blob("a")));
        history.stamp(2);
        let live = || Some(blob("b"));
        assert_eq!(history.image_at("n", 0, 2, live).unwrap(), None);
        assert_eq!(history.image_at("n", 1, 2, live).unwrap(), Some(blob("a")));
        assert_eq!(history.image_at("n", 2, 2, live).unwrap(), Some(blob("b")));
    }

    #[test]
    fn an_unstamped_write_does_not_leak_into_the_previous_version() {
        let history = NodeHistory::default();
        history.record_prior("n", Some(blob("a")));
        let live = || Some(blob("b"));
        assert_eq!(history.image_at("n", 2, 3, live).unwrap(), Some(blob("a")));
        assert_eq!(history.image_at("n", 3, 3, live).unwrap(), Some(blob("b")));
    }

    #[test]
    fn a_graph_reads_a_node_as_of_each_version_across_replace_and_remove() {
        let core = crate::graph::GraphCore::new();
        let image = |n: u8| rmp_serde::to_vec_named(&serde_json::json!({ "n": n })).unwrap();
        core.add_node("x".to_string(), image(1));
        core.mark_dirty();
        let v1 = core.version();
        core.add_node("x".to_string(), image(2));
        core.mark_dirty();
        let v2 = core.version();
        core.remove_node("x".to_string());
        core.mark_dirty();
        let v3 = core.version();
        let at = |v: u64| {
            core.node_properties_at("x", v)
                .unwrap()
                .map(|blob| blob.as_ref().clone())
        };
        assert_eq!(at(0), None);
        assert_eq!(at(v1), Some(image(1)));
        assert_eq!(at(v2), Some(image(2)));
        assert_eq!(at(v3), None);
        assert_eq!(
            core.node_properties_at("x", v3 + 1),
            Err(NodeHistoryError::Future {
                asked: v3 + 1,
                current: v3
            })
        );
    }

    #[test]
    fn reads_outside_the_retained_range_refuse() {
        let history = NodeHistory::default();
        history.reset(5);
        assert_eq!(
            history.image_at("n", 4, 9, || None),
            Err(NodeHistoryError::Retired { asked: 4, floor: 5 })
        );
        assert_eq!(
            history.image_at("n", 10, 9, || None),
            Err(NodeHistoryError::Future {
                asked: 10,
                current: 9
            })
        );
    }
}
