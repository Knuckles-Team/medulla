//! Proofs of the clustering kernel through its public API (MEDULLA-CLUSTER).

use medulla_numeric::cluster::{
    fit, fit_simple, kmeans, nearest, plan, seed_centers, sq_dist, training_bytes, AssignBackend,
    CancelToken, CentroidTracker, ClusterConfig, ClusterRefusal, CpuBackend, End, FitArgs,
    NoProgress, Progress, ProgressSink, Rows,
};
use ndarray::array;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// `per` points around each of `centres`, Gaussian-ish noise of width `spread`.
fn blobs(centres: &[Vec<f32>], per: usize, spread: f32, seed: u64) -> (Vec<f32>, usize) {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let mut out = Vec::new();
    for i in 0..per * centres.len() {
        for &c in &centres[i % centres.len()] {
            out.push(c + (rng.gen::<f32>() - 0.5) * spread);
        }
    }
    (out, centres[0].len())
}

fn three_blobs() -> (Vec<f32>, usize, Vec<Vec<f32>>) {
    let centres = vec![vec![0.0f32, 0.0], vec![10.0, 10.0], vec![-10.0, 10.0]];
    let (data, dim) = blobs(&centres, 1000, 1.0, 9);
    (data, dim, centres)
}

// spec: MEDULLA-CLUSTER-R001.1

#[test]
fn rows_view_validates_shape() {
    let bad = fit_simple(
        Rows::new(&[1.0, 2.0, 3.0], 2, 2),
        1,
        &ClusterConfig::default(),
    );
    assert!(matches!(bad, Err(ClusterRefusal::ShapeInvalid(_))));
    let ok = Rows::new(&[1.0, 2.0, 3.0, 4.0], 2, 2);
    assert_eq!(ok.row(1), &[3.0, 4.0]);
}

// spec: MEDULLA-CLUSTER-R001.2

#[test]
fn every_refusal_has_a_distinct_code() {
    let all = [
        ClusterRefusal::KInvalid,
        ClusterRefusal::KExceedsRows { k: 2, n: 1 },
        ClusterRefusal::TooFewDistinct { k: 2 },
        ClusterRefusal::ShapeInvalid("x"),
        ClusterRefusal::InputNotFinite { row: 0, col: 0 },
        ClusterRefusal::MemoryBudget { need: 2, budget: 1 },
        ClusterRefusal::Cancelled,
        ClusterRefusal::StateIllegal(medulla_types::transition_refused::TransitionRefused {
            machine: "ClusterRunMachine".into(),
            from: "seeding".into(),
            event: "BatchDrawn".into(),
            reason: medulla_types::transition_refused::TransitionRefusalReason::IllegalTransition,
        }),
    ];
    let mut codes: Vec<_> = all.iter().map(ClusterRefusal::code).collect();
    codes.sort_unstable();
    codes.dedup();
    assert_eq!(codes.len(), 8);
    assert!(codes.iter().all(|c| c.starts_with("CLUSTERING_")));
}

// spec: MEDULLA-CLUSTER-R002
// spec: MEDULLA-CLUSTER-R002.3

#[test]
fn emeans_recovers_blobs_with_default_plan() {
    let (data, dim, centres) = three_blobs();
    let cfg = ClusterConfig {
        batch_rows: 500,
        ..ClusterConfig::default()
    };
    let out = fit_simple(Rows::new(&data, 3000, dim), 3, &cfg).unwrap();
    for c in &centres {
        let near = (0..3)
            .map(|i| sq_dist(c, &out.centroids[i * dim..(i + 1) * dim]))
            .fold(f32::INFINITY, f32::min);
        assert!(near < 0.2, "no centroid near {c:?}: {near}");
    }
    assert_eq!(out.labels.len(), 3000);
}

