//! Spectral and checked min-cut partitions (MEDULLA-FEDERATED-QUERY-R607.2).
//!
//! [`spectral_partition`] embeds every node with the `k` smallest eigenvectors of
//! the normalized Laplacian `L = I - D^-1/2 A D^-1/2` (equivalently the `k`
//! largest of `D^-1/2 A D^-1/2`), found by Lanczos with full reorthogonalization
//! over at most [`LANCZOS_ITERATIONS`] steps, then clusters the row-normalized
//! embedding with seeded k-means. Partition ids are the rank of each
//! partition's smallest member id, so they are stable across runs.
//! [`checked_mincut`] wraps [`stoer_wagner`] with the same input checks.

#![allow(clippy::needless_range_loop)] // dense numeric kernels index several arrays at once

use super::{stoer_wagner, AdjacencyGraph, MincutPartition};
use std::hash::Hash;

/// Lanczos steps used by [`spectral_partition`].
pub const LANCZOS_ITERATIONS: usize = 100;
/// Smallest graph a topology analytic accepts.
pub const TOPOLOGY_MIN_NODES: usize = 3;
const KMEANS_MAX_ITERATIONS: usize = 100;
const EPS: f64 = 1e-10;

/// Kernel refusals shared by the topology analytics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopologyKernelError {
    /// Fewer than [`TOPOLOGY_MIN_NODES`] nodes (`TOPOLOGY_TRIVIAL_GRAPH`).
    TrivialGraph,
    /// A non-finite or non-positive edge weight (`TOPOLOGY_PARAM_INVALID`).
    InvalidWeight,
    /// A parameter out of range (`TOPOLOGY_PARAM_INVALID`).
    InvalidParam(&'static str),
}

/// Result of [`spectral_partition`].
#[derive(Debug, Clone, PartialEq)]
pub struct SpectralPartition<N> {
    /// Partitions ordered by smallest member; members sorted. Index = id.
    pub partitions: Vec<Vec<N>>,
    /// Modularity of the returned partition over the undirected graph.
    pub modularity: f64,
    /// Lanczos steps actually taken.
    pub iterations: u32,
}

/// Refuse trivial graphs and non-finite / non-positive weights.
pub fn check_topology_graph<N>(graph: &AdjacencyGraph<N>) -> Result<(), TopologyKernelError>
where
    N: Clone + Eq + Hash + Ord,
{
    if graph.node_count() < TOPOLOGY_MIN_NODES {
        return Err(TopologyKernelError::TrivialGraph);
    }
    for u in 0..graph.node_count() {
        if graph
            .out_edges(u)
            .iter()
            .any(|&(_, w)| !w.is_finite() || w <= 0.0)
        {
            return Err(TopologyKernelError::InvalidWeight);
        }
    }
    Ok(())
}

/// The global minimum cut with the topology input checks applied.
pub fn checked_mincut<N>(
    graph: &AdjacencyGraph<N>,
) -> Result<MincutPartition<N>, TopologyKernelError>
where
    N: Clone + Eq + Hash + Ord,
{
    check_topology_graph(graph)?;
    stoer_wagner(graph).ok_or(TopologyKernelError::InvalidWeight)
}

/// Seeded xorshift64* generator; bit-reproducible for a fixed seed.
#[derive(Debug, Clone)]
pub(crate) struct SeededRng(u64);

impl SeededRng {
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed ^ 0x9E37_79B9_7F4A_7C15 | 1)
    }
    pub(crate) fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub(crate) fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Spectral partition into at most `k` parts (`k` in `[2, 256]`).
