//! General bounded 0-1 integer programming with verifiable certificates.
//!
//! A [`Model`] is a validated set of binary variables, integer linear rows
//! (with typed implications, exactly-one, at-most-k and at-least-k groups) and
//! a lexicographic objective scalarised exactly in `i128`, where an unknown
//! cost is ranked in its own tier and never read as zero. [`solve`] runs a
//! deterministic branch-and-bound under a node budget and returns a
//! [`Certificate`]: status, incumbent, lower bound and a proof. [`verify`]
//! re-checks a certificate without trusting or sharing any of the search.
//!
//! Everything on this path is integer arithmetic; no clock, float or hash
//! order influences a result, so equal inputs give byte-identical
//! certificates and digests.

pub mod certificate;
pub mod context_package;
pub mod model;
pub mod placement;
pub mod scalar;
mod search;
pub mod verify;

use model::{Model, ModelError};
pub use search::solve;
pub use verify::{verify, StatusDefect, Verdict, VerifyError};
