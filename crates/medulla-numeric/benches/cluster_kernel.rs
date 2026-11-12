//! Clustering-kernel timing (MEDULLA-CLUSTER-R010.1): e-means against exact mode at
//! 1, 2 and 8 threads on one data set. Run:
//!
//!   cargo bench -p medulla-numeric --bench cluster_kernel -- 100000 128 1024
//!   #                                                        N      dim  k
//!
//! Prints `mode threads build_s inertia steps` lines.

use medulla_numeric::cluster::{fit_simple, ClusterConfig, Rows};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::time::Instant;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let n: usize = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(100_000);
    let dim: usize = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(128);
    let k: usize = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(1024);
    let mut rng = ChaCha8Rng::seed_from_u64(11);
    let groups = (n / 250).max(64);
    let centres: Vec<Vec<f32>> = (0..groups)
        .map(|_| (0..dim).map(|_| rng.gen::<f32>() * 2.0 - 1.0).collect())
        .collect();
    let mut data = Vec::with_capacity(n * dim);
    for _ in 0..n {
        let c = &centres[rng.gen_range(0..groups)];
        data.extend(c.iter().map(|x| x + (rng.gen::<f32>() - 0.5) * 0.30));
    }
    let rows = Rows::new(&data, n, dim);
    println!("cluster kernel bench: n={n} dim={dim} k={k}");
    for exact in [true, false] {
        for threads in [1usize, 2, 8] {
            let cfg = ClusterConfig {
                exact,
                threads,
                labels: false,
                ..ClusterConfig::default()
            };
            let t = Instant::now();
            let out = fit_simple(rows, k, &cfg).expect("fit");
            let mode = if exact { "exact" } else { "emeans" };
            println!(
                "{mode} threads={threads} build_s={:.3} mean_sq_dist={:.6} steps={}",
                t.elapsed().as_secs_f64(),
                out.mean_sq_dist,
                out.steps
            );
        }
    }
}
