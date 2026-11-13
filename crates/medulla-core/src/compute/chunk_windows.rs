//! Independent document-chunk encoding: window splitting, one encoder call per
//! window, and `mean_chunks` aggregation (MEDULLA-FEDERATED-QUERY-R039.2.2,
//! R039.2.3, R039.2.4).
//!
//! Every function here is pure. The window size and overlap are registry knobs
//! (`retrieval.encode_window_tokens`, `retrieval.encode_window_overlap`) read by
//! the caller (the semantic index worker) and passed in; this module never
//! reads configuration itself.

use medulla_types::contract::ServerErrorCode;

/// How token positions are presented to the encoder for each window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionMode {
    /// Positions restart at zero in every window.
    LocalReset,
    /// Positions keep their offset in the whole document.
    Absolute,
}

impl PositionMode {
    /// The wire name of the mode.
    pub fn as_str(self) -> &'static str {
        match self {
            PositionMode::LocalReset => "local_reset",
            PositionMode::Absolute => "absolute",
        }
    }

    /// Parse a wire name; any other value is `ENCODE_POSITION_MODE_UNSUPPORTED`.
    pub fn parse(raw: &str) -> Result<Self, ChunkRefusal> {
        match raw {
            "local_reset" => Ok(PositionMode::LocalReset),
            "absolute" => Ok(PositionMode::Absolute),
            other => Err(ChunkRefusal::new(
                ServerErrorCode::EncodePositionModeUnsupported,
                format!("position mode `{other}` is not served"),
            )),
        }
    }
}

/// A typed refusal of the chunk-encoding steps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkRefusal {
    code: ServerErrorCode,
    detail: String,
}

impl ChunkRefusal {
    fn new(code: ServerErrorCode, detail: String) -> Self {
        Self { code, detail }
    }

    /// The registered refusal code.
    pub fn code(&self) -> ServerErrorCode {
        self.code
    }

    /// The code text, e.g. `ENCODE_OVERLAP_INVALID`.
    pub fn code_str(&self) -> &'static str {
        self.code.as_str()
    }
}

impl std::fmt::Display for ChunkRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.detail)
    }
}

impl std::error::Error for ChunkRefusal {}

/// One window of a document: the half-open token range it covers and the
/// position of its first token as the encoder sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    /// Ordinal of the window, from zero.
    pub ordinal: usize,
    /// First token index (inclusive).
    pub start: usize,
    /// One past the last token index.
    pub end: usize,
    /// Position of the first token: `0` for `local_reset`, `start` for `absolute`.
    pub position_base: usize,
}

/// Cut `token_count` token ids into ordered, overlapping windows without
/// losing a token. The stride is `size - overlap`; the final window may be
/// partial. An empty document has no windows.
///
/// Refuses `ENCODE_OVERLAP_INVALID` when `size` is zero or `overlap >= size`.
pub fn split_windows(
    token_count: usize,
    size: usize,
    overlap: usize,
    mode: PositionMode,
) -> Result<Vec<Window>, ChunkRefusal> {
    if size == 0 || overlap >= size {
        return Err(ChunkRefusal::new(
            ServerErrorCode::EncodeOverlapInvalid,
            format!("overlap {overlap} must be below window size {size}"),
        ));
    }
    let stride = size - overlap;
    let mut windows = Vec::new();
    let mut start = 0usize;
    while start < token_count {
        let end = (start + size).min(token_count);
        windows.push(Window {
            ordinal: windows.len(),
            start,
            end,
            position_base: match mode {
                PositionMode::LocalReset => 0,
                PositionMode::Absolute => start,
            },
        });
        if end == token_count {
            break;
        }
        start += stride;
    }
    Ok(windows)
}

/// An encoder of one window of token ids. It holds no mutable state shared
/// between calls: the same window always gives the same vector.
pub trait ChunkEncoder {
    /// Encode one window with positions restarting at zero.
    fn encode(&self, window: &[u32]) -> Vec<f32>;

