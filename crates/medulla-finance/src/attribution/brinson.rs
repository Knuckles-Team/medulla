//! Brinson-Fachler profit attribution (MEDULLA-FINANCE-PRIMITIVES-R750.751).
//!
//! With `R_p = sum(w_p * r_p)` and `R_b = sum(w_b * r_b)`, a group's allocation effect is
//! `(w_p - w_b) * (r_b - R_b)`, its selection effect `w_b * (r_p - r_b)` and its
//! interaction effect `(w_p - w_b) * (r_p - r_b)`. The three totals sum to
//! `R_p - R_b`. Plain `f64` summed serially in input order, so the result replays on
//! every target. The served caller validates the weights and returns first.

use medulla_result_types::compute_result::finance::{
    AttributionBreakdown, GroupEffect, GroupInput,
};

/// The Brinson-Fachler breakdown of `groups`, per group in input order plus totals.
pub fn brinson_fachler(groups: &[GroupInput]) -> AttributionBreakdown {
    let mut r_p = 0.0;
    let mut r_b = 0.0;
    for g in groups {
        r_p += g.w_p * g.r_p;
        r_b += g.w_b * g.r_b;
    }
    let mut effects = Vec::with_capacity(groups.len());
    let (mut allocation, mut selection, mut interaction) = (0.0, 0.0, 0.0);
    for g in groups {
        let a = (g.w_p - g.w_b) * (g.r_b - r_b);
        let s = g.w_b * (g.r_p - g.r_b);
        let i = (g.w_p - g.w_b) * (g.r_p - g.r_b);
        allocation += a;
        selection += s;
        interaction += i;
        effects.push(GroupEffect {
            group: g.group.clone(),
            allocation: a,
            selection: s,
            interaction: i,
        });
    }
    AttributionBreakdown {
        groups: effects,
        allocation,
        selection,
        interaction,
        active_return: r_p - r_b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(name: &str, w_p: f64, w_b: f64, r_p: f64, r_b: f64) -> GroupInput {
        GroupInput {
            group: name.into(),
            w_p,
            w_b,
            r_p,
            r_b,
        }
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R750.751
    #[test]
    fn effects_sum_to_active_return() {
        let groups = [
            group("equity", 0.6, 0.5, 0.08, 0.06),
            group("bonds", 0.3, 0.35, 0.02, 0.03),
            group("cash", 0.1, 0.15, 0.01, 0.01),
        ];
        let b = brinson_fachler(&groups);
        let r_p = 0.6 * 0.08 + 0.3 * 0.02 + 0.1 * 0.01;
        let r_b = 0.5 * 0.06 + 0.35 * 0.03 + 0.15 * 0.01;
        assert!((b.active_return - (r_p - r_b)).abs() < 1e-12);
        let total = b.allocation + b.selection + b.interaction;
        assert!((total - b.active_return).abs() < 1e-12, "{total}");
        assert_eq!(b.groups.len(), 3);
        assert_eq!(b.groups[0].group, "equity");
        let per_group: f64 = b
            .groups
            .iter()
            .map(|g| g.allocation + g.selection + g.interaction)
            .sum();
        assert!((per_group - b.active_return).abs() < 1e-12);
        // Equity selection: w_b * (r_p - r_b) = 0.5 * 0.02.
        assert!((b.groups[0].selection - 0.01).abs() < 1e-12);
        // Equity interaction: (0.6 - 0.5) * (0.08 - 0.06).
        assert!((b.groups[0].interaction - 0.002).abs() < 1e-12);
    }
}
