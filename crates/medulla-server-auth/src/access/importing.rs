//! Graphs a knowledge-bundle import is still writing (MEDULLA-DURABLE-KERNEL-R877.3).
//!
//! A graph an import creates is `importing` until the import finishes: the durable registry row
//! of the graph names the import id, the registry entry carries it, and
//! [`check_graph_access`](super::check_graph_access), the one chokepoint every graph method and
//! the graph listing pass through, refuses the graph while the marker is set, with the answer any
//! other denied graph gets. The marker is the graph's own durable state, so another node and a
//! restarted process hide the graph without any in-process set.
//!
//! How the import's own writes pass (the one decision of this row): the check admits work that
//! runs under the same import id as the marker, never a caller identity. The importer and the
//! recovery pass wrap their graph work in [`within_import`]; no request path can set the id, so
//! no caller can name an import it did not start. The check stays one comparison in one place.
//!
//! Whether an import is still running is durable too: the import record in the control graph
//! carries a lease the importer renews, so no process-level set exists.

use std::future::Future;

tokio::task_local! {
    static IMPORT_ID: String;
}

/// Whether a graph whose registry entry carries `marker` is hidden from the work running now:
/// it names an import and the work is not running under that import's id.
pub fn hides(marker: Option<&str>) -> bool {
    let Some(marker) = marker else {
        return false;
    };
    !IMPORT_ID
        .try_with(|current| current == marker)
        .unwrap_or(false)
}

/// Run `work` as import `import_id`: the graphs that import marked reach it.
pub async fn within_import<F: Future>(import_id: String, work: F) -> F::Output {
    IMPORT_ID.scope(import_id, work).await
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DURABLE-KERNEL-R877.3
    #[tokio::test]
    async fn a_marked_graph_is_hidden_outside_its_own_import_only() {
        assert!(!hides(None));
        assert!(hides(Some("import-a")));
        assert!(!within_import("import-a".to_string(), async { hides(Some("import-a")) }).await);
        assert!(within_import("import-b".to_string(), async { hides(Some("import-a")) }).await);
    }
}
