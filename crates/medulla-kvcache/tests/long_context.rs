//! Nightly: a 10 000 000-token context pages in coherent across every tier.

use std::collections::HashMap;

use medulla_kvcache::{content_hash, Block, KvBlockKey, TieredCache};

const TOKENS: usize = 10_000_000;
const TOKENS_PER_BLOCK: usize = 256;
/// Bytes standing in for one 256-token KV page.
const BLOCK_BYTES: usize = 512;

fn page(index: usize) -> Block {
    let mut bytes = Vec::with_capacity(BLOCK_BYTES);
    let mut state = (index as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    while bytes.len() < BLOCK_BYTES {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        bytes.extend_from_slice(&state.to_le_bytes());
    }
    bytes.truncate(BLOCK_BYTES);
    bytes
}

/// The cache key: model revision plus the token-prefix content hash, so a block is
/// never served under another model version or prefix.
fn key(model_version: &str, index: usize) -> String {
    let prefix = content_hash(&(index as u64).to_le_bytes());
    let k = KvBlockKey::new("model", model_version, prefix);
    format!("{}/{}/{}", k.model_id, k.model_version, k.token_prefix_hash)
}

/// Verifies MEDULLA-DURABLE-KERNEL-R083.7: Long-context coherence across tiers
// spec: MEDULLA-DURABLE-KERNEL-R083.7
#[test]
#[ignore = "nightly: 10M-token long-context coherence run"]
fn ten_million_token_context_pages_in_coherent() {
    let blocks = TOKENS.div_ceil(TOKENS_PER_BLOCK);
    let total = blocks * BLOCK_BYTES;
    // A 25 percent host budget across HOT + WARM; COLD holds the rest.
    let host = total / 4;
    let mut cache: TieredCache<String, Block> = TieredCache::new(host / 2, host / 2, total * 2);
    let mut first_write: HashMap<String, String> = HashMap::with_capacity(blocks);
    for index in 0..blocks {
        let bytes = page(index);
        let k = key("v1", index);
        first_write.insert(k.clone(), content_hash(&bytes));
        cache.try_put(k, bytes).expect("put");
    }
    let stats = cache.stats();
    assert!(stats.demotions_to_cold > 0, "the context spans every tier");
    assert_eq!(stats.drops, 0);

    let mut mismatches = 0usize;
    for index in 0..blocks {
        let k = key("v1", index);
        let bytes = cache.try_get(&k).expect("page-in").expect("resident");
        if content_hash(&bytes) != first_write[&k] {
            mismatches += 1;
        }
        assert_eq!(cache.try_get(&key("v2", index)).expect("read"), None);
    }
    assert_eq!(
        mismatches, 0,
        "every paged-in block matches its first-write digest"
    );
}
