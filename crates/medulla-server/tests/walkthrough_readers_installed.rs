//! MEDULLA-ORCH-VOCAB-R013.7.5: the production walkthrough readers are installed at boot,
//! confirm a current revision and an indexed entity digest, and read another tenant's
//! repository and blob as absent.
#![cfg(feature = "blob")]

use std::sync::Arc;

use medulla_protocol_types::protocol::GraphType;
use medulla_server_api::state::ServerState;
use medulla_server_api::walkthrough_readers::{BlobReader, RepositoryIndexReader};
use medulla_server_blob::blob::BlobStateExt;
use medulla_types::contract::Digest256;
use tokio::sync::RwLock;

const TENANT_A: &str = "tenant-a";
const TENANT_B: &str = "tenant-b";
const REPOSITORY: &str = "repo-1";
const REVISION: &str = "1111111111111111111111111111111111111111";
const ENTITY: &str = "symbol:abc";
const SOURCE_TEXT: &[u8] = b"fn indexed() {}";

fn props(value: serde_json::Value) -> Vec<u8> {
    rmp_serde::to_vec_named(&value).expect("encode node properties")
}

// proves R013.7.5: Production walkthrough readers installed at boot
// spec: MEDULLA-ORCH-VOCAB-R013.7.5
#[tokio::test]
async fn installed_readers_confirm_a_current_revision_and_refuse_another_tenant() {
    let mut state = medulla_server::seams_install::new_state_for_test(
        "walkthrough-readers",
        ServerState::test_isolation("walkthrough-agent"),
    );
    let store: Arc<dyn medulla_server_blob::blob::store::ChunkStore> = Arc::new(
        medulla_server_blob::blob::store::RedbChunkStore::open_temp().expect("open blob store"),
    );
    state.set_blob_cursors(Some(Arc::new(medulla_server_blob::blob::BlobCursors::new(
        Arc::clone(&store),
    ))));
    let graph_a = medulla_identity_types::tenancy::tenant_graph(
        &medulla_identity_types::tenancy::TenantId::new(TENANT_A).expect("tenant id"),
    );
    state
        .registry
        .create_graph(&graph_a, GraphType::Team, None)
        .expect("create the tenant graph");
    let digest = Digest256::sha256(SOURCE_TEXT).to_hex();
    {
        let core = &state.registry.get(&graph_a).expect("tenant graph").core;
        core.add_node(
            format!("branch:{REPOSITORY}:main"),
            props(serde_json::json!({
                "type": "Branch", "repository_id": REPOSITORY, "ref_name": "main",
                "revision_id": REVISION, "status": "live",
            })),
        );
        core.add_node(
            ENTITY.to_string(),
            props(serde_json::json!({
                "type": "Symbol", "source_revision": REVISION, "source_digest": digest,
            })),
        );
    }
    let committed = medulla_server_blob::blob::stream::stream_blob_put_owned(
        store.as_ref(),
        &b"walkthrough artifact"[..],
        0,
        TENANT_A,
    )
    .expect("store the artifact");
    let artifact_ref = format!("cas:sha256:{}", committed.digest);

    let shared = Arc::new(RwLock::new(state));
    medulla_server::seams_install::install_seams_on(&shared).await;
    let readers = shared
        .read()
        .await
        .seams
        .walkthrough
        .get()
        .cloned()
        .expect("the walkthrough readers are installed at boot");
    let _ = readers;

    let index = medulla_server::walkthrough_readers::GraphIndexReader::new(Arc::clone(&shared));
    let blobs = medulla_server::walkthrough_readers::ServerBlobReader::new(Arc::clone(&shared));
    let (index, blobs) = tokio::task::spawn_blocking(move || {
        assert_eq!(
            index.current_revision(TENANT_A, REPOSITORY).as_deref(),
            Some(REVISION)
        );
        let good = Digest256::sha256(SOURCE_TEXT);
        assert!(index.snapshot_contains(TENANT_A, REPOSITORY, REVISION, ENTITY, REVISION, &good));
        let wrong = Digest256::sha256(b"other text");
        assert!(!index.snapshot_contains(TENANT_A, REPOSITORY, REVISION, ENTITY, REVISION, &wrong));
        assert_eq!(index.current_revision(TENANT_B, REPOSITORY), None);
        assert!(!index.snapshot_contains(TENANT_B, REPOSITORY, REVISION, ENTITY, REVISION, &good));
        assert_eq!(
            blobs.read(TENANT_A, &artifact_ref).as_deref(),
            Some(&b"walkthrough artifact"[..])
        );
        assert_eq!(blobs.read(TENANT_B, &artifact_ref), None);
        (index, blobs)
    })
    .await
    .expect("reader checks run");
    drop((index, blobs));
}
