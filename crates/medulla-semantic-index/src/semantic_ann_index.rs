//! Off-query-path IVF-PQ build and readiness lifecycle.

use super::*;

impl SemanticStore {
    /// Build the maintained IVF-PQ artifact off the query path. Wider generic
    /// vectors remain exact-only and intentionally do not allocate ANN state.
    pub fn warm(&self, label: &str) {
        if self.arena.len() < BRUTE_FORCE_THRESHOLD.max(ANN_BUILD_THRESHOLD)
            || self.arena.dim > MAX_MAINTAINED_DIMENSION
        {
            return;
        }
        self.ensure_index(label);
    }

    /// `pub(super)` — also called directly by `semantic_ann_persistence::export_generation`
    /// (a sibling submodule of the shared `backend` parent) to guarantee a fresh
    /// index is resident before it is persisted.
    pub(super) fn ensure_index(&self, label: &str) {
        if self.arena.dim > MAX_MAINTAINED_DIMENSION || self.index_ready() {
            return;
        }
        // `begin_build` claims the build from `STATE_COLD`: a caller that checked
        // `index_ready` before another caller's build finished cannot claim a
        // `STATE_READY` store, so exactly one concurrent warm builds.
        self.build_in_place(label);
    }

    fn index_ready(&self) -> bool {
        let idx = self.index.read();
        idx.is_some() && *self.built_len.read() == self.arena.len()
    }
}