pub fn spectral_partition<N>(
    graph: &AdjacencyGraph<N>,
    k: usize,
    seed: u64,
) -> Result<SpectralPartition<N>, TopologyKernelError>
where
    N: Clone + Eq + Hash + Ord,
{
    if !(2..=256).contains(&k) {
        return Err(TopologyKernelError::InvalidParam("k"));
    }
    check_topology_graph(graph)?;
    let n = graph.node_count();
    let adjacency = graph.undirected_weighted_adjacency();
    let inv_sqrt_degree: Vec<f64> = adjacency
        .iter()
        .map(|edges| {
            let d: f64 = edges.iter().map(|&(_, w)| w).sum();
            if d > 0.0 {
                1.0 / d.sqrt()
            } else {
                0.0
            }
        })
        .collect();
    let matvec = |x: &[f64], out: &mut [f64]| {
        for (u, edges) in adjacency.iter().enumerate() {
            let mut acc = 0.0;
            for &(v, w) in edges {
                acc += w * inv_sqrt_degree[v] * x[v];
            }
            out[u] = acc * inv_sqrt_degree[u];
        }
    };
    let mut rng = SeededRng::new(seed);
    let (basis, values, vectors) = lanczos(n, LANCZOS_ITERATIONS.min(n), &matvec, &mut rng);
    let r = values.len();
    let mut order: Vec<usize> = (0..r).collect();
    order.sort_by(|&a, &b| values[b].total_cmp(&values[a]).then(a.cmp(&b)));
    let dims = k.min(r);
    let mut embedding = vec![vec![0.0; dims]; n];
    for (d, &col) in order.iter().take(dims).enumerate() {
        for (j, q) in basis.iter().enumerate() {
            let y = vectors[j][col];
            for i in 0..n {
                embedding[i][d] += q[i] * y;
            }
        }
    }
    for row in &mut embedding {
        let norm = row.iter().map(|x| x * x).sum::<f64>().sqrt();
        if norm > EPS {
            row.iter_mut().for_each(|x| *x /= norm);
        }
    }
    let assignment = kmeans(&embedding, k.min(n), &mut rng);
    let mut groups: Vec<Vec<usize>> = vec![Vec::new(); k.min(n)];
    for (i, &c) in assignment.iter().enumerate() {
        groups[c].push(i);
    }
    groups.retain(|g| !g.is_empty());
    groups.sort_by_key(|g| g[0]);
    let modularity = modularity(&adjacency, &groups, n);
    Ok(SpectralPartition {
        partitions: groups
            .iter()
            .map(|g| g.iter().map(|&i| graph.node_at(i).clone()).collect())
            .collect(),
        modularity,
        iterations: basis.len() as u32,
    })
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn orthogonalize(w: &mut [f64], basis: &[Vec<f64>]) {
    for _ in 0..2 {
        for q in basis {
            let c = dot(w, q);
            w.iter_mut().zip(q).for_each(|(x, y)| *x -= c * y);
        }
    }
}

fn random_unit(n: usize, basis: &[Vec<f64>], rng: &mut SeededRng) -> Option<Vec<f64>> {
    let mut v: Vec<f64> = (0..n).map(|_| rng.next_f64() - 0.5).collect();
    orthogonalize(&mut v, basis);
    let norm = dot(&v, &v).sqrt();
    (norm > EPS).then(|| v.into_iter().map(|x| x / norm).collect())
}

/// Lanczos with full reorthogonalization. Returns the basis, the tridiagonal
/// eigenvalues and its eigenvectors (`vectors[row][col]`).
#[allow(clippy::type_complexity)]
fn lanczos(
    n: usize,
    steps: usize,
    matvec: &dyn Fn(&[f64], &mut [f64]),
    rng: &mut SeededRng,
) -> (Vec<Vec<f64>>, Vec<f64>, Vec<Vec<f64>>) {
    let mut basis: Vec<Vec<f64>> = Vec::with_capacity(steps);
    let mut alpha = Vec::with_capacity(steps);
    let mut beta: Vec<f64> = Vec::with_capacity(steps);
    let Some(first) = random_unit(n, &basis, rng) else {
        return (basis, Vec::new(), Vec::new());
    };
    basis.push(first);
    let mut w = vec![0.0; n];
    loop {
        let j = basis.len() - 1;
        matvec(&basis[j], &mut w);
        let a = dot(&w, &basis[j]);
        alpha.push(a);
        orthogonalize(&mut w, &basis);
        if basis.len() == steps {
            break;
        }
        let b = dot(&w, &w).sqrt();
        let next = if b > EPS {
            beta.push(b);
            w.iter().map(|x| x / b).collect()
        } else {
            match random_unit(n, &basis, rng) {
                Some(v) => {
                    beta.push(0.0);
                    v
                }
                None => break,
            }
        };
        basis.push(next);
    }
    let r = alpha.len();
    let mut t = vec![vec![0.0; r]; r];
    for i in 0..r {
        t[i][i] = alpha[i];
        if i + 1 < r {
            t[i][i + 1] = beta[i];
            t[i + 1][i] = beta[i];
        }
    }
    let (values, vectors) = jacobi_eigen(t);
    (basis, values, vectors)
}

/// Cyclic Jacobi eigen-decomposition of a small symmetric matrix.
fn jacobi_eigen(mut a: Vec<Vec<f64>>) -> (Vec<f64>, Vec<Vec<f64>>) {
    let r = a.len();
    let mut v = vec![vec![0.0; r]; r];
    for (i, row) in v.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    for _ in 0..100 {
        let off: f64 = (0..r)
            .flat_map(|i| (0..r).filter(move |&j| j != i).map(move |j| (i, j)))
            .map(|(i, j)| a[i][j] * a[i][j])
            .sum();
        if off < 1e-22 {
            break;
        }
        for p in 0..r {
            for q in p + 1..r {
                if a[p][q].abs() < 1e-300 {
                    continue;
                }
                let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let t = if theta == 0.0 { 1.0 } else { t };
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for k in 0..r {
                    let akp = a[k][p];
                    let akq = a[k][q];
                    a[k][p] = c * akp - s * akq;
                    a[k][q] = s * akp + c * akq;
                }
                for k in 0..r {
                    let apk = a[p][k];
                    let aqk = a[q][k];
                    a[p][k] = c * apk - s * aqk;
                    a[q][k] = s * apk + c * aqk;
                }
                for row in v.iter_mut() {
                    let vkp = row[p];
                    let vkq = row[q];
                    row[p] = c * vkp - s * vkq;
                    row[q] = s * vkp + c * vkq;
                }
            }
        }
    }
    ((0..r).map(|i| a[i][i]).collect(), v)
}

fn sq_dist(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum()
}

/// Seeded k-means with k-means++ initialization; ties go to the lowest centroid.
fn kmeans(points: &[Vec<f64>], k: usize, rng: &mut SeededRng) -> Vec<usize> {
    let n = points.len();
    let mut centroids: Vec<Vec<f64>> = vec![points[(rng.next_u64() % n as u64) as usize].clone()];
    let mut nearest: Vec<f64> = points.iter().map(|p| sq_dist(p, &centroids[0])).collect();
    while centroids.len() < k {
        let total: f64 = nearest.iter().sum();
        let pick = if total <= EPS {
            centroids.len() % n
        } else {
            let mut target = rng.next_f64() * total;
            let mut chosen = n - 1;
            for (i, &d) in nearest.iter().enumerate() {
                if target < d {
                    chosen = i;
                    break;
                }
                target -= d;
            }
            chosen
        };
        centroids.push(points[pick].clone());
        let c = centroids.last().unwrap();
        for (i, p) in points.iter().enumerate() {
            nearest[i] = nearest[i].min(sq_dist(p, c));
        }
    }
    let mut assignment = vec![0usize; n];
    for _ in 0..KMEANS_MAX_ITERATIONS {
        let mut changed = false;
        for (i, p) in points.iter().enumerate() {
            let best = (0..k)
                .min_by(|&a, &b| {
                    sq_dist(p, &centroids[a])
                        .total_cmp(&sq_dist(p, &centroids[b]))
                        .then(a.cmp(&b))
                })
                .unwrap_or(0);
            if assignment[i] != best {
                assignment[i] = best;
                changed = true;
            }
        }
        let dims = points[0].len();
        let mut sums = vec![vec![0.0; dims]; k];
        let mut counts = vec![0usize; k];
        for (i, p) in points.iter().enumerate() {
            counts[assignment[i]] += 1;
            sums[assignment[i]]
                .iter_mut()
                .zip(p)
                .for_each(|(s, x)| *s += x);
        }
        for c in 0..k {
            if counts[c] > 0 {
                centroids[c] = sums[c].iter().map(|s| s / counts[c] as f64).collect();
            }
        }
        if !changed {
            break;
        }
    }
    assignment
}

/// Newman modularity of `groups` over an undirected weighted adjacency.
pub(crate) fn modularity(adjacency: &[Vec<(usize, f64)>], groups: &[Vec<usize>], n: usize) -> f64 {
    let mut community = vec![0usize; n];
    for (c, g) in groups.iter().enumerate() {
        for &i in g {
            community[i] = c;
        }
    }
    let two_m: f64 = adjacency.iter().flatten().map(|&(_, w)| w).sum();
    if two_m <= 0.0 {
        return 0.0;
    }
    let mut internal = vec![0.0; groups.len()];
    let mut total = vec![0.0; groups.len()];
    for (u, edges) in adjacency.iter().enumerate() {
        for &(v, w) in edges {
            total[community[u]] += w;
            if community[u] == community[v] {
                internal[community[u]] += w;
            }
        }
    }
    internal
        .iter()
        .zip(&total)
        .map(|(i, t)| i / two_m - (t / two_m).powi(2))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn planted() -> AdjacencyGraph<String> {
        let mut edges = Vec::new();
        for block in ["a", "b"] {
            for i in 0..5 {
                for j in i + 1..5 {
                    edges.push((format!("{block}{i}"), format!("{block}{j}"), 1.0));
                }
            }
        }
        edges.push(("a4".to_string(), "b0".to_string(), 0.1));
        AdjacencyGraph::from_edges(edges)
    }

    /// Verifies MEDULLA-FEDERATED-QUERY-R607.2: Behavior: partitions
    // spec: MEDULLA-FEDERATED-QUERY-R607.2
    #[test]
    fn planted_partition_is_recovered_and_ids_are_stable() {
        let g = planted();
        let first = spectral_partition(&g, 2, 7).unwrap();
        let a: Vec<String> = (0..5).map(|i| format!("a{i}")).collect();
        let b: Vec<String> = (0..5).map(|i| format!("b{i}")).collect();
        assert_eq!(first.partitions, vec![a.clone(), b.clone()]);
        assert!(first.modularity > 0.4, "modularity {}", first.modularity);
        assert!(first.iterations >= 2);
        for seed in [7, 7, 11] {
            assert_eq!(
                spectral_partition(&g, 2, seed).unwrap().partitions,
                first.partitions
            );
        }
        let cut = checked_mincut(&g).unwrap();
        assert!((cut.cut_weight - 0.1).abs() < 1e-9);
        assert_eq!(cut.left, a);
        assert_eq!(cut.right, b);

        let tiny = AdjacencyGraph::from_edges(vec![("x".to_string(), "y".to_string(), 1.0)]);
        assert_eq!(
            spectral_partition(&tiny, 2, 0),
            Err(TopologyKernelError::TrivialGraph)
        );
        assert_eq!(
            checked_mincut(&tiny).unwrap_err(),
            TopologyKernelError::TrivialGraph
        );
        let negative = AdjacencyGraph::from_edges(vec![
            ("x".to_string(), "y".to_string(), 1.0),
            ("y".to_string(), "z".to_string(), -1.0),
        ]);
        assert_eq!(
            spectral_partition(&negative, 2, 0),
            Err(TopologyKernelError::InvalidWeight)
        );
        assert_eq!(
            spectral_partition(&g, 1, 0),
            Err(TopologyKernelError::InvalidParam("k"))
        );
    }
}