    /// Encode one window whose first token sits at `position_base`. Encoders
    /// that do not read positions keep this default.
    fn encode_at(&self, window: &[u32], _position_base: usize) -> Vec<f32> {
        self.encode(window)
    }
}

/// `encode_calls_total{model}` of the invocation record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodeCalls {
    /// The pinned model the calls went to.
    pub model: String,
    /// Calls made: exactly one per window.
    pub total: u64,
}

/// The per-window vectors of one document and the call counter.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkEncoding {
    /// One vector per window, in ordinal order.
    pub vectors: Vec<Vec<f32>>,
    /// The call counter for the invocation record.
    pub calls: EncodeCalls,
}

/// Encode every window of `tokens` with exactly one call to `encoder` each, in
/// ordinal order, with no state carried from one call to the next.
pub fn encode_chunks<E: ChunkEncoder + ?Sized>(
    encoder: &E,
    model: &str,
    tokens: &[u32],
    windows: &[Window],
) -> ChunkEncoding {
    let mut calls = 0u64;
    let vectors = windows
        .iter()
        .map(|w| {
            calls += 1;
            encoder.encode_at(&tokens[w.start..w.end], w.position_base)
        })
        .collect();
    ChunkEncoding {
        vectors,
        calls: EncodeCalls {
            model: model.to_string(),
            total: calls,
        },
    }
}

