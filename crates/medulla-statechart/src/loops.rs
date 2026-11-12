//! Loop kernels (MEDULLA-DECISION-ENGINE-R653.2, R653.7, R653.8).
//!
//! Pure functions over the `medulla_types::loops` records: the fenced lease,
//! the loop queue order, gap settlement, the offer rate, the failure-sequence
//! miner and the capability ratchet. Time arrives as an argument and nothing
//! here reads a store or a clock; the `Loops` handler in `medulla-h-decide`
//! is the single writer of loop state and calls these kernels.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap};

use medulla_types::loops::{
    Gap, GapState, ImprovementSample, LoopLease, MinedPattern, RatchetEvidence, RatchetVerdict,
    PPM_MAX,
};

/// The loop knobs, each inside its declared range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoopKnobs {
    /// `loops.lease_ttl`, seconds, `[5, 3600]`.
    pub lease_ttl_s: u64,
    /// `loops.offer_rate_cap_ppm`, `[0, 1000000]`.
    pub offer_rate_cap_ppm: u32,
    /// `loops.offer_capacity`, `[1, 256]`.
    pub offer_capacity: u32,
    /// `loops.miner_max_len`, `[2, 8]`.
    pub miner_max_len: u32,
    /// `loops.miner_min_support`, `[2, 1000]`.
    pub miner_min_support: u32,
    /// `loops.miner_max_patterns`, `[1, 1024]`.
    pub miner_max_patterns: u32,
    /// `loops.ratchet_min_samples`, `[1, 1000]`.
    pub ratchet_min_samples: u32,
    /// `loops.ratchet_min_gain_micro`, `[-1000000, 1000000]`.
    pub ratchet_min_gain_micro: i64,
}

impl Default for LoopKnobs {
    fn default() -> Self {
        Self {
            lease_ttl_s: 60,
            offer_rate_cap_ppm: PPM_MAX,
            offer_capacity: 8,
            miner_max_len: 4,
            miner_min_support: 3,
            miner_max_patterns: 64,
            ratchet_min_samples: 3,
            ratchet_min_gain_micro: 0,
        }
    }
}

/// Why a kernel refused. Each variant maps to one stable refusal code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoopError {
    /// A claim on a live lease.
    LeaseHeld { holder: String, epoch: u64 },
    /// A call whose epoch differs from the stored one.
    LeaseStale { stored: Option<u64>, given: u64 },
    /// A settle on a gap that is already settled.
    GapAlreadySettled { gap_id: String },
    /// A settle on a gap that holds no offer.
    GapNotOffered { gap_id: String },
    /// A ratchet side with too few samples.
    RatchetNoBaseline {
        side: &'static str,
        have: u32,
        need: u32,
    },
}

impl LoopError {
    /// The stable refusal code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::LeaseHeld { .. } => "LOOP_LEASE_HELD",
            Self::LeaseStale { .. } => "LOOP_LEASE_STALE",
            Self::GapAlreadySettled { .. } => "GAP_ALREADY_SETTLED",
            Self::GapNotOffered { .. } => "GAP_NOT_OFFERED",
            Self::RatchetNoBaseline { .. } => "RATCHET_NO_BASELINE",
        }
    }
}

impl std::fmt::Display for LoopError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: ", self.code())?;
        match self {
            Self::LeaseHeld { holder, epoch } => {
                write!(f, "held by {holder} at epoch {epoch}")
            }
            Self::LeaseStale { stored, given } => {
                write!(f, "epoch {given} against stored {stored:?}")
            }
            Self::GapAlreadySettled { gap_id } => write!(f, "gap {gap_id} is settled"),
            Self::GapNotOffered { gap_id } => write!(f, "gap {gap_id} holds no offer"),
            Self::RatchetNoBaseline { side, have, need } => {
                write!(f, "{side} side has {have} samples, needs {need}")
            }
        }
    }
}

impl std::error::Error for LoopError {}

/// Take the lease: succeeds when `lease` is `None` or expired at `now_ms`.
/// The new lease carries `previous epoch + 1` and expires `ttl_s` later.
pub fn claim_loop(
    lease: Option<&LoopLease>,
    holder: &str,
    now_ms: u64,
    ttl_s: u64,
) -> Result<LoopLease, LoopError> {
    if let Some(held) = lease {
        if held.expires_at_ms > now_ms {
            return Err(LoopError::LeaseHeld {
                holder: held.holder.clone(),
                epoch: held.epoch,
            });
        }
    }
    Ok(LoopLease {
        holder: holder.to_string(),
        epoch: lease.map_or(0, |held| held.epoch) + 1,
        expires_at_ms: now_ms.saturating_add(ttl_s.saturating_mul(1000)),
    })
}

