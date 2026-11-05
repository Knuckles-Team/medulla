//! Tier ladder and tier refusals over the byte-budgeted `TieredCache`.

use std::cell::Cell;
use std::collections::HashMap;
use std::io;
use std::rc::Rc;

use medulla_kvcache::{Block, ColdStore, KvTierError, Tier, TieredCache};

fn block(index: u32, len: usize) -> Block {
    (0..len)
        .map(|offset| (index as usize * 31 + offset * 7) as u8)
        .collect()
}

/// A cold device under test control: it can refuse writes, and it can corrupt the
/// bytes it hands back on a page-in.
struct ControlledCold {
    blobs: HashMap<String, Vec<u8>>,
    refuse_writes: Rc<Cell<bool>>,
    corrupt_reads: Rc<Cell<bool>>,
}

impl ColdStore<String> for ControlledCold {
    fn put(&mut self, key: &String, bytes: &[u8]) -> io::Result<()> {
        if self.refuse_writes.get() {
            return Err(io::Error::other("cold device refuses writes"));
        }
        self.blobs.insert(key.clone(), bytes.to_vec());
        Ok(())
    }

    fn get(&self, key: &String) -> io::Result<Option<Vec<u8>>> {
        let mut blob = self.blobs.get(key).cloned();
        if self.corrupt_reads.get() {
            if let Some(bytes) = blob.as_mut().and_then(|b| b.last_mut()) {
                *bytes ^= 0xff;
            }
        }
        Ok(blob)
    }

    fn remove(&mut self, key: &String) -> io::Result<()> {
        self.blobs.remove(key);
        Ok(())
    }

    fn kind(&self) -> &'static str {
        "controlled"
    }
}

fn controlled_cache(
    hot: usize,
    warm: usize,
    cold: usize,
) -> (TieredCache<String, Block>, Rc<Cell<bool>>, Rc<Cell<bool>>) {
    let refuse = Rc::new(Cell::new(false));
    let corrupt = Rc::new(Cell::new(false));
    let store = ControlledCold {
        blobs: HashMap::new(),
        refuse_writes: refuse.clone(),
        corrupt_reads: corrupt.clone(),
    };
    (
        TieredCache::with_cold_store(hot, warm, cold, Box::new(store)),
        refuse,
        corrupt,
    )
}

// spec: MEDULLA-DURABLE-KERNEL-R083.5
#[test]
fn demote_then_promote_keeps_bytes() {
    // Budgets small enough that the working set fills HOT, then WARM, then lands in
    // COLD; COLD holds everything, so a demotion never drops a block.
    let mut cache: TieredCache<String, Block> = TieredCache::new(2_048, 2_048, 1 << 20);
    let blocks: Vec<(String, Block)> = (0..64).map(|i| (format!("b{i}"), block(i, 256))).collect();
    for (key, bytes) in &blocks {
        cache.try_put(key.clone(), bytes.clone()).expect("put");
    }
    let stats = cache.stats();
    assert!(stats.demotions_to_warm > 0, "HOT pressure demotes one rung");
    assert!(
        stats.demotions_to_cold > 0,
        "WARM pressure demotes one rung"
    );
    assert_eq!(stats.drops, 0, "a demotion moves bytes, never drops them");
    assert!(stats.hot_bytes <= 2_048 && stats.warm_bytes <= 2_048);
    assert!(blocks
        .iter()
        .any(|(k, _)| cache.tier_of(k) == Some(Tier::Cold)));

    for (key, bytes) in &blocks {
        assert_eq!(cache.try_get(key).expect("get").as_ref(), Some(bytes));
        assert_eq!(cache.tier_of(key), Some(Tier::Hot), "a hit promotes to HOT");
    }
    assert_eq!(cache.stats().drops, 0);
}

// spec: MEDULLA-DURABLE-KERNEL-R083.8
#[test]
fn cold_write_failure_and_digest_mismatch_refuse() {
    // A failed durable cold write keeps the block in WARM and refuses the put.
    let (mut cache, refuse, corrupt) = controlled_cache(256, 256, 1 << 20);
    cache.try_put("a".into(), block(1, 200)).expect("fits HOT");
    cache
        .try_put("b".into(), block(2, 200))
        .expect("a demotes to WARM");
    assert_eq!(cache.tier_of(&"a".to_string()), Some(Tier::Warm));
    refuse.set(true);
    let refusal = cache.try_put("c".into(), block(3, 200)).unwrap_err();
    assert_eq!(refusal, KvTierError::ColdWriteFailed);
    assert_eq!(refusal.code(), "KV_COLD_WRITE_FAILED");
    assert_eq!(refusal.http_status(), 507);
    let stats = cache.stats();
    assert_eq!(stats.drops, 0, "the block is kept, not dropped");
    assert_eq!(stats.demotions_to_cold, 0);
    assert_eq!(cache.len(), 3, "every block still resident");
    assert_ne!(cache.tier_of(&"a".to_string()), Some(Tier::Cold));
    for (key, seed) in [("a", 1), ("b", 2), ("c", 3)] {
        assert_eq!(
            cache.try_get(&key.to_string()).unwrap(),
            Some(block(seed, 200))
        );
    }

    // A page-in whose digest differs from the digest at first write drops the block.
    refuse.set(false);
    for i in 10..20u32 {
        cache.try_put(format!("k{i}"), block(i, 200)).expect("put");
    }
    let cold_key = (10..20u32)
        .map(|i| format!("k{i}"))
        .find(|k| cache.tier_of(k) == Some(Tier::Cold))
        .expect("a block reached COLD");
    corrupt.set(true);
    let before = cache.len();
    let refusal = cache.try_get(&cold_key).unwrap_err();
    assert_eq!(refusal, KvTierError::BlockDigestMismatch);
    assert_eq!(refusal.code(), "KV_BLOCK_DIGEST_MISMATCH");
    assert_eq!(refusal.http_status(), 422);
    assert_eq!(
        cache.tier_of(&cold_key),
        None,
        "the mismatched block is dropped"
    );
    assert_eq!(
        cache.len(),
        before - 1,
        "no other block changed tier membership"
    );
    corrupt.set(false);
    assert_eq!(cache.try_get(&cold_key).unwrap(), None);
}
