//! Viewport query wire types (MEDULLA-FEDERATED-QUERY-R081.103.1).
//!
//! A viewport query returns a bounded set of bins whatever the size of the source.
//! The request, result, selection token and typed refusals of the served
//! `ViewportQuery` and `ResolveViewportSelection` reads live here, below the protocol
//! crate, because `Method` carries them. The binning kernel is
//! `medulla_decision_stats::viewport`; the handler is `medulla-h-query`.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

/// Largest pixel width or height of a viewport.
pub const MAX_PIXELS: u32 = 4096;
/// Largest byte ceiling a request may carry.
pub const MAX_BYTE_CEILING: u64 = 16_777_216;
/// The one binning formula revision this engine serves.
pub const BINNING_REVISION: u32 = 1;
/// Most raw points one selection resolve returns.
pub const MAX_RESOLVED_POINTS: usize = 1000;
/// The x axis name of a line query: the series timestamp in nanoseconds.
pub const LINE_X_AXIS: &str = "ts";

/// The shape of a viewport query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewportKind {
    /// A count grid with per-cell extrema over two numeric fields (`point_table`).
    Scatter,
    /// One bin per pixel column over a time series (`tsdb_series`).
    Line,
}

/// The visible data window. A line query reads `x_min` and `x_max` as nanosecond
/// timestamps; the y range only clips a scatter query.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ViewportDomain {
    /// Lower x bound, inclusive.
    pub x_min: f64,
    /// Upper x bound, inclusive.
    pub x_max: f64,
    /// Lower y bound, inclusive.
    pub y_min: f64,
    /// Upper y bound, inclusive.
    pub y_max: f64,
}

/// A served `ViewportQuery` request, also the pinned window of a selection resolve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ViewportRequest {
    /// The indexed source: the name of a scoped time series.
    pub source_id: String,
    /// The source revision to read at; `None` reads the current one and pins it in the result.
    #[serde(default)]
    pub source_revision: Option<u64>,
    /// Scatter or line.
    pub kind: ViewportKind,
    /// Scatter: the x field name. Line: `ts`.
    pub x_axis: String,
    /// The y field name.
    pub y_axis: String,
    /// The display units of the axes, echoed for the consumer.
    #[serde(default)]
    pub units: String,
    /// Digest of the filter the caller applied upstream; part of the query identity.
    #[serde(default)]
    pub filter_digest: String,
    /// The visible window.
    pub domain: ViewportDomain,
    /// Viewport width in pixels, 1 to 4096.
    pub pixel_width: u32,
    /// Viewport height in pixels, 1 to 4096.
    pub pixel_height: u32,
    /// The binning formula revision; must equal [`BINNING_REVISION`].
    pub binning_revision: u32,
    /// The response byte ceiling, 1 to 16777216.
    pub byte_ceiling: u64,
}

impl ViewportRequest {
    /// Check every bound of the request.
    pub fn validate(&self) -> Result<(), ViewportError> {
        let invalid = |detail: &str| Err(ViewportError::Invalid(detail.to_string()));
        if self.source_id.is_empty() || self.x_axis.is_empty() || self.y_axis.is_empty() {
            return invalid("source_id, x_axis and y_axis are required");
        }
        if !(1..=MAX_PIXELS).contains(&self.pixel_width)
            || !(1..=MAX_PIXELS).contains(&self.pixel_height)
        {
            return invalid("pixel_width and pixel_height must be in 1..=4096");
        }
        if !(1..=MAX_BYTE_CEILING).contains(&self.byte_ceiling) {
            return invalid("byte_ceiling must be in 1..=16777216");
        }
        if self.binning_revision != BINNING_REVISION {
            return invalid("binning_revision is not served");
        }
        let d = &self.domain;
        let finite = [d.x_min, d.x_max, d.y_min, d.y_max]
            .iter()
            .all(|v| v.is_finite());
        if !finite || d.x_min >= d.x_max || d.y_min > d.y_max {
            return invalid("domain must be finite with x_min < x_max and y_min <= y_max");
        }
        if self.kind == ViewportKind::Line && self.x_axis != LINE_X_AXIS {
            return invalid("a line query reads x_axis `ts`");
        }
        Ok(())
    }