/// A brute-force Lloyd oracle with the kernel's seeds.
fn lloyd_oracle(data: &[f32], dim: usize, mut cents: Vec<f32>, k: usize, steps: usize) -> Vec<f32> {
    let n = data.len() / dim;
    for _ in 0..steps {
        let mut sums = vec![0.0f64; k * dim];
        let mut counts = vec![0u32; k];
        for i in 0..n {
            let c = nearest(&data[i * dim..(i + 1) * dim], &cents, dim, k);
            counts[c] += 1;
            for j in 0..dim {
                sums[c * dim + j] += f64::from(data[i * dim + j]);
            }
        }
        for c in 0..k {
            if counts[c] > 0 {
                for j in 0..dim {
                    cents[c * dim + j] = (sums[c * dim + j] / f64::from(counts[c])) as f32;
                }
            }
        }
    }
    cents
}

// spec: MEDULLA-CLUSTER-R002.4

#[test]
fn exact_mode_matches_brute_force_lloyd_oracle() {
    let centres = vec![vec![0.0f32, 0.0], vec![8.0, 8.0], vec![-8.0, 8.0]];
    let (data, dim) = blobs(&centres, 100, 2.0, 4);
    let cfg = ClusterConfig {
        exact: true,
        ..ClusterConfig::default()
    };
    let rows = Rows::new(&data, 300, dim);
    let out = fit_simple(rows, 3, &cfg).unwrap();
    assert_eq!(out.end, End::Converged);
    let seeds = seed_centers(rows, 3, cfg.seed, &cfg).unwrap();
    let oracle = lloyd_oracle(&data, dim, seeds, 3, 50);
    for (a, b) in out.centroids.iter().zip(&oracle) {
        assert!((a - b).abs() < 1e-4, "{a} vs {b}");
    }
}

// spec: MEDULLA-CLUSTER-R002.2

#[test]
fn seeding_is_deterministic_and_distinct() {
    let (data, dim, _) = three_blobs();
    let rows = Rows::new(&data, 3000, dim);
    let cfg = ClusterConfig::default();
    let a = seed_centers(rows, 8, 7, &cfg).unwrap();
    let b = seed_centers(rows, 8, 7, &cfg).unwrap();
    assert_eq!(a, b);
    let c = seed_centers(rows, 8, 8, &cfg).unwrap();
    assert_ne!(a, c);
    for i in 0..8 {
        for j in i + 1..8 {
            assert_ne!(a[i * dim..(i + 1) * dim], a[j * dim..(j + 1) * dim]);
        }
    }
    // Above the distance-sampling limit the seeds are k distinct rows.
    let small = ClusterConfig {
        plusplus_max_k: 2,
        ..ClusterConfig::default()
    };
    let s = seed_centers(rows, 16, 1, &small).unwrap();
    assert_eq!(s.len(), 16 * dim);
}

// spec: MEDULLA-CLUSTER-R003.2

#[test]
fn centroids_bit_identical_across_threads() {
    let (data, dim, _) = three_blobs();
    let rows = Rows::new(&data, 3000, dim);
    let run = |threads: usize| {
        let cfg = ClusterConfig {
            threads,
            batch_rows: 400,
            ..ClusterConfig::default()
        };
        fit_simple(rows, 12, &cfg).unwrap()
    };
    let one = run(1);
    for t in [2, 8] {
        let other = run(t);
        let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
        assert_eq!(bits(&one.centroids), bits(&other.centroids), "threads={t}");
        assert_eq!(one.labels, other.labels);
        assert_eq!(one.inertia.to_bits(), other.inertia.to_bits());
    }
}

// spec: MEDULLA-CLUSTER-R003.1

#[test]
fn tie_goes_to_lowest_index_and_fold_is_row_ordered() {
    // Two identical centroids: every row must take index 0.
    let data = [1.0f32, 1.0, 2.0, 2.0, 3.0, 3.0];
    let cfg = ClusterConfig {
        warm_start: Some(vec![2.0, 2.0, 2.0, 2.0]),
        exact: true,
        max_steps: 1,
        ..ClusterConfig::default()
    };
    let out = fit_simple(Rows::new(&data, 3, 2), 2, &cfg).unwrap();
    assert!(out.labels.iter().all(|&l| l == 0), "{:?}", out.labels);
}

