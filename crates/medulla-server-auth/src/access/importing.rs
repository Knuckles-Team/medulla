//! Graphs a knowledge-bundle import is still writing (MEDULLA-DURABLE-KERNEL-R877.3).
//!
//! A graph an import creates is in the `importing` state until the import's single control-graph
//! write flips every graph of it to `ready`. Until then the graph is hidden from every read and
//! write: [`check_graph_access`](super::check_graph_access), the one chokepoint every graph
//! method and the graph listing pass through, refuses it, with the answer any other denied graph
//! gets. Only the work of the import itself, which runs inside [`within_import`], reaches it.
//!
//! The durable fact is the `BundleImport` control-graph node. This set is that node's in-process
//! face: the importer fills it before it creates a graph, and the recovery pass of the leader
//! tick fills it again for every `importing` node it finds, so a restart does not expose a
//! half-written graph.

use std::collections::BTreeSet;
use std::future::Future;
use std::sync::{LazyLock, Mutex};

static IMPORTING: LazyLock<Mutex<BTreeSet<String>>> = LazyLock::new(|| Mutex::new(BTreeSet::new()));
/// Import ids this process is running right now; the recovery pass leaves them to their importer.
static LIVE: LazyLock<Mutex<BTreeSet<String>>> = LazyLock::new(|| Mutex::new(BTreeSet::new()));

tokio::task_local! {
    static IMPORT_SCOPE: BTreeSet<String>;
}

fn lock<T>(cell: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    cell.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Hide `graphs` from every read until [`reveal`] names them.
pub fn hide<I, S>(graphs: I)
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    lock(&IMPORTING).extend(graphs.into_iter().map(Into::into));
}

/// Make `graphs` readable again.
pub fn reveal<'a, I: IntoIterator<Item = &'a String>>(graphs: I) {
    let mut set = lock(&IMPORTING);
    for graph in graphs {
        set.remove(graph);
    }
}

/// Record that this process is running import `import_id`.
pub fn mark_live(import_id: &str) {
    lock(&LIVE).insert(import_id.to_string());
}

/// Record that this process finished import `import_id`.
pub fn mark_done(import_id: &str) {
    lock(&LIVE).remove(import_id);
}

/// Whether this process is running import `import_id`.
pub fn is_live(import_id: &str) -> bool {
    lock(&LIVE).contains(import_id)
}

/// Whether `graph` is hidden from the work running now: it is `importing` and the work is not
/// inside [`within_import`] for it.
pub fn hidden_from_current(graph: &str) -> bool {
    if !lock(&IMPORTING).contains(graph) {
        return false;
    }
    !IMPORT_SCOPE
        .try_with(|scope| scope.contains(graph))
        .unwrap_or(false)
}

/// Run `work` with access to the `importing` graphs `graphs`.
pub async fn within_import<F: Future>(graphs: BTreeSet<String>, work: F) -> F::Output {
    IMPORT_SCOPE.scope(graphs, work).await
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DURABLE-KERNEL-R877.3
    #[tokio::test]
    async fn an_importing_graph_is_hidden_outside_its_import_and_visible_after_reveal() {
        let graph = "importing-hide-test".to_string();
        assert!(!hidden_from_current(&graph));
        hide([graph.clone()]);
        assert!(hidden_from_current(&graph));
        assert!(!within_import(BTreeSet::from([graph.clone()]), async { hidden_from_current(&graph) }).await);
        assert!(within_import(BTreeSet::from(["other".to_string()]), async { hidden_from_current(&graph) }).await);
        reveal([&graph]);
        assert!(!hidden_from_current(&graph));
        mark_live("id");
        assert!(is_live("id"));
        mark_done("id");
        assert!(!is_live("id"));
    }
}