    /// Digest of the viewport identity: kind, source, axes, units, filter, window, pixels and
    /// formula. It leaves out the pinned revision and the byte ceiling, so a pan or a zoom
    /// changes it and a repeat of the same view does not.
    pub fn query_digest(&self) -> String {
        let identity = (
            self.kind,
            &self.source_id,
            &self.x_axis,
            &self.y_axis,
            &self.units,
            &self.filter_digest,
            [
                self.domain.x_min.to_bits(),
                self.domain.x_max.to_bits(),
                self.domain.y_min.to_bits(),
                self.domain.y_max.to_bits(),
            ],
            self.pixel_width,
            self.pixel_height,
            self.binning_revision,
        );
        let bytes = serde_json::to_vec(&identity).unwrap_or_default();
        hex::encode(Sha256::digest(bytes))
    }
}

/// One non-empty cell of a scatter grid. Serialized as a six-element array.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScatterBin {
    /// Stable id: `row * pixel_width + column` of the cell.
    pub id: u32,
    /// Points in the cell.
    pub count: u64,
    /// Smallest x in the cell.
    pub x_min: f64,
    /// Largest x in the cell.
    pub x_max: f64,
    /// Smallest y in the cell.
    pub y_min: f64,
    /// Largest y in the cell.
    pub y_max: f64,
}

impl Serialize for ScatterBin {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        (
            self.id, self.count, self.x_min, self.x_max, self.y_min, self.y_max,
        )
            .serialize(s)
    }
}

impl<'de> Deserialize<'de> for ScatterBin {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let (id, count, x_min, x_max, y_min, y_max) =
            <(u32, u64, f64, f64, f64, f64)>::deserialize(d)?;
        Ok(Self {
            id,
            count,
            x_min,
            x_max,
            y_min,
            y_max,
        })
    }
}

/// One non-empty pixel column of a line. Serialized as a seven-element array.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineBin {
    /// The pixel column, also the bin id.
    pub col: u32,
    /// The value of the earliest point in the column.
    pub first: f64,
    /// The value of the latest point in the column.
    pub last: f64,
    /// Smallest value in the column.
    pub min: f64,
    /// Largest value in the column.
    pub max: f64,
    /// Points in the column.
    pub count: u64,
    /// True when at least one empty column lies between this one and the previous bin.
    pub gap_before: bool,
}

impl Serialize for LineBin {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        (
            self.col,
            self.first,
            self.last,
            self.min,
            self.max,
            self.count,
            self.gap_before,
        )
            .serialize(s)
    }
}

impl<'de> Deserialize<'de> for LineBin {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let (col, first, last, min, max, count, gap_before) =
            <(u32, f64, f64, f64, f64, u64, bool)>::deserialize(d)?;
        Ok(Self {
            col,
            first,
            last,
            min,
            max,
            count,
            gap_before,
        })
    }
}

/// The bins of one viewport, by kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "bins", rename_all = "snake_case")]
pub enum ViewportBins {
    /// Non-empty scatter cells in id order.
    Scatter(Vec<ScatterBin>),
    /// Non-empty line columns in column order.
    Line(Vec<LineBin>),
}

impl ViewportBins {
    /// Number of bins.
    pub fn len(&self) -> usize {
        match self {
            Self::Scatter(bins) => bins.len(),
            Self::Line(bins) => bins.len(),
        }
    }

    /// True when no bin holds a point.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The window and pixels a result was binned for.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ViewportSpec {
    /// The visible window.
    pub domain: ViewportDomain,
    /// Viewport width in pixels.
    pub pixel_width: u32,
    /// Viewport height in pixels.
    pub pixel_height: u32,
}

/// The answer of a `ViewportQuery`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ViewportResult {
    /// The bins.
    pub bins: ViewportBins,
    /// The source revision the bins were read at.
    pub source_revision: u64,
    /// The window and pixels.
    pub viewport: ViewportSpec,
    /// The binning formula revision.
    pub binning_revision: u32,
    /// Points the source holds in total, whatever the window.
    pub source_cardinality: u64,
    /// Digest of the viewport identity ([`ViewportRequest::query_digest`]).
    pub query_digest: String,
}

