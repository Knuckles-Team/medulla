//! Re-export of the wire scalars this module's algorithms use. The
//! definitions live in `medulla_types::solve_scalar` -- see that module's own
//! doc for why there is exactly one copy.

pub use medulla_types::solve_scalar::Scalar;
pub use medulla_types::solve_scalar::ScalarParseError;
pub use medulla_types::solve_scalar::Sha256Digest;
