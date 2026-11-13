// CONCEPT:MEDULLA-KG.compute.hmm-regime-detection — HMM Regime Detection Engine
//
// Hidden Markov Model for market regime detection.
// Implements Baum-Welch (EM) for parameter estimation and Viterbi for decoding.
// Replaces Python hmmlearn.

use medulla_types::numeric_kernel::RegimeResult;

use crate::detkernel::math;

/// `ln`, `exp` and `x²` through the pinned software math library.
trait Det {
    fn dln(self) -> f64;
    fn dexp(self) -> f64;
    fn dsq(self) -> f64;
}

impl Det for f64 {
    fn dln(self) -> f64 {
        math::ln(self)
    }
    fn dexp(self) -> f64 {
        math::exp(self)
    }
    fn dsq(self) -> f64 {
        self * self
    }
}

/// Normal PDF: N(x | μ, σ)
fn normal_pdf(x: f64, mean: f64, std: f64) -> f64 {
    let z = (x - mean) / std;
    (-0.5 * z * z).dexp() / (std * (2.0 * std::f64::consts::PI).sqrt())
}

/// Log-sum-exp for numerical stability.
fn log_sum_exp(values: &[f64]) -> f64 {
    let max_val = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    if max_val.is_infinite() {
        return f64::NEG_INFINITY;
    }
    max_val
        + values
            .iter()
            .map(|v| (v - max_val).dexp())
            .sum::<f64>()
            .dln()
}

struct HmmParameters {
    means: Vec<f64>,
    stds: Vec<f64>,
    initial_probs: Vec<f64>,
    transition_matrix: Vec<Vec<f64>>,
}

fn initialize_parameters(observations: &[f64], n_states: usize) -> HmmParameters {
    let (t, mut sorted) = (observations.len(), observations.to_vec());
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let global_mean: f64 = observations.iter().sum::<f64>() / t as f64;
    let global_var: f64 = observations
        .iter()
        .map(|x| (x - global_mean).dsq())
        .sum::<f64>()
        / t as f64;
    let global_std = global_var.sqrt().max(1e-8);

    let means: Vec<f64> = (0..n_states)
        .map(|i| {
            let quantile = (i as f64 + 0.5) / n_states as f64;
            let idx = (quantile * (t - 1) as f64) as usize;
            sorted[idx.min(t - 1)]
        })
        .collect();
    let stds = vec![global_std; n_states];
    let initial_probs = vec![1.0 / n_states as f64; n_states];
    let transition_matrix = vec![vec![1.0 / n_states as f64; n_states]; n_states];

    HmmParameters {
        means,
        stds,
        initial_probs,
        transition_matrix,
    }
}

#[allow(clippy::needless_range_loop)] // the index also drives a second sequence in lockstep (DP recurrence); iterator zips would obscure the maths
fn forward_log_probabilities(observations: &[f64], params: &HmmParameters) -> Vec<Vec<f64>> {
    let (t, n_states) = (observations.len(), params.means.len());
    let mut log_alpha = vec![vec![0.0_f64; n_states]; t];
    for s in 0..n_states {
        log_alpha[0][s] = params.initial_probs[s].max(1e-300).dln()
            + normal_pdf(observations[0], params.means[s], params.stds[s])
                .max(1e-300)
                .dln();
    }
    for tt in 1..t {
        for j in 0..n_states {
            let emission = normal_pdf(observations[tt], params.means[j], params.stds[j])
                .max(1e-300)
                .dln();
            let trans_probs: Vec<f64> = (0..n_states)
                .map(|i| log_alpha[tt - 1][i] + params.transition_matrix[i][j].max(1e-300).dln())
                .collect();
            log_alpha[tt][j] = log_sum_exp(&trans_probs) + emission;
        }
    }
    log_alpha
}

fn backward_log_probabilities(observations: &[f64], params: &HmmParameters) -> Vec<Vec<f64>> {
    let (t, n_states) = (observations.len(), params.means.len());
    let mut log_beta = vec![vec![0.0_f64; n_states]; t];
    for tt in (0..t - 1).rev() {
        for i in 0..n_states {
            let vals: Vec<f64> = (0..n_states)
                .map(|j| {
                    params.transition_matrix[i][j].max(1e-300).dln()
                        + normal_pdf(observations[tt + 1], params.means[j], params.stds[j])
                            .max(1e-300)
                            .dln()
                        + log_beta[tt + 1][j]
                })
                .collect();
            log_beta[tt][i] = log_sum_exp(&vals);
        }
    }
    log_beta
}

fn posterior_probabilities(log_alpha: &[Vec<f64>], log_beta: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let (t, n_states) = (log_alpha.len(), log_alpha[0].len());
    let mut gamma = vec![vec![0.0_f64; n_states]; t];
    for tt in 0..t {
        let vals: Vec<f64> = (0..n_states)
            .map(|s| log_alpha[tt][s] + log_beta[tt][s])
            .collect();
        let norm = log_sum_exp(&vals);
        for s in 0..n_states {
            gamma[tt][s] = (log_alpha[tt][s] + log_beta[tt][s] - norm).dexp();
        }
    }
    gamma
}

