// CONCEPT:MEDULLA-KG.domains.market-microstructure-sizing-backtest — Market-Microstructure, Sizing & Backtest-Validation Kernels
//
// Batched, stateless quantitative kernels for HFT market-making on binary CLOB
// venues (Polymarket / Kalshi) and for rigorous strategy validation. Served over
// the Tokio MessagePack protocol — every op is one round-trip over whole arrays,
// never a per-element loop (see AGENTS.md "Batch, never per-element").
//
// Math is implemented from primary sources, not copied from blogs:
//   - Avellaneda & Stoikov (2008) optimal market making
//   - Guéant, Lehalle, Fernandez-Tapia (2013) closed form with inventory bound
//   - logit-space reformulation for bounded (0,1) prediction-market prices
//   - Glosten & Milgrom (1985) adverse-selection spread
//   - Hawkes (1971) self-exciting process; MLE + Hardiman-Bouchaud branching ratio
//   - Cont-Kukanov-Stoikov OFI, Stoikov microprice, VPIN (Easley-LdP-O'Hara)
//   - Kelly (1956) + Bayesian (Beta-posterior) Kelly
//   - López de Prado: purged combinatorial CV, Deflated Sharpe, PBO; Diebold-Mariano

#[inline]
fn logit(p: f64) -> f64 {
    (p / (1.0 - p)).ln()
}
#[inline]
fn sigmoid(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

#[path = "quant/market_making.rs"]
mod market_making;
#[path = "quant/microstructure.rs"]
mod microstructure;
#[path = "quant/signals.rs"]
mod signals;
#[path = "quant/surveillance.rs"]
mod surveillance;

pub use market_making::{
    avellaneda_stoikov, breakeven_alpha, expected_pnl_rate, glosten_milgrom_spread, glt_quotes,
    logit_space_quotes, Quote,
};
pub(super) use microstructure::nelder_mead_3;
pub use microstructure::{
    hardiman_bouchaud_branching_ratio, hawkes_mle, microprice_series, ofi_series, vpin_pm,
    HawkesFit,
};
pub use signals::{
    alpha_combination_engine, brier_score, convergence_gate, effective_independent_n,
    empirical_kelly, information_ratio, order_book_imbalance, queue_imbalance, realized_vol_tick,
    spread_reversion, ConvergenceGate, QueueSignal, SpreadReversion,
};
pub use surveillance::{kyle_lambda, surveillance_risk, SurveillanceRisk};

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_compute::finance::market::validation::*;

    #[test]
    fn test_avellaneda_stoikov_inventory_skew() {
        let flat = avellaneda_stoikov(100.0, 0.0, 0.02, 0.1, 1.5, 1.0);
        // symmetric around mid when inventory is zero
        assert!((flat.reservation - 100.0).abs() < 1e-9);
        assert!(flat.bid < flat.ask);
        // long inventory pushes reservation below mid (wants to sell)
        let long = avellaneda_stoikov(100.0, 10.0, 0.02, 0.1, 1.5, 1.0);
        assert!(long.reservation < 100.0);
    }

    #[test]
    fn test_logit_quotes_bounded_and_cap() {
        let q = logit_space_quotes(0.5, 0.0, 0.5, 0.1, 1.5, 1.0, 100.0);
        assert!(q.bid > 0.0 && q.ask < 1.0 && q.bid < q.ask);
        assert!(!q.withdraw);
        // huge inventory near boundary triggers withdraw
        let q2 = logit_space_quotes(0.02, 500.0, 0.5, 0.1, 1.5, 1.0, 100.0);
        assert!(q2.withdraw);
    }

    #[test]
    fn test_glosten_milgrom_and_breakeven() {
        assert!((glosten_milgrom_spread(0.2, 0.5) - 0.1).abs() < 1e-9);
        let be = breakeven_alpha(0.01, 0.5, 1.0, 0.0);
        assert!(be > 0.0 && be < 1.0);
    }

    #[test]
    fn test_microprice_and_vpin() {
        let mp = microprice_series(&[0.49], &[100.0], &[0.51], &[100.0]);
        assert!((mp[0] - 0.5).abs() < 1e-9);
        // balanced buy/sell ⇒ low toxicity; one-sided ⇒ higher
        let balanced = vpin_pm(&[50.0], &[50.0], &[0.5]);
        let toxic = vpin_pm(&[100.0], &[0.0], &[0.5]);
        assert!(toxic > balanced);
    }

    #[test]
    fn test_ofi_series_runs() {
        let ts = vec![0.0, 0.5, 1.0, 1.5];
        let bp = vec![0.49, 0.49, 0.50, 0.50];
        let bs = vec![100.0, 120.0, 130.0, 130.0];
        let ap = vec![0.51, 0.51, 0.51, 0.52];
        let as_ = vec![100.0, 90.0, 90.0, 80.0];
        let ofi = ofi_series(&ts, &bp, &bs, &ap, &as_, 1.0);
        assert_eq!(ofi.len(), 4);
    }

    #[test]
    fn test_hawkes_mle_recovers_excitation() {
        // synthetic clustered times; just assert it fits within stationarity
        let times: Vec<f64> = (0..50)
            .map(|i| i as f64 * 0.3 + (i % 3) as f64 * 0.05)
            .collect();
        let fit = hawkes_mle(&times, 20.0, 200);
        assert!(fit.mu > 0.0);
        assert!(fit.branching_ratio >= 0.0 && fit.branching_ratio < 1.0);
        assert!(fit.beta > 0.0);
    }

    #[test]
    fn test_hardiman_bouchaud() {
        let times: Vec<f64> = (0..100).map(|i| i as f64 * 0.1).collect();
        let n = hardiman_bouchaud_branching_ratio(&times, 10.0, 20);
        assert!((0.0..1.0).contains(&n));
    }

    #[test]
    fn test_kelly() {
        // q=0.6, c=0.5 -> f* = 0.2; quarter-kelly -> 0.05
        let f = kelly_fraction(0.6, 0.5, 0.25);
        assert!((f - 0.05).abs() < 1e-9);
        assert_eq!(kelly_fraction(0.4, 0.5, 0.25), 0.0); // negative EV killed
    }

    #[test]
    fn test_bayesian_kelly_shrinks_with_uncertainty() {
        // tight posterior around 0.6 vs wide posterior, same mean
        let tight = bayesian_kelly_fraction(60.0, 40.0, 0.5, 32);
        let wide = bayesian_kelly_fraction(6.0, 4.0, 0.5, 32);
        assert!(tight >= wide); // more uncertainty ⇒ smaller (or equal) bet
        let (lo, hi) = posterior_credible_interval(60.0, 40.0, 0.05);
        assert!(lo < 0.6 && hi > 0.6);
    }

    #[test]
    fn test_purged_cpcv() {
        let splits = purged_cpcv_splits(120, 6, 2, 5, 5);
        // C(6,2) = 15 splits
        assert_eq!(splits.len(), 15);
        for s in &splits {
            // train and test never overlap
            let tset: std::collections::HashSet<_> = s.test.iter().collect();
            assert!(s.train.iter().all(|i| !tset.contains(i)));
        }
    }

    #[test]
    fn test_deflated_sharpe_and_pbo() {
        let rets: Vec<f64> = (0..100)
            .map(|i| 0.01 + 0.001 * ((i % 7) as f64 - 3.0))
            .collect();
        let dsr = deflated_sharpe_ratio(1.5, 10, &rets);
        assert!((0.0..=1.0).contains(&dsr));
        // IS-best always OOS-best ⇒ PBO = 0
        let is = vec![vec![0.1, 0.2, 0.3], vec![0.3, 0.2, 0.1]];
        let oos = vec![vec![0.1, 0.2, 0.3], vec![0.3, 0.2, 0.1]];
        let pbo = probability_of_backtest_overfit(&is, &oos);
        assert!(pbo < 0.5);
    }

    /// A fixture with real skill: four strategies (median rank 1.5), four
    /// splits. The IS-best's OOS rank (0 = worst, ties do not lift it) is
    /// 3, 2, 0 and 2, so only split 2 lands below the median: PBO = 1/4.
    /// With three strategies the median rank is exactly 1, and a split whose
    /// IS-best sits exactly at the OOS median is not counted (strictly below).
    #[test]
    fn pbo_counts_only_splits_whose_is_best_lands_below_the_oos_median() {
        let is = vec![
            vec![0.1, 0.9, 0.2, 0.3],
            vec![0.5, 0.1, 0.2, 0.3],
            vec![0.2, 0.1, 0.7, 0.3],
            vec![0.1, 0.2, 0.3, 0.6],
        ];
        let oos = vec![
            vec![0.1, 0.8, 0.2, 0.3],
            vec![0.4, 0.1, 0.5, 0.2],
            vec![0.3, 0.4, 0.1, 0.2],
            vec![0.2, 0.5, 0.1, 0.5],
        ];
        let pbo = probability_of_backtest_overfit(&is, &oos);
        assert!((pbo - 0.25).abs() < f64::EPSILON, "pbo {pbo}");
        let at_median =
            probability_of_backtest_overfit(&[vec![0.9, 0.1, 0.2]], &[vec![0.2, 0.1, 0.3]]);
        assert!(at_median.abs() < f64::EPSILON, "at-median pbo {at_median}");
    }

    #[test]
    fn test_diebold_mariano() {
        // A strictly lower loss than B ⇒ A better, significant
        let a: Vec<f64> = (0..50).map(|_| 1.0).collect();
        let b: Vec<f64> = (0..50).map(|_| 2.0).collect();
        let dm = diebold_mariano(&a, &b, 1);
        assert!(dm.a_better);
    }

    #[test]
    fn test_order_book_imbalance() {
        let obi = order_book_imbalance(&[100.0, 50.0], &[0.0, 50.0]);
        assert!((obi[0] - 1.0).abs() < 1e-9); // all bid
        assert!(obi[1].abs() < 1e-9); // balanced
    }

    #[test]
    fn test_queue_imbalance() {
        // balanced queue ⇒ skew ≈ 0
        let q = queue_imbalance(
            &[100.0, 50.0],
            &[100.0, 150.0],
            &[10.0, 10.0],
            &[10.0, 10.0],
        );
        assert!(q.skew[0].abs() < 1e-9);
        // ask queue heavier ⇒ positive skew (bid fills faster)
        assert!(q.skew[1] > 0.0);
        // fill time = queue_ahead / rate
        assert!((q.bid_fill_time[0] - 10.0).abs() < 1e-9);
        assert!((q.ask_fill_time[1] - 15.0).abs() < 1e-9);
    }

    #[test]
    fn test_realized_vol_tick() {
        // constant mid ⇒ zero realized vol everywhere
        let flat = realized_vol_tick(&[100.0, 100.0, 100.0, 100.0], 3);
        assert!(flat.iter().all(|v| v.abs() < 1e-12));
        // a moving series ⇒ strictly positive rolling RV
        let rv = realized_vol_tick(&[100.0, 101.0, 100.0, 102.0, 101.0], 3);
        assert_eq!(rv.len(), 5);
        assert!(rv[4] > 0.0);
    }

    #[test]
    fn test_spread_reversion() {
        // a sudden spread widening at the end ⇒ negative reversion signal (expect tighten)
        let bid = vec![0.50, 0.50, 0.50, 0.50, 0.50, 0.50];
        let ask = vec![0.52, 0.52, 0.52, 0.52, 0.52, 0.60];
        let sr = spread_reversion(&bid, &ask, 5);
        assert_eq!(sr.signal.len(), 6);
        assert!(sr.zscore[5] > 0.0 && sr.signal[5] < 0.0);
    }

    #[test]
    fn test_information_ratio_and_effective_n() {
        // IR = IC*sqrt(N): 0.05 * sqrt(50) ≈ 0.3536
        assert!((information_ratio(0.05, 50.0) - 0.353_553).abs() < 1e-4);
        // two identical (perfectly correlated) signals ⇒ N_eff ≈ 1
        let s = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let neff_corr = effective_independent_n(&[s.clone(), s.clone()]);
        assert!(neff_corr < 1.5, "neff={}", neff_corr);
        // two independent-ish signals ⇒ N_eff closer to 2
        let a = vec![1.0, -1.0, 1.0, -1.0, 1.0, -1.0];
        let b = vec![1.0, 1.0, -1.0, -1.0, 1.0, 1.0];
        let neff2 = effective_independent_n(&[a, b]);
        assert!(neff2 > neff_corr);
    }

    #[test]
    fn test_alpha_combination_engine_weights_sum_to_one() {
        let m = vec![
            vec![0.01, 0.02, -0.01, 0.03, 0.0, 0.01, 0.02, -0.02],
            vec![-0.01, 0.0, 0.02, -0.01, 0.01, 0.0, -0.01, 0.02],
            vec![0.02, -0.01, 0.0, 0.01, -0.02, 0.01, 0.0, 0.01],
        ];
        let w = alpha_combination_engine(&m, 4);
        assert_eq!(w.len(), 3);
        let abs_sum: f64 = w.iter().map(|x| x.abs()).sum();
        assert!((abs_sum - 1.0).abs() < 1e-9, "abs_sum={}", abs_sum);
    }

    #[test]
    fn test_brier_score() {
        // perfect calibration ⇒ 0
        assert!(brier_score(&[1.0, 0.0], &[1.0, 0.0]).abs() < 1e-12);
        // p=0.5 on every outcome ⇒ 0.25
        assert!((brier_score(&[0.5, 0.5, 0.5, 0.5], &[1.0, 0.0, 1.0, 0.0]) - 0.25).abs() < 1e-9);
    }

    #[test]
    fn test_convergence_gate() {
        // 5/5 strong up
        let g = convergence_gate(&[0.9, 0.8, 0.95, 0.85, 0.9], 0.6, 5);
        assert!(g.pass && g.direction == 1 && g.agree == 5);
        // only 3 strong, need 5 ⇒ fail
        let g2 = convergence_gate(&[0.9, 0.8, 0.1, 0.0, 0.7], 0.6, 5);
        assert!(!g2.pass);
    }

    #[test]
    fn test_empirical_kelly_penalises_uncertainty() {
        // positive-EV bet, stable returns ⇒ close to raw Kelly
        let stable: Vec<f64> = (0..200).map(|_| 0.02).collect();
        let f_stable = empirical_kelly(0.6, 1.0, &stable, 500, 42);
        // same bet but noisy edge ⇒ smaller fraction
        let noisy: Vec<f64> = (0..200)
            .map(|i| 0.02 + 0.2 * ((i % 5) as f64 - 2.0))
            .collect();
        let f_noisy = empirical_kelly(0.6, 1.0, &noisy, 500, 42);
        assert!(f_stable > 0.0);
        assert!(f_noisy <= f_stable, "stable={} noisy={}", f_stable, f_noisy);
        // negative EV ⇒ 0
        assert_eq!(empirical_kelly(0.4, 1.0, &stable, 100, 1), 0.0);
    }

    #[test]
    fn test_kyle_lambda_recovers_slope() {
        // price_change = 0.5 * signed_flow exactly ⇒ λ = 0.5
        let flow: Vec<f64> = (1..=20).map(|i| i as f64).collect();
        let dp: Vec<f64> = flow.iter().map(|q| 0.5 * q).collect();
        assert!((kyle_lambda(&dp, &flow) - 0.5).abs() < 1e-9);
        // no flow variance ⇒ 0 (no division blow-up)
        assert_eq!(kyle_lambda(&[0.1, 0.2], &[3.0, 3.0]), 0.0);
        assert_eq!(kyle_lambda(&[], &[]), 0.0);
    }

    #[test]
    fn test_surveillance_risk_separates_toxic_from_benign() {
        // Benign: near-balanced two-sided flow near σ ⇒ low hazard, low toxicity,
        // mostly noise (high camouflage ratio).
        let benign = surveillance_risk(
            &[110.0, 110.0, 110.0],
            &[100.0, 100.0, 100.0],
            &[0.5, 0.5, 0.5],
            &[1.0, -1.0, 1.0],
            &[0.01, -0.01, 0.01],
            1.0,
        );
        // Toxic: persistent one-sided buying far above the noise baseline.
        let toxic = surveillance_risk(
            &[500.0, 500.0, 500.0],
            &[5.0, 5.0, 5.0],
            &[0.5, 0.5, 0.5],
            &[8.0, 9.0, 10.0],
            &[0.4, 0.45, 0.5],
            1.0,
        );
        for r in [&benign, &toxic] {
            assert!((0.0..=1.0).contains(&r.legal_risk_score));
            assert!(r.informed_share >= 0.0);
        }
        assert!(
            toxic.legal_risk_score > benign.legal_risk_score,
            "toxic={} benign={}",
            toxic.legal_risk_score,
            benign.legal_risk_score
        );
        assert!(toxic.detection_hazard > benign.detection_hazard);
        assert!(toxic.cumulative_suspicion > benign.cumulative_suspicion);
        // toxic flow is mostly informed ⇒ low camouflage; benign is mostly noise.
        assert!(toxic.stealth_ratio < benign.stealth_ratio);
    }
}
