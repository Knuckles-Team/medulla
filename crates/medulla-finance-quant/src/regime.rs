// CONCEPT:MEDULLA-KG.compute.hmm-regime-detection — HMM Regime Detection Engine
//
// The Gaussian HMM (Baum-Welch EM + Viterbi) lives in
// `medulla_numeric::evaluation::regime_hmm` (MEDULLA-FINANCE-PRIMITIVES-R002.1.14): one
// implementation behind `NumericEvaluate` op `regime_hmm`, reached through the
// numeric registry; nothing here wraps it.

#[cfg(test)]
mod tests {
    use medulla_numeric::evaluation::regime_hmm::detect_regimes;

    #[test]
    fn test_regime_detection_two_states() {
        // Generate synthetic two-regime data
        // Bull regime (50 obs) then bear regime (50 obs).
        let mut obs = vec![0.01; 50];
        obs.extend(std::iter::repeat_n(-0.02, 50));

        let result = detect_regimes(&obs, 2, 100, 1e-6);
        assert_eq!(result.states.len(), 100);
        assert_eq!(result.means.len(), 2);
        assert_eq!(result.transition_matrix.len(), 2);
    }

    #[test]
    fn test_empty_observations() {
        let result = detect_regimes(&[], 2, 100, 1e-6);
        assert!(result.states.is_empty());
    }

    #[test]
    fn test_regime_parameters_remain_normalized() {
        let observations = [0.02, 0.018, 0.021, -0.03, -0.028, -0.031];
        let result = detect_regimes(&observations, 2, 100, 1e-8);

        assert!(result.means.iter().all(|mean| mean.is_finite()));
        assert!(result.stds.iter().all(|std| *std >= 1e-8));
        assert!((result.initial_probs.iter().sum::<f64>() - 1.0).abs() < 1e-6);
        for row in &result.transition_matrix {
            assert!((row.iter().sum::<f64>() - 1.0).abs() < 1e-6);
        }
    }
}