struct Record(Mutex<Vec<(&'static str, u32)>>);
impl ProgressSink for Record {
    fn report(&self, p: &Progress) {
        if let Ok(mut v) = self.0.lock() {
            v.push((p.state, p.step));
        }
    }
}

// spec: MEDULLA-CLUSTER-R004.3

#[test]
fn progress_is_reported_per_state_change_and_throttled() {
    let (data, dim, _) = three_blobs();
    let sink = Record(Mutex::new(Vec::new()));
    let cfg = ClusterConfig {
        batch_rows: 300,
        ..ClusterConfig::default()
    };
    let cancel = CancelToken::new();
    let out = fit(FitArgs {
        rows: Rows::new(&data, 3000, dim),
        k: 3,
        cfg: &cfg,
        sink: &sink,
        cancel: &cancel,
    })
    .unwrap();
    let seen = sink.0.lock().unwrap().clone();
    assert_eq!(seen.first().map(|r| r.0), Some("Sampling"));
    let last = seen.last().unwrap();
    assert!(
        matches!(last.0, "Converged" | "BudgetExhausted"),
        "{last:?}"
    );
    assert_eq!(last.1, out.steps);
    // The steps ran in far less than one interval, so only the first and end reports show.
    assert!(seen.len() <= 4, "{seen:?}");
}

struct CancelOnFirst(CancelToken, AtomicUsize);
impl AssignBackend for CancelOnFirst {
    fn name(&self) -> &'static str {
        "cancel-on-first"
    }
    fn assign(
        &self,
        rows: Rows<'_>,
        idx: &[u32],
        c: &[f32],
        k: usize,
        l: &mut [u32],
        d: &mut [f32],
    ) {
        if self.1.fetch_add(1, Ordering::SeqCst) == 0 {
            self.0.cancel();
        }
        CpuBackend.assign(rows, idx, c, k, l, d);
    }
}

// spec: MEDULLA-CLUSTER-R004.4

#[test]
fn cancel_stops_at_next_boundary_without_centroids() {
    let (data, dim, _) = three_blobs();
    let cancel = CancelToken::new();
    medulla_numeric::cluster::set_assign_backend(Arc::new(CancelOnFirst(
        cancel.clone(),
        AtomicUsize::new(0),
    )));
    let cfg = ClusterConfig::default();
    let result = fit(FitArgs {
        rows: Rows::new(&data, 3000, dim),
        k: 3,
        cfg: &cfg,
        sink: &NoProgress,
        cancel: &cancel,
    });
    medulla_numeric::cluster::clear_assign_backend();
    assert_eq!(result.unwrap_err(), ClusterRefusal::Cancelled);
}

struct ThreadNames(Mutex<Vec<String>>);
impl AssignBackend for ThreadNames {
    fn name(&self) -> &'static str {
        "thread-names"
    }
    fn assign(
        &self,
        rows: Rows<'_>,
        idx: &[u32],
        c: &[f32],
        k: usize,
        l: &mut [u32],
        d: &mut [f32],
    ) {
        let name = std::thread::current().name().unwrap_or("").to_string();
        if let Ok(mut v) = self.0.lock() {
            v.push(name);
        }
        CpuBackend.assign(rows, idx, c, k, l, d);
    }
}

// spec: MEDULLA-CLUSTER-R011.3

#[test]
fn fit_uses_its_own_pool_not_the_global_one() {
    let (data, dim, _) = three_blobs();
    let names = Arc::new(ThreadNames(Mutex::new(Vec::new())));
    medulla_numeric::cluster::set_assign_backend(names.clone());
    fit_simple(Rows::new(&data, 3000, dim), 3, &ClusterConfig::default()).unwrap();
    medulla_numeric::cluster::clear_assign_backend();
    let seen = names.0.lock().unwrap().clone();
    assert!(!seen.is_empty());
    assert!(
        seen.iter().all(|n| n.starts_with("cluster-kernel-")),
        "{seen:?}"
    );
}

// spec: MEDULLA-CLUSTER-R008.3

