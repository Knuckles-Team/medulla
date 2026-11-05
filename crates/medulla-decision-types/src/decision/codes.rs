//! Shared request refusal codes of the decision plane (MEDULLA-DECISION-ENGINE-R010.100.1).
//!
//! A code here may be returned by any family. A family's own `REFUSALS` table
//! must not repeat one of these, nor another family's code, so one code names
//! one refusal across the whole decision surface.

use super::record::outcome::RefusalCode;
use super::statistical::QuestionKind;

/// The caller may not read or act on what the request names.
pub const ACCESS_DENIED: RefusalCode = RefusalCode::new("ACCESS_DENIED");
/// A request parameter is missing, malformed or out of range.
pub const PARAMETER_INVALID: RefusalCode = RefusalCode::new("PARAMETER_INVALID");
/// A request reused an idempotency key with a different body.
pub const IDEMPOTENCY_CONFLICT: RefusalCode = RefusalCode::new("IDEMPOTENCY_CONFLICT");

/// Every shared request code, in declaration order.
pub const SHARED: &[RefusalCode] = &[ACCESS_DENIED, PARAMETER_INVALID, IDEMPOTENCY_CONFLICT];

/// The `REFUSALS` table of every registered family, keyed by its question
/// kind. A family module joins this list in the change that declares its table.
pub const FAMILY_TABLES: &[(QuestionKind, &[RefusalCode])] = &[];

/// Resolve a wire string to its registered code, shared codes first.
pub fn resolve(code: &str) -> Option<RefusalCode> {
    SHARED
        .iter()
        .chain(FAMILY_TABLES.iter().flat_map(|(_, table)| table.iter()))
        .chain(super::usage::REFUSALS.iter())
        .copied()
        .find(|registered| registered.as_str() == code)
}