/// Reject any call whose epoch differs from the stored epoch.
pub fn check_epoch(lease: Option<&LoopLease>, epoch: u64) -> Result<(), LoopError> {
    match lease {
        Some(held) if held.epoch == epoch => Ok(()),
        other => Err(LoopError::LeaseStale {
            stored: other.map(|held| held.epoch),
            given: epoch,
        }),
    }
}

/// One queued loop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueEntry {
    pub loop_id: String,
    pub priority: u32,
    pub created_at_ms: u64,
}

impl Ord for QueueEntry {
    /// Greater runs first: priority descending, then `created_at_ms`
    /// ascending, then `loop_id` ascending.
    fn cmp(&self, other: &Self) -> Ordering {
        self.priority
            .cmp(&other.priority)
            .then_with(|| other.created_at_ms.cmp(&self.created_at_ms))
            .then_with(|| other.loop_id.cmp(&self.loop_id))
    }
}

impl PartialOrd for QueueEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// The loop queue: a binary heap in the total order of [`QueueEntry`].
#[derive(Clone, Debug, Default)]
pub struct LoopQueue {
    heap: BinaryHeap<QueueEntry>,
}

impl LoopQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, entry: QueueEntry) {
        self.heap.push(entry);
    }

    /// The loop that runs next.
    pub fn pop(&mut self) -> Option<QueueEntry> {
        self.heap.pop()
    }

    pub fn len(&self) -> usize {
        self.heap.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }

    /// Every entry in run order.
    pub fn into_order(self) -> Vec<QueueEntry> {
        self.heap.into_sorted_vec().into_iter().rev().collect()
    }
}

/// Compare-and-set `offered` to `settled`; a second call fails.
pub fn settle_gap(gap: &mut Gap) -> Result<(), LoopError> {
    match gap.state {
        GapState::Offered => {
            gap.state = GapState::Settled;
            Ok(())
        }
        GapState::Settled => Err(LoopError::GapAlreadySettled {
            gap_id: gap.gap_id.clone(),
        }),
        GapState::Open => Err(LoopError::GapNotOffered {
            gap_id: gap.gap_id.clone(),
        }),
    }
}

/// `saturation_ppm = min(open_offers * 1000000 / offer_capacity, 1000000)`.
pub fn saturation_ppm(open_offers: u32, offer_capacity: u32) -> u32 {
    let ppm = u64::from(open_offers) * u64::from(PPM_MAX) / u64::from(offer_capacity.max(1));
    ppm.min(u64::from(PPM_MAX)) as u32
}

/// `severity_ppm * (1000000 - saturation_ppm) / 1000000`, capped at
/// `offer_rate_cap_ppm`. Integer arithmetic only.
pub fn offer_rate(severity_ppm: u32, open_offers: u32, knobs: &LoopKnobs) -> u32 {
    let saturation = u64::from(saturation_ppm(open_offers, knobs.offer_capacity));
    let rate = u64::from(severity_ppm.min(PPM_MAX)) * (u64::from(PPM_MAX) - saturation)
        / u64::from(PPM_MAX);
    rate.min(u64::from(knobs.offer_rate_cap_ppm)) as u32
}

/// Count contiguous subsequences of length 2 to `miner_max_len` over the
/// failure sequences. A pattern counts once per sequence. Patterns with
/// support at or above `miner_min_support` are kept, sorted by support
/// descending then lexicographically, truncated to `miner_max_patterns`.
pub fn mine_patterns(sequences: &[Vec<String>], knobs: &LoopKnobs) -> Vec<MinedPattern> {
    let mut support: BTreeMap<&[String], u32> = BTreeMap::new();
    for sequence in sequences {
        let mut seen: std::collections::BTreeSet<&[String]> = std::collections::BTreeSet::new();
        for len in 2..=(knobs.miner_max_len as usize).min(sequence.len()) {
            for window in sequence.windows(len) {
                if seen.insert(window) {
                    *support.entry(window).or_insert(0) += 1;
                }
            }
        }
    }
    let mut kept: Vec<MinedPattern> = support
        .into_iter()
        .filter(|(_, count)| *count >= knobs.miner_min_support)
        .map(|(sequence, count)| MinedPattern {
            sequence: sequence.to_vec(),
            support: count,
        })
        .collect();
    kept.sort_by(|a, b| {
        b.support
            .cmp(&a.support)
            .then_with(|| a.sequence.cmp(&b.sequence))
    });
    kept.truncate(knobs.miner_max_patterns as usize);
    kept
}

fn mean_micro(values: &[i64]) -> i64 {
    let sum: i128 = values.iter().map(|v| i128::from(*v)).sum();
    (sum / values.len() as i128) as i64
}