#[test]
fn custom_backend_is_used_and_must_agree() {
    let (data, dim, _) = three_blobs();
    let rows = Rows::new(&data, 3000, dim);
    let cfg = ClusterConfig::default();
    let plain = fit_simple(rows, 3, &cfg).unwrap();
    medulla_numeric::cluster::set_assign_backend(Arc::new(ThreadNames(Mutex::new(Vec::new()))));
    let custom = fit_simple(rows, 3, &cfg).unwrap();
    medulla_numeric::cluster::clear_assign_backend();
    assert_eq!(plain.labels, custom.labels);
    assert_eq!(plain.centroids, custom.centroids);
}

// spec: MEDULLA-CLUSTER-R005.1

#[test]
fn bad_inputs_refuse_with_typed_codes() {
    let cfg = ClusterConfig::default();
    let r = |data: &[f32], n, d, k| fit_simple(Rows::new(data, n, d), k, &cfg).unwrap_err();
    assert_eq!(r(&[1.0, 2.0], 1, 2, 0), ClusterRefusal::KInvalid);
    assert_eq!(
        r(&[1.0, 2.0, 3.0, 4.0], 2, 2, 5),
        ClusterRefusal::KExceedsRows { k: 5, n: 2 }
    );
    assert!(matches!(r(&[], 0, 2, 1), ClusterRefusal::ShapeInvalid(_)));
    assert!(matches!(
        r(&[1.0], 1, 0, 1),
        ClusterRefusal::ShapeInvalid(_)
    ));
    let nan = [1.0, 2.0, 3.0, 4.0, 5.0, f32::NAN, 7.0, 8.0];
    assert_eq!(
        r(&nan, 4, 2, 2),
        ClusterRefusal::InputNotFinite { row: 2, col: 1 }
    );
    let inf = [1.0, f32::INFINITY];
    assert_eq!(
        r(&inf, 1, 2, 1),
        ClusterRefusal::InputNotFinite { row: 0, col: 1 }
    );
    // at_most_k clamps instead of refusing.
    let clamp = ClusterConfig {
        at_most_k: true,
        ..ClusterConfig::default()
    };
    let out = fit_simple(Rows::new(&[0.0, 5.0], 2, 1), 5, &clamp).unwrap();
    assert_eq!(out.centroids.len(), 2);
}

// spec: MEDULLA-CLUSTER-R005.4

#[test]
fn duplicate_rows_refuse_or_clamp() {
    let same = [1.0f32; 8];
    let cfg = ClusterConfig::default();
    let err = fit_simple(Rows::new(&same, 4, 2), 3, &cfg).unwrap_err();
    assert_eq!(err, ClusterRefusal::TooFewDistinct { k: 3 });
    let clamp = ClusterConfig {
        at_most_k: true,
        ..ClusterConfig::default()
    };
    let out = fit_simple(Rows::new(&same, 4, 2), 3, &clamp).unwrap();
    assert_eq!(out.centroids.len(), 2);
}

// spec: MEDULLA-CLUSTER-R005.2

#[test]
fn budget_exhaustion_is_a_named_end_state() {
    let mut rng = ChaCha8Rng::seed_from_u64(2);
    let data: Vec<f32> = (0..5000 * 4).map(|_| rng.gen::<f32>()).collect();
    let cfg = ClusterConfig {
        batch_rows: 500,
        max_steps: 5,
        min_steps: 5,
        tol: 1e-12,
        patience: 50,
        ..ClusterConfig::default()
    };
    let out = fit_simple(Rows::new(&data, 5000, 4), 8, &cfg).unwrap();
    assert_eq!(out.end, End::BudgetExhausted);
    assert_eq!(out.steps, 5);
}

// spec: MEDULLA-CLUSTER-R005
#[test]
fn no_input_spins() {
    // Every adversarial shape ends within the step budget.
    let cfg = ClusterConfig {
        max_steps: 7,
        batch_rows: 64,
        ..ClusterConfig::default()
    };
    let line: Vec<f32> = (0..2000).map(|i| (i % 3) as f32).collect();
    let out = fit_simple(Rows::new(&line, 2000, 1), 3, &cfg).unwrap();
    assert!(out.steps <= 7);
    let wide: Vec<f32> = (0..200 * 50).map(|i| ((i * 7919) % 101) as f32).collect();
    let out = fit_simple(Rows::new(&wide, 200, 50), 40, &cfg).unwrap();
    assert!(out.steps <= 7);
}

