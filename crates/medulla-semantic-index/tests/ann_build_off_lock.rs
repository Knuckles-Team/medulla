//! The index build holds no store lock (MEDULLA-CLUSTER-R011.1, R011.2, R011.4, R009.3).
#![cfg(feature = "ann")]

use medulla_semantic_index::semantic::{
    build_off_lock, BuildHooks, BuildOutcome, CancelToken, Progress, ProgressSink, SemanticStore,
};
use parking_lot::RwLock;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const DIM: usize = 16;

fn unit(rng: &mut ChaCha8Rng) -> Vec<f32> {
    (0..DIM).map(|_| rng.gen::<f32>() - 0.5).collect()
}

/// `n` vectors around 12 tight centres.
fn clustered_store(n: usize) -> SemanticStore {
    let mut rng = ChaCha8Rng::seed_from_u64(5);
    let centres: Vec<Vec<f32>> = (0..12).map(|_| unit(&mut rng)).collect();
    let mut store = SemanticStore::new();
    for i in 0..n {
        let c = &centres[i % 12];
        let v: Vec<f32> = c
            .iter()
            .map(|x| x + (rng.gen::<f32>() - 0.5) * 0.02)
            .collect();
        store.add_embedding(format!("n{i}"), v).unwrap();
    }
    store
}

/// Holds the training thread inside its first report until released.
struct Hold {
    reached: AtomicBool,
    release: AtomicBool,
}

impl ProgressSink for Hold {
    fn report(&self, _p: &Progress) {
        self.reached.store(true, Ordering::SeqCst);
        let began = Instant::now();
        while !self.release.load(Ordering::SeqCst) && began.elapsed() < Duration::from_secs(20) {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

fn wait_for(flag: &AtomicBool) {
    let began = Instant::now();
    while !flag.load(Ordering::SeqCst) {
        assert!(
            began.elapsed() < Duration::from_secs(60),
            "build never reached training"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

// spec: MEDULLA-CLUSTER-R011
// spec: MEDULLA-CLUSTER-R011.1
// spec: MEDULLA-CLUSTER-R011.2
#[test]
fn build_held_midway_does_not_block_writers_or_queries() {
    let lock = Arc::new(RwLock::new(clustered_store(5000)));
    let hold = Arc::new(Hold {
        reached: AtomicBool::new(false),
        release: AtomicBool::new(false),
    });
    let hooks = BuildHooks {
        sink: hold.clone(),
        ..BuildHooks::default()
    };
    let builder = {
        let lock = lock.clone();
        std::thread::spawn(move || build_off_lock(&lock, "g", false, &hooks))
    };
    wait_for(&hold.reached);

    // The build is held mid-training: a writer takes the lock at once.
    let began = Instant::now();
    lock.write()
        .add_embedding("late".into(), vec![0.25; DIM])
        .unwrap();
    assert!(
        began.elapsed() < Duration::from_millis(50),
        "writer waited {:?}",
        began.elapsed()
    );
    // A query answers (exact search) while the build holds.
    let began = Instant::now();
    let hits = lock.read().semantic_search(&[0.25; DIM], 3);
    assert!(began.elapsed() < Duration::from_secs(2));
    assert_eq!(hits.first().map(|h| h.0.as_str()), Some("late"));

    hold.release.store(true, Ordering::SeqCst);
    assert_eq!(builder.join().unwrap(), BuildOutcome::Built);
    // The write that landed during the build is in the installed index.
    let store = lock.read();
    assert!(store.is_ready());
    let hits = store.semantic_search(&[0.25; DIM], 3);
    assert_eq!(hits.first().map(|h| h.0.as_str()), Some("late"));
}

// spec: MEDULLA-CLUSTER-R011.4
#[test]
fn shutdown_cancels_running_build() {
    let lock = RwLock::new(clustered_store(5000));
    let cancel = CancelToken::new();
    struct CancelOnReport(CancelToken);
    impl ProgressSink for CancelOnReport {
        fn report(&self, _p: &Progress) {
            self.0.cancel();
        }
    }
    let hooks = BuildHooks {
        cancel: cancel.clone(),
        sink: Arc::new(CancelOnReport(cancel)),
        ..BuildHooks::default()
    };
    assert_eq!(
        build_off_lock(&lock, "g", false, &hooks),
        BuildOutcome::Cancelled
    );
    let store = lock.read();
    assert!(!store.is_ready() && !store.is_warming());
}

// spec: MEDULLA-CLUSTER-R009.3
#[test]
fn insert_after_shift_sets_stale_flag() {
    let lock = RwLock::new(clustered_store(5000));
    assert_eq!(
        build_off_lock(&lock, "g", false, &BuildHooks::default()),
        BuildOutcome::Built
    );
    assert!(!lock.read().ann_stale());
    // Same distribution: no drift.
    let mut rng = ChaCha8Rng::seed_from_u64(5);
    let centres: Vec<Vec<f32>> = (0..12).map(|_| unit(&mut rng)).collect();
    for i in 0..1500 {
        let v: Vec<f32> = centres[i % 12]
            .iter()
            .map(|x| x + (rng.gen::<f32>() - 0.5) * 0.02)
            .collect();
        lock.write().add_embedding(format!("same{i}"), v).unwrap();
    }
    assert!(!lock.read().ann_stale());
    // A shifted distribution: uniform random directions.
    let mut rng = ChaCha8Rng::seed_from_u64(99);
    for i in 0..1500 {
        lock.write()
            .add_embedding(format!("far{i}"), unit(&mut rng))
            .unwrap();
    }
    assert!(lock.read().ann_stale());
}

// spec: MEDULLA-CLUSTER-R009
// spec: MEDULLA-CLUSTER-R009.4
#[test]
fn shifted_ingest_triggers_one_background_rebuild() {
    let lock = Arc::new(RwLock::new(clustered_store(5000)));
    assert_eq!(
        build_off_lock(&lock, "g", false, &BuildHooks::default()),
        BuildOutcome::Built
    );
    let mut rng = ChaCha8Rng::seed_from_u64(99);
    for i in 0..1500 {
        lock.write()
            .add_embedding(format!("far{i}"), unit(&mut rng))
            .unwrap();
    }
    assert!(lock.read().ann_stale());

    let hold = Arc::new(Hold {
        reached: AtomicBool::new(false),
        release: AtomicBool::new(false),
    });
    let hooks = BuildHooks {
        sink: hold.clone(),
        ..BuildHooks::default()
    };
    let rebuild = {
        let lock = lock.clone();
        let hooks = hooks.clone();
        std::thread::spawn(move || build_off_lock(&lock, "g", true, &hooks))
    };
    wait_for(&hold.reached);
    // Serving continues on the old index while the rebuild trains; a second
    // trigger backs off at the claim.
    {
        let store = lock.read();
        assert!(store.is_rebuilding());
        assert_eq!(store.semantic_search(&[0.1; DIM], 3).len(), 3);
    }
    assert_eq!(
        build_off_lock(&lock, "g", true, &BuildHooks::default()),
        BuildOutcome::Skipped
    );
    hold.release.store(true, Ordering::SeqCst);
    assert_eq!(rebuild.join().unwrap(), BuildOutcome::Built);
    let store = lock.read();
    assert!(store.is_ready() && !store.ann_stale());
}