/// A reference into a viewport result, resolvable only inside its pinned revisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SelectionToken {
    /// One raw point of a series, named by its timestamp.
    ExactPoint {
        /// The point timestamp in nanoseconds.
        point_id: i64,
        /// The source revision the point was seen at.
        point_revision: u64,
    },
    /// One bin of a viewport result.
    AggregateBin {
        /// The bin id: a scatter cell id or a line column.
        bin_id: u32,
        /// The source revision of the result the bin came from.
        source_revision: u64,
        /// The binning formula revision of that result.
        index_revision: u32,
        /// The query digest of that result.
        viewport_digest: String,
    },
}

/// One raw point returned by a selection resolve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedPoint {
    /// Timestamp in nanoseconds.
    pub ts: i64,
    /// The field values of the point.
    pub values: Vec<f64>,
}

/// The answer of a `ResolveViewportSelection`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedSelection {
    /// At most [`MAX_RESOLVED_POINTS`] raw points, in timestamp order.
    pub points: Vec<ResolvedPoint>,
    /// Points the selection holds in total.
    pub total_count: u64,
    /// True when `total_count` exceeds the points returned.
    pub truncated: bool,
    /// The source revision read.
    pub source_revision: u64,
}

/// A typed viewport refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewportError {
    /// `VIEWPORT_INVALID`: a request or token the engine cannot honour.
    Invalid(String),
    /// `BYTE_CEILING_TOO_SMALL`: the ceiling cannot hold the envelope.
    ByteCeilingTooSmall {
        /// The requested ceiling.
        ceiling: u64,
        /// The smallest ceiling accepted.
        minimum: u64,
    },
    /// `VIEWPORT_CAP_EXCEEDED`: the bins or bytes pass their cap.
    CapExceeded(String),
    /// `INDEX_UNAVAILABLE`: the source or one of its axes is not indexed for the caller.
    IndexUnavailable(String),
    /// `STALE_SOURCE_REVISION`: the pinned revision is no longer current.
    StaleSourceRevision {
        /// The revision the caller pinned.
        pinned: u64,
        /// The current revision.
        current: u64,
    },
    /// `ACCESS_DENIED`: the request carries no verified caller.
    AccessDenied,
}

impl ViewportError {
    /// The stable refusal code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "VIEWPORT_INVALID",
            Self::ByteCeilingTooSmall { .. } => "BYTE_CEILING_TOO_SMALL",
            Self::CapExceeded(_) => "VIEWPORT_CAP_EXCEEDED",
            Self::IndexUnavailable(_) => "INDEX_UNAVAILABLE",
            Self::StaleSourceRevision { .. } => "STALE_SOURCE_REVISION",
            Self::AccessDenied => "ACCESS_DENIED",
        }
    }
}

impl std::fmt::Display for ViewportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let code = self.code();
        match self {
            Self::Invalid(d) | Self::CapExceeded(d) | Self::IndexUnavailable(d) => {
                write!(f, "{code}: {d}")
            }
            Self::ByteCeilingTooSmall { ceiling, minimum } => {
                write!(
                    f,
                    "{code}: ceiling {ceiling} is below the {minimum} byte envelope"
                )
            }
            Self::StaleSourceRevision { pinned, current } => {
                write!(f, "{code}: pinned {pinned}, current {current}")
            }
            Self::AccessDenied => write!(f, "{code}: no verified caller"),
        }
    }
}

