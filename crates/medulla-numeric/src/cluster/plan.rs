//! The scaling law and the memory formula (MEDULLA-CLUSTER-R002.1, R006.1).

use super::config::ClusterConfig;

/// Training mode of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Full-batch: every step uses every row.
    Exact,
    /// Seeded mini-batch updates with moving-average centroids.
    EMeans,
}

/// The sizes a run uses, derived from `n`, `k`, `d` and the knobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    pub batch: usize,
    pub steps: u32,
    pub mode: Mode,
}

/// Batch size and step budget for `n` rows, `k` cells and `d` dimensions.
pub fn plan(n: usize, k: usize, d: usize, cfg: &ClusterConfig) -> Plan {
    let max_steps = cfg.max_steps.max(1);
    let min_steps = cfg.min_steps.clamp(1, max_steps);
    let mut batch = if cfg.batch_rows > 0 {
        cfg.batch_rows
    } else {
        let wanted = k
            .saturating_mul(32)
            .clamp(1024, cfg.batch_max_rows.max(1024));
        let per_row = (k as u64).saturating_mul(d as u64).max(1);
        let cap = usize::try_from(cfg.step_budget_madds / per_row).unwrap_or(usize::MAX);
        wanted.min(cap.max(n.min(256)))
    };
    batch = batch.max(1);
    if cfg.exact || n <= batch {
        return Plan {
            batch: n,
            steps: max_steps,
            mode: Mode::Exact,
        };
    }
    let passes = (cfg.epochs * n as f64 / batch as f64).ceil();
    let steps = if passes.is_finite() && passes > 0.0 {
        (passes.min(f64::from(u32::MAX)) as u32).clamp(min_steps, max_steps)
    } else {
        min_steps
    };
    Plan {
        batch,
        steps,
        mode: Mode::EMeans,
    }
}

/// Peak extra bytes of one run, without the borrowed input rows (R006.1).
///
/// Centroids in `f32` (`4kd`), per-centroid counters (`24k`), the seeding sample
/// and its distances (`64k + 32768`), the distinct-row table (`32k`), the batch
/// index, label, distance and order arrays (`16 * batch`) and one `f64` accumulator
/// row (`8d`). There is no `k*d*threads` term.
pub fn training_bytes(k: usize, d: usize, batch: usize) -> u64 {
    let (k, d, b) = (k as u64, d as u64, batch as u64);
    4 * k * d + 120 * k + 32_768 + 16 * b + 8 * d
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> ClusterConfig {
        ClusterConfig::default()
    }

    // spec: MEDULLA-CLUSTER-R002.1

    #[test]
    fn scaling_law_matches_table() {
        // k=1024, d=128: wanted 32768, step cap 2^31/(1024*128)=16384 -> batch 16384.
        let p = plan(60_000, 1024, 128, &cfg());
        assert_eq!((p.batch, p.mode), (16_384, Mode::EMeans));
        assert_eq!(p.steps, 11); // ceil(3 * 60000 / 16384)
                                 // Small k: batch floors at 1024.
        let p = plan(100_000, 8, 16, &cfg());
        assert_eq!((p.batch, p.steps), (1024, 293));
        // Cap by max_steps.
        let p = plan(100_000_000, 8, 16, &cfg());
        assert_eq!(p.steps, 300);
        // At most one batch runs exact.
        let p = plan(500, 4, 8, &cfg());
        assert_eq!((p.mode, p.batch), (Mode::Exact, 500));
        // The override wins.
        let mut c = cfg();
        c.batch_rows = 2000;
        assert_eq!(plan(100_000, 8, 16, &c).batch, 2000);
        // The forced exact mode.
        c.exact = true;
        assert_eq!(plan(100_000, 8, 16, &c).mode, Mode::Exact);
    }

    // spec: MEDULLA-CLUSTER-R006.1

    #[test]
    fn training_bytes_has_no_k_d_threads_term() {
        let base = training_bytes(65_536, 128, 4096);
        assert_eq!(
            base,
            4 * 65_536 * 128 + 120 * 65_536 + 32_768 + 16 * 4096 + 8 * 128
        );
        // Doubling d adds only the 4kd and 8d terms, never a threads multiple.
        let wide = training_bytes(65_536, 256, 4096);
        assert_eq!(wide - base, 4 * 65_536 * 128 + 8 * 128);
    }
}
