//! Interned ids and capability postings (MEDULLA-DECISION-ENGINE-R172.2.1).
//!
//! Capability IRIs and policy tags intern to dense `u32` ids; each id owns a
//! sorted `Vec<u32>` of the record slots that carry it. A filter intersects the
//! lists by merge, after uniting each required capability with its subtypes.
//! No bitmap dependency: sorted vectors only.

use std::collections::{BTreeMap, BTreeSet};

/// Strings interned to dense `u32` ids.
#[derive(Debug, Clone, Default)]
pub struct Interner {
    ids: BTreeMap<String, u32>,
    names: Vec<String>,
}

impl Interner {
    /// The id of `name`, assigned on first sight.
    pub fn intern(&mut self, name: &str) -> u32 {
        if let Some(id) = self.ids.get(name) {
            return *id;
        }
        let id = self.names.len() as u32;
        self.ids.insert(name.to_string(), id);
        self.names.push(name.to_string());
        id
    }

    /// The id of `name`, if it was ever interned.
    pub fn get(&self, name: &str) -> Option<u32> {
        self.ids.get(name).copied()
    }

    /// The name of an interned id.
    pub fn name(&self, id: u32) -> Option<&str> {
        self.names.get(id as usize).map(String::as_str)
    }

    /// How many names are interned.
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Whether nothing is interned.
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

/// The sorted merge intersection of two sorted lists.
pub fn intersect(a: &[u32], b: &[u32]) -> Vec<u32> {
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::with_capacity(a.len().min(b.len()));
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                out.push(a[i]);
                i += 1;
                j += 1;
            }
        }
    }
    out
}

/// The sorted merge union of two sorted lists.
pub fn union(a: &[u32], b: &[u32]) -> Vec<u32> {
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::with_capacity(a.len() + b.len());
    while i < a.len() || j < b.len() {
        let next = match (a.get(i), b.get(j)) {
            (Some(x), Some(y)) if x == y => {
                i += 1;
                j += 1;
                *x
            }
            (Some(x), Some(y)) if x < y => {
                i += 1;
                *x
            }
            (Some(_), Some(y)) => {
                j += 1;
                *y
            }
            (Some(x), None) => {
                i += 1;
                *x
            }
            (None, Some(y)) => {
                j += 1;
                *y
            }
            (None, None) => break,
        };
        out.push(next);
    }
    out
}

/// Posting lists of one shard: capability and policy tag to sorted slots.
#[derive(Debug, Clone, Default)]
pub struct Postings {
    capabilities: Interner,
    tags: Interner,
    by_capability: Vec<Vec<u32>>,
    by_tag: Vec<Vec<u32>>,
}

impl Postings {
    /// Bytes the posting lists and interned names hold resident
    /// (MEDULLA-DECISION-ENGINE-R172.10).
    pub fn resident_bytes(&self) -> usize {
        let lists = |lists: &[Vec<u32>]| {
            lists
                .iter()
                .map(|list| list.capacity() * std::mem::size_of::<u32>())
                .sum::<usize>()
        };
        lists(&self.by_capability) + lists(&self.by_tag)
    }
}

fn insert_sorted(list: &mut Vec<u32>, slot: u32) {
    if let Err(at) = list.binary_search(&slot) {
        list.insert(at, slot);
    }
}

fn remove_sorted(list: &mut Vec<u32>, slot: u32) {
    if let Ok(at) = list.binary_search(&slot) {
        list.remove(at);
    }
}

impl Postings {
    /// Post `slot` under each capability and tag.
    pub fn add(&mut self, slot: u32, capabilities: &BTreeSet<String>, tags: &BTreeSet<String>) {
        for capability in capabilities {
            let id = self.capabilities.intern(capability) as usize;
            if self.by_capability.len() <= id {
                self.by_capability.resize_with(id + 1, Vec::new);
            }
            insert_sorted(&mut self.by_capability[id], slot);
        }
        for tag in tags {
            let id = self.tags.intern(tag) as usize;
            if self.by_tag.len() <= id {
                self.by_tag.resize_with(id + 1, Vec::new);
            }
            insert_sorted(&mut self.by_tag[id], slot);
        }
    }

    /// Remove `slot` from each capability and tag list it was posted under.
    pub fn remove(&mut self, slot: u32, capabilities: &BTreeSet<String>, tags: &BTreeSet<String>) {
        for capability in capabilities {
            if let Some(id) = self.capabilities.get(capability) {
                remove_sorted(&mut self.by_capability[id as usize], slot);
            }
        }
        for tag in tags {
            if let Some(id) = self.tags.get(tag) {
                remove_sorted(&mut self.by_tag[id as usize], slot);
            }
        }
    }

    fn capability_slots(&self, capability: &str) -> &[u32] {
        self.capabilities
            .get(capability)
            .map(|id| self.by_capability[id as usize].as_slice())
            .unwrap_or(&[])
    }

    /// The live slots that meet every required capability (directly or through
    /// a subtype listed in `subtypes`) and carry every required policy tag.
    /// `universe` is every live slot, sorted; it answers a filter that asks for
    /// nothing. A capability or tag nobody carries admits no slot.
    pub fn filtered_pool(
        &self,
        universe: &[u32],
        required: &BTreeSet<String>,
        tags: &BTreeSet<String>,
        subtypes: &BTreeMap<String, BTreeSet<String>>,
    ) -> Vec<u32> {
        let mut pool: Option<Vec<u32>> = None;
        let mut narrow = |pool: &mut Option<Vec<u32>>, list: Vec<u32>| {
            *pool = Some(match pool.take() {
                Some(current) => intersect(&current, &list),
                None => list,
            });
        };
        for capability in required {
            let mut united: Vec<u32> = self.capability_slots(capability).to_vec();
            for subtype in subtypes.get(capability).into_iter().flatten() {
                if subtype != capability {
                    united = union(&united, self.capability_slots(subtype));
                }
            }
            narrow(&mut pool, united);
        }
        for tag in tags {
            let list = self
                .tags
                .get(tag)
                .map(|id| self.by_tag[id as usize].clone())
                .unwrap_or_default();
            narrow(&mut pool, list);
        }
        pool.unwrap_or_else(|| universe.to_vec())
    }
}