#[allow(clippy::needless_range_loop)] // the index also drives a second sequence in lockstep (DP recurrence); iterator zips would obscure the maths
fn update_initial_probabilities(gamma: &[Vec<f64>], params: &mut HmmParameters) {
    let gamma_sum_0: f64 = gamma[0].iter().sum();
    for s in 0..params.initial_probs.len() {
        params.initial_probs[s] = (gamma[0][s] / gamma_sum_0).max(1e-8);
    }
}

#[allow(clippy::needless_range_loop)] // the index also drives a second sequence in lockstep (DP recurrence); iterator zips would obscure the maths
fn update_transition_matrix(
    observations: &[f64],
    log_alpha: &[Vec<f64>],
    log_beta: &[Vec<f64>],
    log_likelihood: f64,
    params: &mut HmmParameters,
) {
    let (t, n_states) = (observations.len(), params.means.len());
    for i in 0..n_states {
        let mut row_sum = 0.0;
        for j in 0..n_states {
            let mut xi_sum = 0.0;
            for tt in 0..t - 1 {
                let xi_val = (log_alpha[tt][i]
                    + params.transition_matrix[i][j].max(1e-300).dln()
                    + normal_pdf(observations[tt + 1], params.means[j], params.stds[j])
                        .max(1e-300)
                        .dln()
                    + log_beta[tt + 1][j]
                    - log_likelihood)
                    .dexp();
                xi_sum += xi_val;
            }
            params.transition_matrix[i][j] = xi_sum.max(1e-8);
            row_sum += params.transition_matrix[i][j];
        }
        for j in 0..n_states {
            params.transition_matrix[i][j] /= row_sum.max(1e-8);
        }
    }
}

fn update_emissions(observations: &[f64], gamma: &[Vec<f64>], params: &mut HmmParameters) {
    let n_states = params.means.len();
    for s in 0..n_states {
        let gamma_s_sum: f64 = gamma.iter().map(|g| g[s]).sum();
        if gamma_s_sum > 1e-8 {
            params.means[s] = gamma
                .iter()
                .enumerate()
                .map(|(tt, g)| g[s] * observations[tt])
                .sum::<f64>()
                / gamma_s_sum;

            let var: f64 = gamma
                .iter()
                .enumerate()
                .map(|(tt, g)| g[s] * (observations[tt] - params.means[s]).dsq())
                .sum::<f64>()
                / gamma_s_sum;
            params.stds[s] = var.sqrt().max(1e-8);
        }
    }
}

/// Detect market regimes using a Gaussian HMM with Baum-Welch EM.
///
/// # Arguments
/// * `observations` — time series of returns
/// * `n_states` — number of hidden states (typically 2-3: bull/bear/neutral)
/// * `max_iter` — maximum EM iterations
/// * `tol` — convergence tolerance on log-likelihood
pub fn detect_regimes(
    observations: &[f64],
    n_states: usize,
    max_iter: usize,
    tol: f64,
) -> RegimeResult {
    let t = observations.len();
    if t == 0 || n_states == 0 {
        return RegimeResult {
            states: vec![],
            means: vec![],
            stds: vec![],
            transition_matrix: vec![],
            initial_probs: vec![],
            log_likelihood: 0.0,
            n_iterations: 0,
        };
    }

    let mut params = initialize_parameters(observations, n_states);
    let mut prev_ll = f64::NEG_INFINITY;
    let mut n_iter = 0;

    for iter in 0..max_iter {
        n_iter = iter + 1;

        // E-step: Forward-backward
        let log_alpha = forward_log_probabilities(observations, &params);
        let log_beta = backward_log_probabilities(observations, &params);

        // Log-likelihood
        let ll_parts: Vec<f64> = (0..n_states).map(|s| log_alpha[t - 1][s]).collect();
        let ll = log_sum_exp(&ll_parts);

        if (ll - prev_ll).abs() < tol {
            break;
        }
        prev_ll = ll;

        // Compute gamma (state posteriors) and xi (transition posteriors)
        let gamma = posterior_probabilities(&log_alpha, &log_beta);

        // M-step: update parameters
        update_initial_probabilities(&gamma, &mut params);
        update_transition_matrix(observations, &log_alpha, &log_beta, ll, &mut params);
        update_emissions(observations, &gamma, &mut params);
    }

    // Viterbi decoding for most likely state sequence
    let states = viterbi(
        observations,
        &params.means,
        &params.stds,
        &params.initial_probs,
        &params.transition_matrix,
    );

    RegimeResult {
        states,
        means: params.means,
        stds: params.stds,
        transition_matrix: params.transition_matrix,
        initial_probs: params.initial_probs,
        log_likelihood: prev_ll,
        n_iterations: n_iter,
    }
}