impl std::error::Error for ViewportError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> ViewportRequest {
        ViewportRequest {
            source_id: "cpu".into(),
            source_revision: Some(7),
            kind: ViewportKind::Scatter,
            x_axis: "a".into(),
            y_axis: "b".into(),
            units: "ms".into(),
            filter_digest: String::new(),
            domain: ViewportDomain {
                x_min: 0.0,
                x_max: 10.0,
                y_min: 0.0,
                y_max: 10.0,
            },
            pixel_width: 100,
            pixel_height: 50,
            binning_revision: BINNING_REVISION,
            byte_ceiling: 1 << 20,
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R081.103.1
    #[test]
    fn request_bounds_and_token_union_round_trip() {
        assert_eq!(request().validate(), Ok(()));
        for (edit, label) in [
            (
                Box::new(|r: &mut ViewportRequest| r.pixel_width = 0)
                    as Box<dyn Fn(&mut ViewportRequest)>,
                "w0",
            ),
            (Box::new(|r| r.pixel_width = 4097), "w4097"),
            (Box::new(|r| r.pixel_height = 0), "h0"),
            (Box::new(|r| r.pixel_height = 4097), "h4097"),
            (Box::new(|r| r.byte_ceiling = 0), "c0"),
            (Box::new(|r| r.byte_ceiling = MAX_BYTE_CEILING + 1), "cmax"),
            (Box::new(|r| r.binning_revision = 9), "rev"),
            (Box::new(|r| r.domain.x_max = r.domain.x_min), "empty x"),
            (Box::new(|r| r.domain.y_min = f64::NAN), "nan"),
            (Box::new(|r| r.kind = ViewportKind::Line), "line axis"),
        ] {
            let mut r = request();
            edit(&mut r);
            let err = r.validate().expect_err(label);
            assert_eq!(err.code(), "VIEWPORT_INVALID", "{label}");
        }
        let mut edge = request();
        edge.pixel_width = MAX_PIXELS;
        edge.pixel_height = 1;
        edge.byte_ceiling = MAX_BYTE_CEILING;
        assert_eq!(edge.validate(), Ok(()));

        let tokens = [
            SelectionToken::ExactPoint {
                point_id: -5,
                point_revision: 3,
            },
            SelectionToken::AggregateBin {
                bin_id: 9,
                source_revision: 3,
                index_revision: BINNING_REVISION,
                viewport_digest: request().query_digest(),
            },
        ];
        for token in tokens {
            let json = serde_json::to_string(&token).unwrap();
            assert_eq!(
                serde_json::from_str::<SelectionToken>(&json).unwrap(),
                token
            );
            let packed = rmp_serde::to_vec_named(&token).unwrap();
            assert_eq!(
                rmp_serde::from_slice::<SelectionToken>(&packed).unwrap(),
                token
            );
        }
        assert!(serde_json::from_str::<SelectionToken>(r#"{"kind":"Raw","x":1}"#).is_err());

        let result = ViewportResult {
            bins: ViewportBins::Scatter(vec![ScatterBin {
                id: 4,
                count: 3,
                x_min: 1.0,
                x_max: 2.0,
                y_min: 3.0,
                y_max: 4.0,
            }]),
            source_revision: 3,
            viewport: ViewportSpec {
                domain: request().domain,
                pixel_width: 100,
                pixel_height: 50,
            },
            binning_revision: BINNING_REVISION,
            source_cardinality: 10,
            query_digest: request().query_digest(),
        };
        let packed = rmp_serde::to_vec_named(&result).unwrap();
        assert_eq!(
            rmp_serde::from_slice::<ViewportResult>(&packed).unwrap(),
            result
        );
        let line = ViewportBins::Line(vec![LineBin {
            col: 1,
            first: 1.0,
            last: 2.0,
            min: 0.5,
            max: 2.5,
            count: 4,
            gap_before: true,
        }]);
        let packed = rmp_serde::to_vec_named(&line).unwrap();
        assert_eq!(
            rmp_serde::from_slice::<ViewportBins>(&packed).unwrap(),
            line
        );

        // A pan changes the digest; the pinned revision and the byte ceiling do not.
        let base = request().query_digest();
        let mut panned = request();
        panned.domain.x_min = 1.0;
        assert_ne!(panned.query_digest(), base);
        let mut zoomed = request();
        zoomed.pixel_width = 200;
        assert_ne!(zoomed.query_digest(), base);
        let mut repin = request();
        repin.source_revision = None;
        repin.byte_ceiling = 4096;
        assert_eq!(repin.query_digest(), base);
    }
}
