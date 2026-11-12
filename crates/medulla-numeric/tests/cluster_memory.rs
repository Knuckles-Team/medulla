//! Measured peak memory of one clustering run (MEDULLA-CLUSTER-R006.3).
//!
//! A counting global allocator wraps the system allocator. This file holds one test,
//! so no other test thread allocates while the run is measured.

use medulla_numeric::cluster::{fit_simple, training_bytes, ClusterConfig, Mode, Rows};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counting;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call forwards to `System` unchanged; the counters only observe sizes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        if !ptr.is_null() {
            let now = LIVE.fetch_add(layout.size(), Ordering::SeqCst) + layout.size();
            PEAK.fetch_max(now, Ordering::SeqCst);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
        LIVE.fetch_sub(layout.size(), Ordering::SeqCst);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let out = System.realloc(ptr, layout, new_size);
        if !out.is_null() {
            if new_size >= layout.size() {
                let now = LIVE.fetch_add(new_size - layout.size(), Ordering::SeqCst)
                    + (new_size - layout.size());
                PEAK.fetch_max(now, Ordering::SeqCst);
            } else {
                LIVE.fetch_sub(layout.size() - new_size, Ordering::SeqCst);
            }
        }
        out
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

// spec: MEDULLA-CLUSTER-R006.3

#[test]
fn peak_live_bytes_within_formula() {
    let (n, dim, k) = (20_000usize, 64usize, 8_192usize);
    let data: Vec<f32> = (0..n * dim)
        .map(|i| ((i * 2_654_435_761) % 1000) as f32 / 10.0)
        .collect();
    let cfg = ClusterConfig {
        threads: 2,
        labels: false,
        ..ClusterConfig::default()
    };
    // Warm the pool so its one-time allocations are outside the measurement.
    let warm = Rows::new(&data[..100 * dim], 100, dim);
    fit_simple(warm, 4, &cfg).unwrap();

    let plan = medulla_numeric::cluster::plan(n, k, dim, &cfg);
    assert_eq!(plan.mode, Mode::EMeans);
    let formula = training_bytes(k, dim, plan.batch) as usize;
    let before = LIVE.load(Ordering::SeqCst);
    PEAK.store(before, Ordering::SeqCst);
    let out = fit_simple(Rows::new(&data, n, dim), k, &cfg).unwrap();
    let peak = PEAK.load(Ordering::SeqCst) - before;
    assert_eq!(out.centroids.len(), k * dim);
    assert!(
        peak <= formula + 64 * 1024,
        "peak {peak} bytes above formula {formula} + 64 KiB"
    );
}
