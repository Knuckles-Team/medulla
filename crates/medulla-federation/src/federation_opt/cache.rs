//! Bounded, owner-scoped foreign fragment cache (FO-11). Only a served registry may
//! supply a fresh connector checkpoint; inline and unwatermarked sources bypass it.

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use super::capability::RemoteRequest;
use super::stats::Fingerprint;
use medulla_exec::rowset::RowSet;

const MAX_ENTRIES: usize = 512;
const MAX_ROWS_PER_ENTRY: usize = 4096;

/// A named source's checkpoint, valid through the stated wall-clock millisecond.
#[derive(Clone, Debug)]
pub struct SourceWatermark {
    pub watermark: String,
    pub valid_through_ms: u64,
    /// The longest a cached entry of this source serves, in milliseconds. `Some` for a source
    /// with no checkpoint (a web metasearch source, MEDULLA-UNIFIED-DATA-PLANE-R038.7.2);
    /// `None` for a checkpointed source, whose entries live until the checkpoint advances.
    pub entry_ttl_ms: Option<u64>,
}

/// The verified registry owner and its named source checkpoints. The salt changes
/// across grants, revocations, re-registration and owner changes.
#[derive(Clone, Debug)]
pub struct FragmentCacheScope {
    owner_salt: String,
    sources: HashMap<String, SourceWatermark>,
}

impl FragmentCacheScope {
    pub fn new(owner_salt: String, sources: HashMap<String, SourceWatermark>) -> Self {
        Self {
            owner_salt,
            sources,
        }
    }

    fn key(&self, name: &str, fingerprint: &Fingerprint, request: &RemoteRequest) -> Option<Key> {
        let source = self.sources.get(name)?;
        if source.entry_ttl_ms.is_some() {
            // A TTL source is read only through `get_or_fetch_web`.
            return None;
        }
        if now_ms() > source.valid_through_ms {
            return None;
        }
        // Struct-field order and the caller's key order are preserved. A different
        // key order may change remote row order, so it is a distinct request.
        let request = rmp_serde::to_vec_named(request).ok()?;
        let mut digest = Sha256::new();
        for part in [
            self.owner_salt.as_bytes(),
            name.as_bytes(),
            fingerprint.as_slice(),
            source.watermark.as_bytes(),
            request.as_slice(),
        ] {
            digest.update((part.len() as u64).to_be_bytes());
            digest.update(part);
        }
        Some(Key(digest.finalize().into()))
    }

    /// The checkpoint a named source's copy is valid at, while it is fresh.
    pub(super) fn watermark_of(&self, name: &str) -> Option<String> {
        let source = self.sources.get(name)?;
        (now_ms() <= source.valid_through_ms).then(|| source.watermark.clone())
    }

    pub(super) fn get(
        &self,
        name: &str,
        fingerprint: &Fingerprint,
        request: &RemoteRequest,
    ) -> Option<RowSet> {
        let key = self.key(name, fingerprint, request)?;
        let mut cache = store().lock().ok()?;
        let entry = cache.entries.get(&key)?;
        if now_ms() > entry.valid_through_ms {
            cache.entries.remove(&key);
            cache.order.retain(|ordered| ordered != &key);
            return None;
        }
        Some(entry.rows.clone())
    }

    pub(super) fn insert(
        &self,
        name: &str,
        fingerprint: &Fingerprint,
        request: &RemoteRequest,
        rows: &RowSet,
    ) {
        if rows.len() > MAX_ROWS_PER_ENTRY {
            return;
        }
        let Some(key) = self.key(name, fingerprint, request) else {
            return;
        };
        let Some(valid_through_ms) = self.sources.get(name).map(|source| source.valid_through_ms)
        else {
            return;
        };
        let Ok(mut cache) = store().lock() else {
            return;
        };
        if !cache.entries.contains_key(&key) {
            while cache.entries.len() >= MAX_ENTRIES {
                let Some(oldest) = cache.order.pop_front() else {
                    break;
                };
                cache.entries.remove(&oldest);
            }
            cache.order.push_back(key);
        }
        cache.entries.insert(
            key,
            Entry {
                rows: rows.clone(),
                valid_through_ms,
                stored_at_ms: now_ms(),
            },
        );
    }
}

impl FragmentCacheScope {
    /// The entry TTL of a named web source, when the registry supplied one.
    pub(super) fn web_ttl_ms(&self, name: &str) -> Option<u64> {
        self.sources.get(name)?.entry_ttl_ms
    }

