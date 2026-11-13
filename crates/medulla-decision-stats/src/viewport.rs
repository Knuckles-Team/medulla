//! Viewport binning kernels (MEDULLA-FEDERATED-QUERY-R081.103.2).
//!
//! `bin_scatter` and `bin_line` fold any number of points into a bounded set of
//! bins. Scatter bins are the non-empty cells of a uniform grid over the viewport
//! window; line bins are the non-empty pixel columns. A bin set never truncates
//! silently: a result past its cap is refused.

use std::collections::BTreeMap;

use medulla_types::viewport::{LineBin, ScatterBin, ViewportDomain, ViewportError};

/// Most scatter bins any response carries.
pub const SCATTER_BIN_CAP: u64 = 262_144;
/// Most line bins any response carries.
pub const LINE_BIN_CAP: u64 = 65_536;
/// The response byte cap before the per-bin term.
pub const RESPONSE_BYTES_BASE: u64 = 65_536;
/// Bytes each bin adds to the response byte cap.
pub const RESPONSE_BYTES_PER_BIN: u64 = 96;
/// The largest response byte cap.
pub const RESPONSE_BYTES_CAP: u64 = 16_777_216;
/// The smallest ceiling that can hold the result envelope.
pub const MIN_ENVELOPE_BYTES: u64 = 256;

/// `min(pixel_width * pixel_height, 262144)`.
pub fn scatter_max_bins(pixel_width: u32, pixel_height: u32) -> u64 {
    (u64::from(pixel_width) * u64::from(pixel_height)).min(SCATTER_BIN_CAP)
}

/// `min(4 * pixel_width, 65536)`.
pub fn line_max_bins(pixel_width: u32) -> u64 {
    (4 * u64::from(pixel_width)).min(LINE_BIN_CAP)
}

/// `min(16777216, 65536 + 96 * max_bin_count)`.
pub fn response_byte_cap(max_bins: u64) -> u64 {
    RESPONSE_BYTES_CAP.min(RESPONSE_BYTES_BASE + RESPONSE_BYTES_PER_BIN * max_bins)
}

/// The effective ceiling, `min(byte_ceiling, response cap)`. A ceiling below the envelope
/// refuses before any scan.
pub fn effective_ceiling(byte_ceiling: u64, max_bins: u64) -> Result<u64, ViewportError> {
    if byte_ceiling < MIN_ENVELOPE_BYTES {
        return Err(ViewportError::ByteCeilingTooSmall {
            ceiling: byte_ceiling,
            minimum: MIN_ENVELOPE_BYTES,
        });
    }
    Ok(byte_ceiling.min(response_byte_cap(max_bins)))
}

/// The scatter cell of a point, or `None` outside the window. The upper edges belong to the
/// last cell.
pub fn scatter_cell(
    x: f64,
    y: f64,
    domain: &ViewportDomain,
    pixel_width: u32,
    pixel_height: u32,
) -> Option<u32> {
    if !x.is_finite()
        || !y.is_finite()
        || x < domain.x_min
        || x > domain.x_max
        || y < domain.y_min
        || y > domain.y_max
    {
        return None;
    }
    let cell = |v: f64, lo: f64, hi: f64, n: u32| -> u32 {
        if hi <= lo {
            return 0;
        }
        let at = ((v - lo) * f64::from(n) / (hi - lo)).floor();
        (at as i64).clamp(0, i64::from(n) - 1) as u32
    };
    let col = cell(x, domain.x_min, domain.x_max, pixel_width);
    let row = cell(y, domain.y_min, domain.y_max, pixel_height);
    Some(row * pixel_width + col)
}

/// The pixel column of a timestamp, or `None` outside `[x_min, x_max]`. Integer arithmetic.
pub fn line_column(ts: i64, x_min: i64, x_max: i64, pixel_width: u32) -> Option<u32> {
    if ts < x_min || ts > x_max || x_max <= x_min {
        return None;
    }
    let span = i128::from(x_max) - i128::from(x_min);
    let at = (i128::from(ts) - i128::from(x_min)) * i128::from(pixel_width) / span;
    Some(at.min(i128::from(pixel_width) - 1) as u32)
}