/// Viterbi algorithm — find the most likely hidden state sequence.
fn viterbi(
    observations: &[f64],
    means: &[f64],
    stds: &[f64],
    pi: &[f64],
    trans: &[Vec<f64>],
) -> Vec<usize> {
    let t = observations.len();
    let n = means.len();
    if t == 0 {
        return vec![];
    }

    let mut v = vec![vec![0.0_f64; n]; t];
    let mut backtrack = vec![vec![0usize; n]; t];
    let model = ViterbiModel {
        means,
        stds,
        trans,
        n,
    };

    init_viterbi_start(&mut v[0], observations[0], &model, pi);

    for tt in 1..t {
        viterbi_step(&mut v, &mut backtrack, tt, observations, &model);
    }

    backtrack_viterbi_path(&v, &backtrack, t, n)
}

/// The emission means/stds and transition matrix a Viterbi pass runs against,
/// bundled so the per-step helpers stay under a handful of parameters.
struct ViterbiModel<'a> {
    means: &'a [f64],
    stds: &'a [f64],
    trans: &'a [Vec<f64>],
    n: usize,
}

/// `t = 0` row: `log(π_s) + log N(x_0 | μ_s, σ_s)` for every state `s`.
fn init_viterbi_start(v0: &mut [f64], x0: f64, model: &ViterbiModel, pi: &[f64]) {
    for s in 0..model.n {
        v0[s] = pi[s].max(1e-300).dln()
            + normal_pdf(x0, model.means[s], model.stds[s])
                .max(1e-300)
                .dln();
    }
}

/// One Viterbi recursion step at time `tt`: for every state `j`, the best
/// predecessor at `tt - 1` plus the emission log-probability.
fn viterbi_step(
    v: &mut [Vec<f64>],
    backtrack: &mut [Vec<usize>],
    tt: usize,
    observations: &[f64],
    model: &ViterbiModel,
) {
    for j in 0..model.n {
        let emission = normal_pdf(observations[tt], model.means[j], model.stds[j])
            .max(1e-300)
            .dln();
        let (best_state, best_val) = best_predecessor(&v[tt - 1], j, model.trans, model.n);
        v[tt][j] = best_val + emission;
        backtrack[tt][j] = best_state;
    }
}

/// The predecessor state `i` maximizing `v_prev[i] + log(trans[i][j])`.
fn best_predecessor(v_prev: &[f64], j: usize, trans: &[Vec<f64>], n: usize) -> (usize, f64) {
    let mut best_val = f64::NEG_INFINITY;
    let mut best_state = 0;
    for i in 0..n {
        let score = v_prev[i] + trans[i][j].max(1e-300).dln();
        if score > best_val {
            best_val = score;
            best_state = i;
        }
    }
    (best_state, best_val)
}

/// Recover the most-likely state sequence from the Viterbi trellis.
#[allow(clippy::needless_range_loop)] // the index also drives a second sequence in lockstep (DP recurrence); iterator zips would obscure the maths
fn backtrack_viterbi_path(
    v: &[Vec<f64>],
    backtrack: &[Vec<usize>],
    t: usize,
    n: usize,
) -> Vec<usize> {
    let mut states = vec![0usize; t];
    let mut best_last = 0;
    let mut best_val = f64::NEG_INFINITY;
    for s in 0..n {
        if v[t - 1][s] > best_val {
            best_val = v[t - 1][s];
            best_last = s;
        }
    }
    states[t - 1] = best_last;
    for tt in (0..t - 1).rev() {
        states[tt] = backtrack[tt + 1][states[tt + 1]];
    }

    states
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evaluation::registry::evaluate;
    use medulla_types::numeric_kernel::{
        KernelCode, NumericEvaluateOp as Op, MAX_REFIT_OBSERVATIONS,
    };

    // spec: MEDULLA-FINANCE-PRIMITIVES-R002.1.14
    #[test]
    fn regime_hmm_independent_vectors() {
        // Two constant regimes of known mean decode to a clean two-block Viterbi path.
        let mut obs = vec![0.01; 50];
        obs.extend(std::iter::repeat_n(-0.02, 50));
        let fit = detect_regimes(&obs, 2, 100, 1e-6);
        assert_eq!(fit.states.len(), 100);
        assert!(fit.states[..50].iter().all(|s| *s == fit.states[0]));
        assert!(fit.states[50..].iter().all(|s| *s == fit.states[99]));
        assert_ne!(fit.states[0], fit.states[99]);
        let mut means = fit.means.clone();
        means.sort_by(f64::total_cmp);
        assert!((means[0] + 0.02).abs() < 1e-4 && (means[1] - 0.01).abs() < 1e-4);

        let code = |op: Op| evaluate(&op).unwrap_err().code;
        let hmm = |observations: Vec<f64>, n_states: usize| Op::RegimeHmm {
            observations,
            n_states,
            max_iter: 10,
            tol: 1e-6,
        };
        assert_eq!(code(hmm(vec![0.1], 2)), KernelCode::InputShort);
        assert_eq!(code(hmm(vec![0.1, 0.2], 0)), KernelCode::ParamInvalid);
        assert_eq!(
            code(hmm(vec![0.0; MAX_REFIT_OBSERVATIONS + 1], 2)),
            KernelCode::StateBudget
        );
    }
}