// spec: MEDULLA-CLUSTER-R005.3

#[test]
fn empty_cluster_reseeds_farthest_point_lowest_index_first() {
    let (data, dim, _) = three_blobs();
    let rows = Rows::new(&data, 3000, dim);
    let cfg = ClusterConfig {
        // The third centroid starts far from every point and receives none.
        warm_start: Some(vec![0.0, 0.0, 10.0, 10.0, 1000.0, 1000.0]),
        empty_patience: 1,
        batch_rows: 300,
        ..ClusterConfig::default()
    };
    let a = fit_simple(rows, 3, &cfg).unwrap();
    let b = fit_simple(rows, 3, &cfg).unwrap();
    assert_eq!(a.centroids, b.centroids);
    assert!(
        a.centroids[4].abs() < 100.0,
        "centroid never reseeded: {:?}",
        a.centroids
    );
}

// spec: MEDULLA-CLUSTER-R006.2

#[test]
fn over_budget_refuses_before_allocating() {
    let (data, dim, _) = three_blobs();
    let cfg = ClusterConfig {
        memory_budget_bytes: 1024,
        ..ClusterConfig::default()
    };
    let err = fit_simple(Rows::new(&data, 3000, dim), 3, &cfg).unwrap_err();
    assert!(
        matches!(err, ClusterRefusal::MemoryBudget { budget: 1024, .. }),
        "{err:?}"
    );
}

// spec: MEDULLA-CLUSTER-R006

#[test]
fn peak_memory_within_formula() {
    // The formula at the spec's example: 39.6 MiB plus 64 KiB.
    let need = training_bytes(65_536, 128, 4096);
    assert_eq!(
        need,
        4 * 65_536 * 128 + 120 * 65_536 + 32_768 + 16 * 4096 + 8 * 128
    );
    assert!(need < 40 * 1024 * 1024 + 64 * 1024);
    // The same formula governs a real run: a budget equal to it admits the run, one
    // byte less refuses it and names the need.
    let (data, dim, _) = three_blobs();
    let n = data.len() / dim;
    let base = ClusterConfig::default();
    let batch = plan(n, 3, dim, &base).batch;
    let need = training_bytes(3, dim, batch);
    let at = ClusterConfig {
        memory_budget_bytes: need,
        ..base.clone()
    };
    assert!(fit_simple(Rows::new(&data, n, dim), 3, &at).is_ok());
    let below = ClusterConfig {
        memory_budget_bytes: need - 1,
        ..base
    };
    let err = fit_simple(Rows::new(&data, n, dim), 3, &below).unwrap_err();
    assert_eq!(
        err,
        ClusterRefusal::MemoryBudget {
            need,
            budget: need - 1
        }
    );
}

// spec: MEDULLA-CLUSTER-R009.1

#[test]
fn tracker_follows_shifted_distribution() {
    let (data, dim, _) = three_blobs();
    let cfg = ClusterConfig::default();
    let out = fit_simple(Rows::new(&data, 3000, dim), 3, &cfg).unwrap();
    let mut t = CentroidTracker::new(out.centroids.clone(), dim, out.mean_sq_dist, 256);
    // Same distribution: no drift.
    for i in 0..1000 {
        t.observe(&data[(i % 3000) * dim..(i % 3000 + 1) * dim]);
    }
    assert!(!t.is_stale(1.5), "{:?}", t.drift_ratio());
    // Shifted distribution: drift passes the threshold.
    for i in 0..1000 {
        let row = &data[(i % 3000) * dim..(i % 3000 + 1) * dim];
        t.observe(&[row[0] + 4.0, row[1] + 4.0]);
    }
    assert!(t.is_stale(1.5), "{:?}", t.drift_ratio());
}

// spec: MEDULLA-CLUSTER-R009.2

#[test]
fn drift_ratio_crosses_threshold_only_after_warmup() {
    let mut t = CentroidTracker::new(vec![0.0, 0.0], 2, 1.0, 400);
    for _ in 0..99 {
        t.observe(&[10.0, 10.0]);
    }
    assert_eq!(t.drift_ratio(), None);
    t.observe(&[10.0, 10.0]);
    assert!(t.drift_ratio().is_some());
}