/// Fold `(x, y)` points into the non-empty cells of the viewport grid.
pub fn bin_scatter(
    points: impl Iterator<Item = (f64, f64)>,
    domain: &ViewportDomain,
    pixel_width: u32,
    pixel_height: u32,
) -> Result<Vec<ScatterBin>, ViewportError> {
    let max_bins = scatter_max_bins(pixel_width, pixel_height);
    let mut cells: BTreeMap<u32, ScatterBin> = BTreeMap::new();
    for (x, y) in points {
        let Some(id) = scatter_cell(x, y, domain, pixel_width, pixel_height) else {
            continue;
        };
        let bin = cells.entry(id).or_insert(ScatterBin {
            id,
            count: 0,
            x_min: x,
            x_max: x,
            y_min: y,
            y_max: y,
        });
        bin.count += 1;
        bin.x_min = bin.x_min.min(x);
        bin.x_max = bin.x_max.max(x);
        bin.y_min = bin.y_min.min(y);
        bin.y_max = bin.y_max.max(y);
        if cells.len() as u64 > max_bins {
            return Err(ViewportError::CapExceeded(format!(
                "more than {max_bins} non-empty scatter cells"
            )));
        }
    }
    Ok(cells.into_values().collect())
}

struct Column {
    first: (i64, f64),
    last: (i64, f64),
    min: f64,
    max: f64,
    count: u64,
}

/// A stored block of a series folded to a summary (MEDULLA-FEDERATED-QUERY-R081.103.5):
/// the finite samples of one field in a run of timestamps that lies inside one pixel column.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineBlock {
    /// Earliest timestamp of the block.
    pub ts_min: i64,
    /// Latest timestamp of the block.
    pub ts_max: i64,
    /// Finite samples in the block.
    pub count: u64,
    /// Earliest finite sample.
    pub first: (i64, f64),
    /// Latest finite sample.
    pub last: (i64, f64),
    /// Smallest finite value.
    pub min: f64,
    /// Largest finite value.
    pub max: f64,
}

/// One input of a line fold: a sample, or a block already summarized.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LineInput {
    /// One `(ts, value)` sample.
    Point(i64, f64),
    /// A summarized block that lies inside one pixel column.
    Block(LineBlock),
}

/// Fold `(ts, value)` points into the non-empty pixel columns of `[x_min, x_max]`.
pub fn bin_line(
    points: impl Iterator<Item = (i64, f64)>,
    x_min: i64,
    x_max: i64,
    pixel_width: u32,
) -> Result<Vec<LineBin>, ViewportError> {
    bin_line_blocks(
        points.map(|(ts, v)| LineInput::Point(ts, v)),
        x_min,
        x_max,
        pixel_width,
    )
}

