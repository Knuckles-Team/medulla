//! IVF-PQ trained on the shared clustering kernel (MEDULLA-CLUSTER-R003.3, R008.1, R010).

use medulla_ann::ivfpq::{IvfPq, IvfPqParams, SearchParams};
use medulla_numeric::cluster::sq_dist;
use medulla_numeric::cluster::ClusterConfig;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

fn clustered(n: usize, dim: usize, groups: usize, seed: u64) -> Vec<Vec<f32>> {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let centres: Vec<Vec<f32>> = (0..groups)
        .map(|_| (0..dim).map(|_| rng.gen::<f32>() * 2.0 - 1.0).collect())
        .collect();
    (0..n)
        .map(|_| {
            let c = &centres[rng.gen_range(0..groups)];
            c.iter()
                .map(|&x| x + (rng.gen::<f32>() - 0.5) * 0.30)
                .collect()
        })
        .collect()
}

fn params(dim: usize, nlist: usize, opq_iters: usize, cluster: ClusterConfig) -> IvfPqParams {
    IvfPqParams {
        dim,
        nlist,
        m: dim / 4,
        cluster,
        opq_iters,
        seed: 7,
    }
}

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|x| x.to_bits()).collect()
}

// spec: MEDULLA-CLUSTER-R003.3
#[test]
fn codebooks_are_byte_identical_across_runs_and_threads() {
    let data = clustered(6000, 16, 24, 3);
    let train = |threads: usize| {
        let cluster = ClusterConfig {
            threads,
            batch_rows: 1500,
            ..ClusterConfig::default()
        };
        IvfPq::train(&params(16, 32, 3, cluster), &data)
    };
    let a = train(1);
    let b = train(1);
    let c = train(4);
    for other in [&b, &c] {
        assert_eq!(bits(&a.rotation), bits(&other.rotation));
        assert_eq!(bits(&a.coarse_centroids), bits(&other.coarse_centroids));
        assert_eq!(bits(&a.pq_centroids), bits(&other.pq_centroids));
    }
}

/// Brute-force ground truth and recall@10 with refinement.
fn recall(idx: &IvfPq, data: &[Vec<f32>], queries: &[usize]) -> f64 {
    let (mut hits, mut total) = (0usize, 0usize);
    for &qi in queries {
        let q = &data[qi];
        let mut truth: Vec<(u64, f32)> = data
            .iter()
            .enumerate()
            .map(|(i, v)| (i as u64, sq_dist(q, v)))
            .collect();
        truth.sort_by(|a, b| a.1.total_cmp(&b.1));
        let want: Vec<u64> = truth.iter().take(10).map(|t| t.0).collect();
        let sp = SearchParams {
            nprobe: 16,
            refine: true,
            refine_factor: 16,
        };
        let got: Vec<u64> = idx.search(q, 10, sp).into_iter().map(|r| r.id).collect();
        hits += want.iter().filter(|w| got.contains(w)).count();
        total += want.len();
    }
    hits as f64 / total as f64
}

/// Mean squared distance of the training set to its nearest coarse centroid.
fn coarse_inertia(idx: &IvfPq, data: &[Vec<f32>]) -> f64 {
    let total: f64 = data
        .iter()
        .map(|v| {
            let (_, d) = medulla_numeric::cluster::nearest_with_dist(
                v,
                &idx.coarse_centroids,
                idx.dim,
                idx.coarse_centroids.len() / idx.dim,
            );
            f64::from(d)
        })
        .sum();
    total / data.len() as f64
}

// spec: MEDULLA-CLUSTER-R008.1
// spec: MEDULLA-CLUSTER-R010.1
#[test]
fn recall_matches_exact_mode_within_budget() {
    let (n, dim) = (12_000usize, 32usize);
    let data = clustered(n, dim, 48, 11);
    let items: Vec<(u64, Vec<f32>)> = data
        .iter()
        .cloned()
        .enumerate()
        .map(|(i, v)| (i as u64, v))
        .collect();
    let queries: Vec<usize> = (0..120).map(|i| i * 97 % n).collect();
    let run = |exact: bool| {
        let cluster = ClusterConfig {
            exact,
            ..ClusterConfig::default()
        };
        let mut idx = IvfPq::train(&params(dim, 96, 0, cluster), &data);
        idx.add(&items);
        (recall(&idx, &data, &queries), coarse_inertia(&idx, &data))
    };
    let (exact_recall, exact_inertia) = run(true);
    let (emeans_recall, emeans_inertia) = run(false);
    assert!(
        emeans_recall >= exact_recall - 0.01,
        "recall {emeans_recall} vs exact {exact_recall}"
    );
    assert!(
        emeans_inertia <= exact_inertia * 1.03,
        "inertia {emeans_inertia} vs exact {exact_inertia}"
    );
}

/// The acceptance thresholds of MEDULLA-CLUSTER-R010.2, in one block. They are the starting
/// values of the row until `research.md` D7 records the measured ones.
mod acceptance {
    /// The test's size: the row's 100,000 vectors scaled to the 60 second budget of a test.
    pub const VECTORS: usize = 20_000;
    pub const DIM: usize = 128;
    pub const CELLS: usize = 1_024;
    pub const GROUPS: usize = 80;
    pub const QUERIES: usize = 60;
    /// e-means build time at most this times the exact-mode build time.
    pub const BUILD_TIME_RATIO_MAX: f64 = 0.33;
    /// recall@10 at least the exact mode's minus this (0.5 points).
    pub const RECALL_GAP_MAX: f64 = 0.005;
    /// Inertia at most this times the exact mode's.
    pub const INERTIA_RATIO_MAX: f64 = 1.03;
}

// spec: MEDULLA-CLUSTER-R010.2
#[test]
fn emeans_meets_the_acceptance_thresholds() {
    use acceptance::*;
    let data = clustered(VECTORS, DIM, GROUPS, 11);
    let items: Vec<(u64, Vec<f32>)> = data
        .iter()
        .cloned()
        .enumerate()
        .map(|(i, v)| (i as u64, v))
        .collect();
    let queries: Vec<usize> = (0..QUERIES).map(|i| i * 331 % VECTORS).collect();
    let run = |exact: bool| {
        let cluster = ClusterConfig {
            exact,
            ..ClusterConfig::default()
        };
        let started = std::time::Instant::now();
        let mut idx = IvfPq::train(&params(DIM, CELLS, 0, cluster), &data);
        let build = started.elapsed().as_secs_f64();
        idx.add(&items);
        (
            build,
            recall(&idx, &data, &queries),
            coarse_inertia(&idx, &data),
        )
    };
    let (exact_build, exact_recall, exact_inertia) = run(true);
    let (emeans_build, emeans_recall, emeans_inertia) = run(false);
    assert!(
        emeans_build <= exact_build * BUILD_TIME_RATIO_MAX,
        "build {emeans_build:.2}s is over {BUILD_TIME_RATIO_MAX} of exact {exact_build:.2}s"
    );
    assert!(
        emeans_recall >= exact_recall - RECALL_GAP_MAX,
        "recall {emeans_recall} vs exact {exact_recall}"
    );
    assert!(
        emeans_inertia <= exact_inertia * INERTIA_RATIO_MAX,
        "inertia {emeans_inertia} vs exact {exact_inertia}"
    );
}