/// Compare the mean ledger value of `metric` before and after `change_at_ms`
/// (samples strictly before it, and at or after it). Each side needs
/// `ratchet_min_samples`; the verdict is `Keep` when after minus before is at
/// least `ratchet_min_gain_micro`, else `Revert`.
pub fn ratchet_verdict(
    samples: &[ImprovementSample],
    metric: &str,
    change_at_ms: u64,
    knobs: &LoopKnobs,
) -> Result<RatchetEvidence, LoopError> {
    let (mut before, mut after) = (Vec::new(), Vec::new());
    for sample in samples.iter().filter(|s| s.metric == metric) {
        if sample.at_ms < change_at_ms {
            before.push(sample.value_micro);
        } else {
            after.push(sample.value_micro);
        }
    }
    for (side, values) in [("before", &before), ("after", &after)] {
        if (values.len() as u64) < u64::from(knobs.ratchet_min_samples) {
            return Err(LoopError::RatchetNoBaseline {
                side,
                have: values.len() as u32,
                need: knobs.ratchet_min_samples,
            });
        }
    }
    let (before_mean, after_mean) = (mean_micro(&before), mean_micro(&after));
    let gain = i128::from(after_mean) - i128::from(before_mean);
    let verdict = if gain >= i128::from(knobs.ratchet_min_gain_micro) {
        RatchetVerdict::Keep
    } else {
        RatchetVerdict::Revert
    };
    Ok(RatchetEvidence {
        metric: metric.to_string(),
        change_at_ms,
        before_mean_micro: before_mean,
        before_samples: before.len() as u32,
        after_mean_micro: after_mean,
        after_samples: after.len() as u32,
        verdict,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn entries() -> Vec<QueueEntry> {
        (0..64u64)
            .map(|n| QueueEntry {
                loop_id: format!("loop-{:02}", (n * 7) % 64),
                priority: (n % 4) as u32,
                created_at_ms: n % 5,
            })
            .collect()
    }

    // spec: MEDULLA-DECISION-ENGINE-R653.2
    #[test]
    fn lease_epochs_fence_and_queue_order_is_total() {
        let first = claim_loop(None, "a", 1_000, 60).unwrap();
        assert_eq!((first.epoch, first.expires_at_ms), (1, 61_000));
        // A live lease refuses, even to its own holder.
        assert_eq!(
            claim_loop(Some(&first), "a", 60_999, 60)
                .unwrap_err()
                .code(),
            "LOOP_LEASE_HELD"
        );
        // Expiry opens it; the epoch rises by one.
        let second = claim_loop(Some(&first), "b", 61_000, 5).unwrap();
        assert_eq!((second.epoch, second.holder.as_str()), (2, "b"));
        assert_eq!(second.expires_at_ms, 66_000);

        assert!(check_epoch(Some(&second), 2).is_ok());
        for stale in [1, 3] {
            assert_eq!(
                check_epoch(Some(&second), stale).unwrap_err().code(),
                "LOOP_LEASE_STALE"
            );
        }
        assert_eq!(check_epoch(None, 1).unwrap_err().code(), "LOOP_LEASE_STALE");

        // Priority descending, creation ascending, id ascending.
        let mut queue = LoopQueue::new();
        for (id, priority, created) in [("b", 1, 5), ("a", 1, 5), ("c", 9, 99), ("d", 1, 2)] {
            queue.push(QueueEntry {
                loop_id: id.into(),
                priority,
                created_at_ms: created,
            });
        }
        let order: Vec<String> = queue.into_order().into_iter().map(|e| e.loop_id).collect();
        assert_eq!(order, ["c", "d", "a", "b"]);

        // The order is the same pushed from 1 thread or 8.
        let mut single = LoopQueue::new();
        entries().into_iter().for_each(|e| single.push(e));
        let single = single.into_order();
        let shared = Arc::new(Mutex::new(LoopQueue::new()));
        let all = entries();
        let handles: Vec<_> = all
            .chunks(8)
            .map(|chunk| {
                let (shared, chunk) = (Arc::clone(&shared), chunk.to_vec());
                std::thread::spawn(move || {
                    for entry in chunk {
                        shared.lock().unwrap().push(entry);
                    }
                })
            })
            .collect();
        handles.into_iter().for_each(|h| h.join().unwrap());
        let threaded = Arc::try_unwrap(shared)
            .unwrap()
            .into_inner()
            .unwrap()
            .into_order();
        assert_eq!(single, threaded);
        assert_eq!(single.len(), 64);
    }

    fn gap(state: GapState) -> Gap {
        Gap {
            gap_id: "g".into(),
            loop_id: "l".into(),
            severity_ppm: 800_000,
            evidence: vec![],
            state,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R653.7
    #[test]
    fn settle_once_and_offer_rate_formula() {
        let mut g = gap(GapState::Offered);
        settle_gap(&mut g).unwrap();
        assert_eq!(g.state, GapState::Settled);
        assert_eq!(
            settle_gap(&mut g).unwrap_err().code(),
            "GAP_ALREADY_SETTLED"
        );
        assert_eq!(
            settle_gap(&mut gap(GapState::Open)).unwrap_err().code(),
            "GAP_NOT_OFFERED"
        );

        let knobs = LoopKnobs::default();
        assert_eq!(saturation_ppm(0, 8), 0);
        assert_eq!(saturation_ppm(2, 8), 250_000);
        assert_eq!(saturation_ppm(80, 8), PPM_MAX, "capped");
        assert_eq!(offer_rate(800_000, 0, &knobs), 800_000);
        assert_eq!(offer_rate(800_000, 2, &knobs), 600_000);
        assert_eq!(offer_rate(800_000, 8, &knobs), 0);
        let capped = LoopKnobs {
            offer_rate_cap_ppm: 500_000,
            ..knobs
        };
        assert_eq!(offer_rate(800_000, 0, &capped), 500_000);
        let wide = LoopKnobs {
            offer_capacity: 256,
            ..knobs
        };
        assert_eq!(offer_rate(1_000_000, 1, &wide), 996_094);
    }

    fn seq(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn sample(at_ms: u64, value_micro: i64) -> ImprovementSample {
        ImprovementSample {
            at_ms,
            metric: "pass".into(),
            value_micro,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R653.8
    #[test]
    fn miner_bounds_and_ratchet_verdict() {
        let sequences = vec![
            seq(&["a", "b", "c", "a", "b"]),
            seq(&["a", "b", "c"]),
            seq(&["x", "a", "b", "c", "d"]),
        ];
        let knobs = LoopKnobs::default();
        let mined = mine_patterns(&sequences, &knobs);
        // ab is in all three (once per sequence even when repeated), bc and
        // abc in all three; nothing else reaches support 3.
        let found: Vec<(Vec<String>, u32)> = mined
            .iter()
            .map(|p| (p.sequence.clone(), p.support))
            .collect();
        assert_eq!(
            found,
            [
                (seq(&["a", "b"]), 3),
                (seq(&["a", "b", "c"]), 3),
                (seq(&["b", "c"]), 3)
            ]
        );
        // Length bound: max_len 2 drops the 3-step pattern.
        let short = LoopKnobs {
            miner_max_len: 2,
            ..knobs
        };
        assert_eq!(mine_patterns(&sequences, &short).len(), 2);
        // Support floor and truncation.
        let loose = LoopKnobs {
            miner_min_support: 2,
            ..knobs
        };
        assert!(mine_patterns(&sequences, &loose).len() > 3);
        let one = LoopKnobs {
            miner_max_patterns: 1,
            ..knobs
        };
        assert_eq!(mine_patterns(&sequences, &one).len(), 1);
        assert!(mine_patterns(&[], &knobs).is_empty());

        // Ratchet: before = 100, 200, 300 (mean 200); after = 400, 500, 600.
        let ledger = vec![
            sample(1, 100),
            sample(2, 200),
            sample(3, 300),
            sample(10, 400),
            sample(11, 500),
            sample(12, 600),
        ];
        let kept = ratchet_verdict(&ledger, "pass", 10, &knobs).unwrap();
        assert_eq!(kept.verdict, RatchetVerdict::Keep);
        assert_eq!((kept.before_mean_micro, kept.before_samples), (200, 3));
        assert_eq!((kept.after_mean_micro, kept.after_samples), (500, 3));
        // A gain below the floor reverts; equality keeps.
        let strict = LoopKnobs {
            ratchet_min_gain_micro: 301,
            ..knobs
        };
        assert_eq!(
            ratchet_verdict(&ledger, "pass", 10, &strict)
                .unwrap()
                .verdict,
            RatchetVerdict::Revert
        );
        let exact = LoopKnobs {
            ratchet_min_gain_micro: 300,
            ..knobs
        };
        assert_eq!(
            ratchet_verdict(&ledger, "pass", 10, &exact)
                .unwrap()
                .verdict,
            RatchetVerdict::Keep
        );
        // A regression reverts at the default floor of zero.
        let worse = vec![
            sample(1, 500),
            sample(2, 500),
            sample(3, 500),
            sample(10, 100),
            sample(11, 100),
            sample(12, 100),
        ];
        assert_eq!(
            ratchet_verdict(&worse, "pass", 10, &knobs).unwrap().verdict,
            RatchetVerdict::Revert
        );
        // Too few samples on a side, or another metric, has no baseline.
        let err = ratchet_verdict(&ledger, "pass", 12, &knobs).unwrap_err();
        assert_eq!(err.code(), "RATCHET_NO_BASELINE");
        assert_eq!(
            ratchet_verdict(&ledger, "other", 10, &knobs)
                .unwrap_err()
                .code(),
            "RATCHET_NO_BASELINE"
        );
    }
}