/// Fold samples and summarized blocks into the non-empty pixel columns of `[x_min, x_max]`.
/// A block counts as one run inside the column of its earliest timestamp; a block that is
/// not wholly inside one column of the window is refused, because folding it would move
/// samples between columns.
pub fn bin_line_blocks(
    inputs: impl Iterator<Item = LineInput>,
    x_min: i64,
    x_max: i64,
    pixel_width: u32,
) -> Result<Vec<LineBin>, ViewportError> {
    let max_bins = line_max_bins(pixel_width);
    let mut columns: BTreeMap<u32, Column> = BTreeMap::new();
    for input in inputs {
        let (col, run) = match input {
            LineInput::Point(ts, value) => {
                if !value.is_finite() {
                    continue;
                }
                let Some(col) = line_column(ts, x_min, x_max, pixel_width) else {
                    continue;
                };
                (
                    col,
                    Column {
                        first: (ts, value),
                        last: (ts, value),
                        min: value,
                        max: value,
                        count: 1,
                    },
                )
            }
            LineInput::Block(block) => {
                if block.count == 0 {
                    continue;
                }
                let lo = line_column(block.ts_min, x_min, x_max, pixel_width);
                let hi = line_column(block.ts_max, x_min, x_max, pixel_width);
                let (Some(col), true) = (lo, lo == hi) else {
                    return Err(ViewportError::Invalid(
                        "a summarized block does not lie inside one pixel column".into(),
                    ));
                };
                (
                    col,
                    Column {
                        first: block.first,
                        last: block.last,
                        min: block.min,
                        max: block.max,
                        count: block.count,
                    },
                )
            }
        };
        match columns.get_mut(&col) {
            None => {
                columns.insert(col, run);
            }
            Some(c) => {
                c.count += run.count;
                c.min = c.min.min(run.min);
                c.max = c.max.max(run.max);
                if run.first.0 < c.first.0 {
                    c.first = run.first;
                }
                if run.last.0 >= c.last.0 {
                    c.last = run.last;
                }
            }
        }
    }
    if columns.len() as u64 > max_bins {
        return Err(ViewportError::CapExceeded(format!(
            "more than {max_bins} line columns"
        )));
    }
    let mut previous: Option<u32> = None;
    Ok(columns
        .into_iter()
        .map(|(col, c)| {
            let gap_before = previous.is_some_and(|p| col > p + 1);
            previous = Some(col);
            LineBin {
                col,
                first: c.first.1,
                last: c.last.1,
                min: c.min,
                max: c.max,
                count: c.count,
                gap_before,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn domain() -> ViewportDomain {
        ViewportDomain {
            x_min: 0.0,
            x_max: 100.0,
            y_min: 0.0,
            y_max: 100.0,
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R081.103.2
    #[test]
    fn bins_match_oracle_and_caps_hold() {
        // The three cap formulas.
        assert_eq!(scatter_max_bins(10, 10), 100);
        assert_eq!(scatter_max_bins(4096, 4096), 262_144);
        assert_eq!(line_max_bins(100), 400);
        assert_eq!(line_max_bins(4096), 16_384);
        assert_eq!(response_byte_cap(100), 65_536 + 9_600);
        assert_eq!(response_byte_cap(262_144), 16_777_216);
        assert_eq!(effective_ceiling(1_000_000, 100), Ok(65_536 + 9_600));
        assert_eq!(effective_ceiling(70_000, 100), Ok(70_000));
        assert_eq!(
            effective_ceiling(10, 100),
            Err(ViewportError::ByteCeilingTooSmall {
                ceiling: 10,
                minimum: MIN_ENVELOPE_BYTES
            })
        );

        // Scatter against an exhaustive per-cell oracle on a small integer source.
        let points: Vec<(f64, f64)> = (0..500)
            .map(|i| (((i * 37) % 101) as f64, ((i * 53) % 103) as f64))
            .collect();
        let (w, h) = (10u32, 5u32);
        let bins = bin_scatter(points.iter().copied(), &domain(), w, h).unwrap();
        let mut oracle = Vec::new();
        for row in 0..h {
            for col in 0..w {
                let (x0, x1) = (f64::from(col) * 10.0, f64::from(col + 1) * 10.0);
                let (y0, y1) = (f64::from(row) * 20.0, f64::from(row + 1) * 20.0);
                let inside: Vec<&(f64, f64)> = points
                    .iter()
                    .filter(|(x, y)| {
                        let xin = *x >= x0 && (*x < x1 || (col + 1 == w && *x <= x1));
                        let yin = *y >= y0 && (*y < y1 || (row + 1 == h && *y <= y1));
                        xin && yin && *x <= 100.0 && *y <= 100.0
                    })
                    .collect();
                if inside.is_empty() {
                    continue;
                }
                let fold = |f: fn(f64, f64) -> f64, pick: fn(&(f64, f64)) -> f64, seed: f64| {
                    inside.iter().fold(seed, |a, p| f(a, pick(p)))
                };
                oracle.push(ScatterBin {
                    id: row * w + col,
                    count: inside.len() as u64,
                    x_min: fold(f64::min, |p| p.0, f64::INFINITY),
                    x_max: fold(f64::max, |p| p.0, f64::NEG_INFINITY),
                    y_min: fold(f64::min, |p| p.1, f64::INFINITY),
                    y_max: fold(f64::max, |p| p.1, f64::NEG_INFINITY),
                });
            }
        }
        assert_eq!(bins, oracle);
        assert_eq!(bins.iter().map(|b| b.count).sum::<u64>(), 500);

        // Points outside the window are not binned; upper edges fall in the last cell.
        let edge = bin_scatter(
            [(100.0, 100.0), (-1.0, 5.0), (5.0, 101.0), (f64::NAN, 1.0)].into_iter(),
            &domain(),
            w,
            h,
        )
        .unwrap();
        assert_eq!(edge.len(), 1);
        assert_eq!(edge[0].id, h * w - 1);

        // Scatter cap overflow refuses; it never truncates. A 4096 by 4096 grid has more
        // cells than the 262144 cap, so 300,000 distinct cells overflow it.
        let huge = ViewportDomain {
            x_min: 0.0,
            x_max: 4096.0,
            y_min: 0.0,
            y_max: 4096.0,
        };
        let dense = (0..4096u32)
            .flat_map(|i| (0..74u32).map(move |j| (f64::from(i) + 0.5, f64::from(j) + 0.5)));
        assert_eq!(
            bin_scatter(dense.clone(), &huge, 4096, 4096)
                .unwrap_err()
                .code(),
            "VIEWPORT_CAP_EXCEEDED"
        );
        let within = dense.take(262_144);
        assert_eq!(
            bin_scatter(within, &huge, 4096, 4096).unwrap().len(),
            262_144
        );

        // Line against an oracle: 1000 points, 8 columns, a gap in columns 3 and 4.
        let line: Vec<(i64, f64)> = (0..1000i64)
            .filter(|t| !(375..625).contains(t))
            .map(|t| (t, ((t * 7) % 31) as f64))
            .collect();
        let bins = bin_line(line.iter().copied(), 0, 999, 8).unwrap();
        let mut want = Vec::new();
        let mut previous: Option<u32> = None;
        for col in 0..8u32 {
            let inside: Vec<&(i64, f64)> = line
                .iter()
                .filter(|(t, _)| (i128::from(*t) * 8 / 999) as u32 == col)
                .collect();
            if inside.is_empty() {
                continue;
            }
            want.push(LineBin {
                col,
                first: inside.first().unwrap().1,
                last: inside.last().unwrap().1,
                min: inside.iter().map(|p| p.1).fold(f64::INFINITY, f64::min),
                max: inside.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max),
                count: inside.len() as u64,
                gap_before: previous.is_some_and(|p| col > p + 1),
            });
            previous = Some(col);
        }
        assert_eq!(bins, want);
        assert!(bins.iter().any(|b| b.gap_before));
        assert!(bins.len() as u64 <= line_max_bins(8));
        assert_eq!(bin_line(line.iter().copied(), 0, 999, 8).unwrap(), bins);
        // The last timestamp lands in the last column; points outside are dropped.
        assert_eq!(line_column(999, 0, 999, 8), Some(7));
        assert_eq!(line_column(1000, 0, 999, 8), None);
    }

    // spec: MEDULLA-FEDERATED-QUERY-R081.103.5
    #[test]
    fn block_fold_equals_point_fold() {
        let points: Vec<(i64, f64)> = (0..100i64).map(|t| (t, ((t * 37) % 11) as f64)).collect();
        let block_of = |lo: usize, hi: usize| {
            let run = &points[lo..hi];
            LineInput::Block(LineBlock {
                ts_min: run[0].0,
                ts_max: run[run.len() - 1].0,
                count: run.len() as u64,
                first: run[0],
                last: run[run.len() - 1],
                min: run.iter().map(|p| p.1).fold(f64::INFINITY, f64::min),
                max: run.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max),
            })
        };
        // Ten columns of ten; the blocks 0..10 and 20..30 fold whole, the rest are points.
        let mut inputs = vec![block_of(0, 10)];
        inputs.extend(points[10..20].iter().map(|p| LineInput::Point(p.0, p.1)));
        inputs.push(block_of(20, 30));
        inputs.extend(points[30..].iter().map(|p| LineInput::Point(p.0, p.1)));
        let folded = bin_line_blocks(inputs.into_iter(), 0, 99, 10).unwrap();
        let expected = bin_line(points.iter().copied(), 0, 99, 10).unwrap();
        assert_eq!(folded, expected);
        // A block that straddles two columns is refused.
        let straddling = bin_line_blocks([block_of(5, 15)].into_iter(), 0, 99, 10);
        assert!(straddling.is_err());
    }
}
