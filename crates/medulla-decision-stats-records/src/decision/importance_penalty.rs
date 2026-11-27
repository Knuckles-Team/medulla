//! The caller-supplied feature-importance penalty of `DecisionFit`
//! (MEDULLA-DECISION-ENGINE-R151.2).
//!
//! A fit that names a penalty pulls each coordinate toward the parent head in
//! proportion to the importance the caller gave that feature:
//! `penalty = loss_coefficient * importance_scale * sum_k importance_k *
//! (w_k - parent_k)^2`. A feature with no entry has importance 0, so it keeps
//! its variability. The engine computes no importance; it only applies it.

/// The resolved penalty: one importance and one parent weight per feature of
/// the fit, in the dataset's feature order.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportancePenalty {
    /// `loss_coefficient * importance_scale`.
    coefficient: f64,
    importance: Vec<f64>,
    parent: Vec<f64>,
}

impl ImportancePenalty {
    /// `importance` and `parent` are indexed like the fit's features. A missing
    /// parent head is `parent = zeros`. Returns `None` when the lengths differ.
    pub fn new(
        loss_coefficient: f64,
        importance_scale: f64,
        importance: Vec<f64>,
        parent: Vec<f64>,
    ) -> Option<Self> {
        (importance.len() == parent.len()).then_some(Self {
            coefficient: loss_coefficient * importance_scale,
            importance,
            parent,
        })
    }

    /// The number of features the penalty covers.
    pub fn width(&self) -> usize {
        self.importance.len()
    }

    /// `d penalty / d w_k` at `w`: `2 * coefficient * importance_k * (w - parent_k)`.
    pub fn slope(&self, k: usize, w: f64) -> f64 {
        2.0 * self.coefficient * self.importance[k] * (w - self.parent[k])
    }
}

/// The penalty value at `weights`. Cost is O(features).
pub fn weighted_penalty(penalty: &ImportancePenalty, weights: &[f64]) -> f64 {
    let mut total = 0.0;
    for (k, w) in weights.iter().enumerate().take(penalty.width()) {
        let drift = w - penalty.parent[k];
        total += penalty.importance[k] * drift * drift;
    }
    penalty.coefficient * total
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DECISION-ENGINE-R151.2
    #[test]
    fn penalty_respects_importance() {
        let weights = [0.5, 0.5];
        let high_first = ImportancePenalty::new(1.0, 1.0, vec![0.9, 0.1], vec![0.0, 0.0]).unwrap();
        let high_second = ImportancePenalty::new(1.0, 1.0, vec![0.1, 0.9], vec![0.0, 0.0]).unwrap();
        // Equal drift on both features costs the same in total, but one drift
        // on the important feature alone costs far more than on the other.
        assert!(
            weighted_penalty(&high_first, &[0.5, 0.0]) > weighted_penalty(&high_first, &[0.0, 0.5])
        );
        assert!(
            weighted_penalty(&high_second, &[0.0, 0.5])
                > weighted_penalty(&high_second, &[0.5, 0.0])
        );
        assert_eq!(
            weighted_penalty(&high_first, &weights),
            weighted_penalty(&high_second, &weights)
        );
        // No importance, no cost; no drift, no cost.
        let none = ImportancePenalty::new(1.0, 1.0, vec![0.0, 0.0], vec![0.0, 0.0]).unwrap();
        assert_eq!(weighted_penalty(&none, &[3.0, -3.0]), 0.0);
        let anchored = ImportancePenalty::new(1.0, 1.0, vec![1.0, 1.0], vec![0.2, 0.4]).unwrap();
        assert_eq!(weighted_penalty(&anchored, &[0.2, 0.4]), 0.0);
        // A length mismatch is not a penalty.
        assert!(ImportancePenalty::new(1.0, 1.0, vec![1.0], vec![]).is_none());
    }

    // spec: MEDULLA-DECISION-ENGINE-R151.2
    #[test]
    fn gradient_matches_finite_difference() {
        let p =
            ImportancePenalty::new(1.5, 0.5, vec![0.9, 0.3, 0.0], vec![0.1, -0.2, 0.7]).unwrap();
        let at = [0.4, 0.25, -1.0];
        let h = 1e-6;
        for k in 0..3 {
            let mut up = at;
            let mut down = at;
            up[k] += h;
            down[k] -= h;
            let numeric = (weighted_penalty(&p, &up) - weighted_penalty(&p, &down)) / (2.0 * h);
            assert!((numeric - p.slope(k, at[k])).abs() < 1e-6, "coordinate {k}");
        }
    }
}