/// `mean_chunks`: average the chunk vectors in ordinal order (accumulating in
/// `f64`), then apply one L2 normalization. The result is one document vector
/// independent of any query.
///
/// Refusals: `NO_EMBEDDING` for no chunks or a zero-norm mean,
/// `ENCODE_DIM_MISMATCH` when chunk dimensions differ (or are zero),
/// `ENCODE_NON_FINITE` for a NaN or infinite component.
pub fn aggregate_mean_chunks(vectors: &[Vec<f32>]) -> Result<Vec<f32>, ChunkRefusal> {
    let first = vectors.first().ok_or_else(|| {
        ChunkRefusal::new(ServerErrorCode::NoEmbedding, "no chunk vectors".to_string())
    })?;
    let dim = first.len();
    if dim == 0 {
        return Err(ChunkRefusal::new(
            ServerErrorCode::NoEmbedding,
            "chunk vector has no components".to_string(),
        ));
    }
    let mut sum = vec![0f64; dim];
    for (ordinal, v) in vectors.iter().enumerate() {
        if v.len() != dim {
            return Err(ChunkRefusal::new(
                ServerErrorCode::EncodeDimMismatch,
                format!("chunk {ordinal} has {} components, expected {dim}", v.len()),
            ));
        }
        for (acc, c) in sum.iter_mut().zip(v) {
            if !c.is_finite() {
                return Err(ChunkRefusal::new(
                    ServerErrorCode::EncodeNonFinite,
                    format!("chunk {ordinal} has a non-finite component"),
                ));
            }
            *acc += f64::from(*c);
        }
    }
    let n = vectors.len() as f64;
    for acc in &mut sum {
        *acc /= n;
    }
    let norm = sum.iter().map(|c| c * c).sum::<f64>().sqrt();
    if norm == 0.0 || !norm.is_finite() {
        return Err(ChunkRefusal::new(
            ServerErrorCode::NoEmbedding,
            "mean vector has zero norm".to_string(),
        ));
    }
    Ok(sum.into_iter().map(|c| (c / norm) as f32).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn spans(w: &[Window]) -> Vec<(usize, usize, usize)> {
        w.iter()
            .map(|w| (w.start, w.end, w.position_base))
            .collect()
    }

    // spec: MEDULLA-FEDERATED-QUERY-R039.2.2
    #[test]
    fn golden_boundaries() {
        use PositionMode::{Absolute, LocalReset};
        // empty
        assert!(split_windows(0, 4, 1, LocalReset).unwrap().is_empty());
        // short: one partial window
        assert_eq!(
            spans(&split_windows(3, 4, 1, LocalReset).unwrap()),
            [(0, 3, 0)]
        );
        // exact size: one window, no trailing duplicate
        assert_eq!(
            spans(&split_windows(4, 4, 1, LocalReset).unwrap()),
            [(0, 4, 0)]
        );
        // overlapping, stride 3, ending exactly on a boundary
        assert_eq!(
            spans(&split_windows(10, 4, 1, LocalReset).unwrap()),
            [(0, 4, 0), (3, 7, 0), (6, 10, 0)]
        );
        // partial final window, absolute positions keep document offsets
        assert_eq!(
            spans(&split_windows(9, 4, 2, Absolute).unwrap()),
            [(0, 4, 0), (2, 6, 2), (4, 8, 4), (6, 9, 6)]
        );
        // zero overlap tiles the document
        assert_eq!(
            spans(&split_windows(8, 4, 0, LocalReset).unwrap()),
            [(0, 4, 0), (4, 8, 0)]
        );
        // no token is lost for any shape
        for n in 0..40 {
            for size in 1..8 {
                for overlap in 0..size {
                    let w = split_windows(n, size, overlap, LocalReset).unwrap();
                    let mut covered = vec![false; n];
                    for win in &w {
                        covered[win.start..win.end]
                            .iter_mut()
                            .for_each(|c| *c = true);
                    }
                    assert!(
                        covered.iter().all(|c| *c),
                        "n={n} size={size} overlap={overlap}"
                    );
                    assert!(w.iter().enumerate().all(|(i, win)| win.ordinal == i));
                }
            }
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R039.2.2
    #[test]
    fn invalid_overlap_and_position_mode_are_refused_with_codes() {
        for (size, overlap) in [(4, 4), (4, 9), (0, 0)] {
            let e = split_windows(10, size, overlap, PositionMode::LocalReset).unwrap_err();
            assert_eq!(e.code_str(), "ENCODE_OVERLAP_INVALID");
            assert!(e.to_string().starts_with("ENCODE_OVERLAP_INVALID: "));
        }
        assert_eq!(
            PositionMode::parse("local_reset").unwrap(),
            PositionMode::LocalReset
        );
        assert_eq!(
            PositionMode::parse("absolute").unwrap(),
            PositionMode::Absolute
        );
        let e = PositionMode::parse("rotary").unwrap_err();
        assert_eq!(e.code_str(), "ENCODE_POSITION_MODE_UNSUPPORTED");
    }

    /// Pure test encoder: a function of the window alone.
    struct SumEncoder;
    impl ChunkEncoder for SumEncoder {
        fn encode(&self, window: &[u32]) -> Vec<f32> {
            vec![window.iter().sum::<u32>() as f32, window.len() as f32]
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R039.2.3
    #[test]
    fn one_call_per_chunk_no_shared_state() {
        struct Recording(RefCell<Vec<(Vec<u32>, usize)>>);
        impl ChunkEncoder for Recording {
            fn encode(&self, w: &[u32]) -> Vec<f32> {
                self.encode_at(w, 0)
            }
            fn encode_at(&self, w: &[u32], base: usize) -> Vec<f32> {
                self.0.borrow_mut().push((w.to_vec(), base));
                SumEncoder.encode(w)
            }
        }
        let tokens: Vec<u32> = (1..=9).collect();
        let windows = split_windows(9, 4, 2, PositionMode::Absolute).unwrap();
        let rec = Recording(RefCell::new(Vec::new()));
        let out = encode_chunks(&rec, "tiny-test-encoder@1", &tokens, &windows);
        // exactly one call per window, each with its own slice and position base
        let seen = rec.0.borrow();
        assert_eq!(seen.len(), windows.len());
        assert_eq!(out.calls.total, windows.len() as u64);
        assert_eq!(out.calls.model, "tiny-test-encoder@1");
        for (w, (slice, base)) in windows.iter().zip(seen.iter()) {
            assert_eq!(slice.as_slice(), &tokens[w.start..w.end]);
            assert_eq!(*base, w.position_base);
        }
        // no state shared between calls: every vector equals a fresh, isolated call
        for (w, v) in windows.iter().zip(&out.vectors) {
            assert_eq!(v, &SumEncoder.encode(&tokens[w.start..w.end]));
        }
        // and a different call order gives the same vectors
        let mut reversed: Vec<Vec<f32>> = windows
            .iter()
            .rev()
            .map(|w| SumEncoder.encode(&tokens[w.start..w.end]))
            .collect();
        reversed.reverse();
        assert_eq!(out.vectors, reversed);
        // an empty document makes no call
        let none = encode_chunks(&SumEncoder, "m", &[], &[]);
        assert!(none.vectors.is_empty());
        assert_eq!(none.calls.total, 0);
    }

    // spec: MEDULLA-FEDERATED-QUERY-R039.2.4
    #[test]
    fn mean_matches_numeric_oracle() {
        // Hand-computed oracle: mean of [3,4] and [1,0] is [2,2]; its L2 norm is
        // sqrt(8), so the document vector is [1/sqrt(2), 1/sqrt(2)].
        let h = 0.707_106_781_186_547_6_f64;
        let v = aggregate_mean_chunks(&[vec![3.0, 4.0], vec![1.0, 0.0]]).unwrap();
        assert!(v.iter().all(|c| (f64::from(*c) - h).abs() <= 1e-6), "{v:?}");
        // a single chunk is its own normalised direction: [3,4] -> [0.6,0.8]
        let v = aggregate_mean_chunks(&[vec![3.0, 4.0]]).unwrap();
        assert!((f64::from(v[0]) - 0.6).abs() <= 1e-6 && (f64::from(v[1]) - 0.8).abs() <= 1e-6);
        // unit norm
        let v = aggregate_mean_chunks(&[vec![1.0, 2.0, 3.0], vec![-4.0, 0.5, 9.0]]).unwrap();
        let n: f64 = v.iter().map(|c| f64::from(*c).powi(2)).sum();
        assert!((n - 1.0).abs() <= 1e-6);
        // order of the mean is not hidden state: the same inputs give the same bytes
        assert_eq!(
            aggregate_mean_chunks(&[vec![3.0, 4.0], vec![1.0, 0.0]]).unwrap(),
            aggregate_mean_chunks(&[vec![3.0, 4.0], vec![1.0, 0.0]]).unwrap()
        );
    }

    // spec: MEDULLA-FEDERATED-QUERY-R039.2.4
    #[test]
    fn aggregate_refuses_empty_zero_norm_mismatch_and_non_finite() {
        assert_eq!(
            aggregate_mean_chunks(&[]).unwrap_err().code_str(),
            "NO_EMBEDDING"
        );
        assert_eq!(
            aggregate_mean_chunks(&[vec![]]).unwrap_err().code_str(),
            "NO_EMBEDDING"
        );
        assert_eq!(
            aggregate_mean_chunks(&[vec![1.0, -1.0], vec![-1.0, 1.0]])
                .unwrap_err()
                .code_str(),
            "NO_EMBEDDING"
        );
        assert_eq!(
            aggregate_mean_chunks(&[vec![1.0, 2.0], vec![1.0]])
                .unwrap_err()
                .code_str(),
            "ENCODE_DIM_MISMATCH"
        );
        assert_eq!(
            aggregate_mean_chunks(&[vec![1.0, f32::NAN]])
                .unwrap_err()
                .code_str(),
            "ENCODE_NON_FINITE"
        );
        assert_eq!(
            aggregate_mean_chunks(&[vec![f32::INFINITY, 0.0]])
                .unwrap_err()
                .code_str(),
            "ENCODE_NON_FINITE"
        );
    }
}