// spec: MEDULLA-CLUSTER-R008.5

#[test]
fn python_surface_args_map_to_config() {
    let data = array![[0.0, 0.0], [0.1, 0.0], [9.0, 9.0], [9.1, 9.0]];
    let res = kmeans(data.view(), 2, 100, 7).unwrap();
    assert_eq!(res.labels[0], res.labels[1]);
    assert_ne!(res.labels[0], res.labels[2]);
    assert!(res.n_iter <= 100);
    assert!(kmeans(data.view(), 0, 10, 1).is_err());
}

/// Repo root: two levels above this crate.
fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn rust_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

// spec: MEDULLA-CLUSTER-R007.1

#[test]
fn kernel_has_no_ffi_and_no_target_cpu_flags() {
    let root = repo_root();
    let mut scanned = 0;
    let config = std::fs::read_to_string(root.join(".cargo/config.toml")).unwrap_or_default();
    assert!(
        !config.contains("target-cpu"),
        ".cargo/config.toml pins a CPU"
    );
    assert!(
        !config.contains("target-feature"),
        ".cargo/config.toml pins a CPU feature"
    );
    let mut files = Vec::new();
    rust_files(&root.join("crates/medulla-numeric/src/cluster"), &mut files);
    for f in files {
        let src = std::fs::read_to_string(&f).unwrap();
        scanned += 1;
        for banned in ["extern \"C\"", "extern crate", "target_feature(enable"] {
            assert!(!src.contains(banned), "{} contains {banned}", f.display());
        }
    }
    assert!(scanned >= 8, "scanned only {scanned} kernel files");
    let manifest = std::fs::read_to_string(root.join("crates/medulla-numeric/Cargo.toml")).unwrap();
    for banned in ["faiss", "cblas", "openblas", "-sys"] {
        assert!(!manifest.contains(banned), "manifest names {banned}");
    }
}

// spec: MEDULLA-CLUSTER-R001
// spec: MEDULLA-CLUSTER-R001.4
// spec: MEDULLA-CLUSTER-R008
// spec: MEDULLA-CLUSTER-R008.8
#[test]
fn kernel_is_the_only_trainer() {
    let root = repo_root();
    let mut files = Vec::new();
    rust_files(&root.join("crates"), &mut files);
    rust_files(&root.join("src"), &mut files);
    let kernel = root.join("crates/medulla-numeric/src/cluster");
    let mut scanned = 0;
    let mut findings = Vec::new();
    for f in files {
        let norm = f.components().collect::<std::path::PathBuf>();
        let this_file = norm.ends_with("medulla-numeric/tests/cluster_kernel.rs");
        if norm.starts_with(&kernel) || this_file || norm.to_string_lossy().contains("/target") {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(&f) else {
            continue;
        };
        scanned += 1;
        for banned in [
            "fn lloyd_step",
            "fn update_centroids",
            "fn kmeans_plus_plus",
            "fn kmeanspp_init",
            "fn worst_fit_point",
            "fn assign_labels",
            "fn nearest_centroid",
            "fn batch_assign_dispatch",
        ] {
            if src.contains(banned) {
                findings.push(format!("{}: {banned}", f.display()));
            }
        }
        // No name of a removed copy survives, in code or in a doc comment, and no doc
        // comment names the removed accelerator module instead of the kernel backend.
        for leftover in [
            "lloyd_step",
            "kmeanspp_init",
            "kmeans_plus_plus",
            "update_centroids",
            "worst_fit_point",
            "kmeans_gpu",
        ] {
            if src.contains(leftover) {
                findings.push(format!("{}: names {leftover}", f.display()));
            }
        }
        // A function named `kmeans` outside the kernel may only delegate to it.
        if src.contains("fn kmeans(") && !src.contains("cluster::") {
            findings.push(format!("{}: fn kmeans without a kernel call", f.display()));
        }
    }
    assert!(scanned > 500, "scan universe too small: {scanned} files");
    assert!(findings.is_empty(), "second trainer found: {findings:#?}");
}
