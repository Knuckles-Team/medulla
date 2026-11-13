//! The refusal and premise vocabulary shared by the trajectory decisions
//! (MEDULLA-DECISION-ENGINE-R140.100 to R140.103).

use crate::decision::record::outcome::RefusalCode;
use crate::decision::{PremiseClass, PremiseProvenance, PremiseRef};

/// Why a trajectory decision was refused: a registered code and a detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrajectoryRefusal {
    pub code: RefusalCode,
    pub detail: String,
}

impl TrajectoryRefusal {
    pub fn new(code: RefusalCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

/// A premise the request stated in `field`.
pub(super) fn request_premise(
    subject: &str,
    fact: String,
    class: PremiseClass,
    field: &str,
) -> PremiseRef {
    PremiseRef {
        subject: subject.to_string(),
        fact,
        class,
        provenance: PremiseProvenance::Request {
            field: field.to_string(),
        },
    }
}
