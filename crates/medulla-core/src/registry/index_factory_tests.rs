//! Registry-installed secondary-index factories: a failed rebuild publishes `Failed`.

use medulla_graph_core::graph::GraphCore;
use medulla_graph_core::index::ChangeSet;
use medulla_graph_core::index::IndexDescriptor;
use medulla_graph_core::index::IndexError;
use medulla_graph_core::index::IndexKind;
use medulla_graph_core::index::IndexManifest;
use medulla_graph_core::index::IndexValidity;
use medulla_graph_core::index::SecondaryIndex;
use medulla_graph_core::index::SecondaryIndexFactory;

struct ManifestProbe {
    manifest: std::sync::Mutex<IndexManifest>,
    /// Both `apply_delta` and `full_rebuild` fail.
    fails: bool,
}

fn manifest_probe(manifest: IndexManifest, fails: bool) -> Box<ManifestProbe> {
    Box::new(ManifestProbe {
        manifest: std::sync::Mutex::new(manifest),
        fails,
    })
}

impl SecondaryIndex for ManifestProbe {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn descriptor(&self) -> IndexDescriptor {
        IndexDescriptor::discoverable_only(IndexKind::Text)
    }

    fn manifest(&self) -> IndexManifest {
        *self.manifest.lock().unwrap()
    }

    fn publish_manifest(&self, manifest: IndexManifest) {
        *self.manifest.lock().unwrap() = manifest;
    }

    fn maintains_manifest(&self) -> bool {
        true
    }

    fn apply_delta(&self, _core: &GraphCore, _change: &ChangeSet) -> Result<(), IndexError> {
        self.outcome()
    }

    fn full_rebuild(&self, _core: &GraphCore) -> Result<(), IndexError> {
        self.outcome()
    }
}

impl ManifestProbe {
    fn outcome(&self) -> Result<(), IndexError> {
        if self.fails {
            return Err(IndexError::Failed("probe".to_string()));
        }
        Ok(())
    }
}

/// Installing a secondary-index factory rebuilds each registered index before
/// publication; a failed rebuild is published `Failed` and incomplete.
#[test]
fn registry_factory_publishes_a_failed_rebuild_as_failed() {
    struct ProbeFactory(bool);
    impl SecondaryIndexFactory for ProbeFactory {
        fn for_graph(&self, _name: &str) -> Vec<Box<dyn SecondaryIndex>> {
            vec![manifest_probe(IndexManifest::default(), self.0)]
        }
    }
    let registered = |fails: bool| {
        let mut registry = crate::registry::GraphRegistry::new();
        registry.set_secondary_index_factory(std::sync::Arc::new(ProbeFactory(fails)));
        let core = &registry.get("__commons__").expect("commons exists").core;
        let manifests = core.indexes().server_manifests();
        assert_eq!(manifests.len(), 1);
        (core.version(), manifests[0].1)
    };
    let (version, valid) = registered(false);
    assert_eq!(valid, IndexManifest::valid(version, 0, 0));
    let (version, failed) = registered(true);
    assert_eq!(failed.validity, IndexValidity::Failed);
    assert!(!failed.completeness.complete);
    assert_eq!(failed.source_snapshot_version, version);
}
