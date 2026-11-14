//! A capacity replay row under encryption at rest: sealed once as a whole on
//! write, unsealed once on read, and its inner result decodes as plain bytes.

use medulla_kernel::crypto::ValueCipher;
use medulla_store::redb_store::shard::{Shard, ShardWrite};
use medulla_store::redb_store::DurableCrypto;

use super::{read_replay, write_replay, ReplayKey};

const GRAPH: &str = "graph-replay";

#[test]
fn a_sealed_replay_row_is_unsealed_once_and_its_result_decodes() {
    let path = medulla_store::redb_store::temp_path("medulla-capacity-replay", "sealed");
    let shard = Shard::open(&path).expect("temp shard opens");
    let cipher = ValueCipher::from_key_material(b"capacity-replay-test-key");
    let crypto = DurableCrypto::new(Some(&cipher));
    let members = shard.graph_members(&[GRAPH]).unwrap();
    let (group, batches) = shard.admit_maintenance(&members, "replay-test").unwrap();
    let write = ShardWrite::open(&shard, &group, &members, &batches).unwrap();
    let result = vec!["lease-1".to_string(), "lease-2".to_string()];
    write_replay(
        &write,
        GRAPH,
        ReplayKey {
            tenant: "tenant-a",
            key: "key-1",
            operation: "acquire",
            digest: "digest-1",
        },
        &result,
        crypto,
    )
    .unwrap();

    let replay = read_replay(&write, GRAPH, "tenant-a", "key-1", crypto)
        .unwrap()
        .expect("the replay row is stored");
    assert_eq!(replay.operation, "acquire");
    assert_eq!(replay.decode_result::<Vec<String>>().unwrap(), result);
    // The inner result was never sealed on its own: unsealing it again fails
    // closed, which is the double unseal a replay read must not do.
    assert!(crypto.unseal(&replay.result).is_err());
    drop(write);
    let _ = std::fs::remove_file(&path);
}