    /// The rows of one web query by its argument digest (MEDULLA-UNIFIED-DATA-PLANE-R038.7.2).
    /// An entry serves while `now_ms - stored_at_ms <= entry_ttl_ms`; an expired entry is
    /// removed and never serves. A miss runs `fetch` once and stores its rows. Returns the rows,
    /// the millisecond they were stored and whether they came from the cache. The key holds the
    /// owner salt, the source name, the source fingerprint and the digest, so two owners never
    /// share an entry.
    pub(super) fn get_or_fetch_web(
        &self,
        (name, fingerprint, digest): (&str, &Fingerprint, &str),
        now_ms: u64,
        fetch: impl FnOnce() -> Result<RowSet, String>,
    ) -> Result<(RowSet, u64, bool), String> {
        let Some(ttl) = self.web_ttl_ms(name) else {
            return fetch().map(|rows| (rows, now_ms, false));
        };
        let mut digest_key = Sha256::new();
        for part in [
            self.owner_salt.as_bytes(),
            name.as_bytes(),
            fingerprint.as_slice(),
            digest.as_bytes(),
        ] {
            digest_key.update((part.len() as u64).to_be_bytes());
            digest_key.update(part);
        }
        let key = Key(digest_key.finalize().into());
        if let Ok(mut cache) = store().lock() {
            if let Some(entry) = cache.entries.get(&key) {
                if now_ms.saturating_sub(entry.stored_at_ms) <= ttl {
                    return Ok((entry.rows.clone(), entry.stored_at_ms, true));
                }
                cache.entries.remove(&key);
                cache.order.retain(|ordered| ordered != &key);
            }
        }
        let rows = fetch()?;
        if rows.len() <= MAX_ROWS_PER_ENTRY {
            if let Ok(mut cache) = store().lock() {
                if !cache.entries.contains_key(&key) {
                    while cache.entries.len() >= MAX_ENTRIES {
                        let Some(oldest) = cache.order.pop_front() else {
                            break;
                        };
                        cache.entries.remove(&oldest);
                    }
                    cache.order.push_back(key);
                }
                cache.entries.insert(
                    key,
                    Entry {
                        rows: rows.clone(),
                        valid_through_ms: u64::MAX,
                        stored_at_ms: now_ms,
                    },
                );
            }
        }
        Ok((rows, now_ms, false))
    }
}

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
struct Key([u8; 32]);

struct Entry {
    rows: RowSet,
    valid_through_ms: u64,
    stored_at_ms: u64,
}

#[derive(Default)]
struct Store {
    entries: HashMap<Key, Entry>,
    order: VecDeque<Key>,
}

fn store() -> &'static Mutex<Store> {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(Store::default()))
}

pub(super) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(owner: &str, mark: &str, expires: u64) -> FragmentCacheScope {
        FragmentCacheScope::new(
            owner.into(),
            HashMap::from([(
                "crm".into(),
                SourceWatermark {
                    watermark: mark.into(),
                    valid_through_ms: expires,
                    entry_ttl_ms: None,
                },
            )]),
        )
    }

    // spec: MEDULLA-FEDERATED-QUERY-R049
    #[test]
    fn hit_miss_watermark_expiry_and_owner_are_isolated() {
        let fresh = scope("owner-a", "lsn-1", now_ms().saturating_add(30_000));
        let fp = [7; 32];
        let request = RemoteRequest::keys(vec!["k".into()]);
        let rows = RowSet::from_rows(vec![("k".into(), None)]);
        assert!(fresh.get("crm", &fp, &request).is_none());
        fresh.insert("crm", &fp, &request, &rows);
        assert_eq!(fresh.get("crm", &fp, &request), Some(rows));
        assert!(fresh.get("crm", &fp, &RemoteRequest::full()).is_none());
        assert!(scope("owner-b", "lsn-1", now_ms() + 30_000)
            .get("crm", &fp, &request)
            .is_none());
        assert!(scope("owner-a", "lsn-2", now_ms() + 30_000)
            .get("crm", &fp, &request)
            .is_none());
        assert!(scope("owner-a", "lsn-1", now_ms() - 1)
            .get("crm", &fp, &request)
            .is_none());
        assert!(fresh.get("unknown", &fp, &request).is_none());
    }
}
