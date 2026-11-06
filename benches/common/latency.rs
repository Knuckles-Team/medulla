//! Shared latency-receipt helpers for the root hot-path benches.

/// The `fraction` percentile of ascending `sorted` samples (0 when empty).
pub fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * fraction).round() as usize]
}

/// The process's resident set in MiB (VmRSS), or NaN off Linux.
pub fn rss_mib() -> f64 {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return f64::NAN;
    };
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))
        .and_then(|rest| rest.trim().strip_suffix(" kB"))
        .and_then(|kilobytes| kilobytes.parse::<f64>().ok())
        .map(|kilobytes| kilobytes / 1024.0)
        .unwrap_or(f64::NAN)
}
