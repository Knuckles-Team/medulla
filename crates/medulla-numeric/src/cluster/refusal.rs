//! Typed refusals of the clustering kernel (MEDULLA-CLUSTER-R001.2).

use crate::error::NumericError;

/// Why a clustering run refused to start or stopped without centroids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClusterRefusal {
    /// `k` is zero.
    KInvalid,
    /// `k` is above the row count.
    KExceedsRows { k: usize, n: usize },
    /// Fewer than `k` distinct rows.
    TooFewDistinct { k: usize },
    /// Empty input, zero width, or a slice that does not match `n * dim`.
    ShapeInvalid(&'static str),
    /// A NaN or infinite value at the named row and column.
    InputNotFinite { row: usize, col: usize },
    /// The training memory need is above the budget.
    MemoryBudget { need: u64, budget: u64 },
    /// The run was cancelled at a step boundary.
    Cancelled,
    /// The state machine received an event its table does not allow.
    StateIllegal {
        from: &'static str,
        event: &'static str,
    },
}

impl ClusterRefusal {
    /// The stable wire code of the refusal.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::KInvalid => "CLUSTERING_K_INVALID",
            Self::KExceedsRows { .. } => "CLUSTERING_K_EXCEEDS_ROWS",
            Self::TooFewDistinct { .. } => "CLUSTERING_TOO_FEW_DISTINCT",
            Self::ShapeInvalid(_) => "CLUSTERING_SHAPE_INVALID",
            Self::InputNotFinite { .. } => "CLUSTERING_INPUT_NOT_FINITE",
            Self::MemoryBudget { .. } => "CLUSTERING_MEMORY_BUDGET",
            Self::Cancelled => "CLUSTERING_CANCELLED",
            Self::StateIllegal { .. } => "CLUSTERING_STATE_ILLEGAL",
        }
    }
}

impl std::fmt::Display for ClusterRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: ", self.code())?;
        match self {
            Self::KInvalid => write!(f, "k must be at least 1"),
            Self::KExceedsRows { k, n } => write!(f, "k = {k} is above the {n} input rows"),
            Self::TooFewDistinct { k } => write!(f, "fewer than {k} distinct rows"),
            Self::ShapeInvalid(why) => write!(f, "{why}"),
            Self::InputNotFinite { row, col } => {
                write!(f, "non-finite value at row {row}, column {col}")
            }
            Self::MemoryBudget { need, budget } => {
                write!(f, "training needs {need} bytes, budget is {budget}")
            }
            Self::Cancelled => write!(f, "the run was cancelled"),
            Self::StateIllegal { from, event } => {
                write!(f, "event {event} is illegal in state {from}")
            }
        }
    }
}

impl std::error::Error for ClusterRefusal {}

impl From<ClusterRefusal> for NumericError {
    fn from(r: ClusterRefusal) -> Self {
        let text = r.to_string();
        match r {
            ClusterRefusal::ShapeInvalid(_) | ClusterRefusal::InputNotFinite { .. } => {
                NumericError::Shape(text)
            }
            ClusterRefusal::MemoryBudget { .. } | ClusterRefusal::Cancelled => {
                NumericError::Resource(text)
            }
            _ => NumericError::Bounds(text),
        }
    }
}
