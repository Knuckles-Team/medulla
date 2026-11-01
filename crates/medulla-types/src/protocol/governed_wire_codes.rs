//! Governed-change refusals keep their declared wire codes through a protocol
//! `Response` (the response envelope lives in this hub).

// spec: MEDULLA-CONTRACT-R014
#[test]
fn governed_refusals_preserve_their_declared_wire_codes() {
    use super::Response;
    use crate::governed_change::GovernedRefusal;

    for refusal in [
        GovernedRefusal::InvalidRequest,
        GovernedRefusal::UnknownKind,
        GovernedRefusal::NotAuthorized,
        GovernedRefusal::Collision,
        GovernedRefusal::Full,
        GovernedRefusal::NotFound,
        GovernedRefusal::SelfApproval,
        GovernedRefusal::DelegatedApprover,
        GovernedRefusal::DigestMismatch,
        GovernedRefusal::Conflict,
    ] {
        let response = Response::err(7, refusal.to_string());
        assert_eq!(response.error.as_deref(), Some(refusal.code()));
        assert_eq!(
            response.refusal_text().as_deref(),
            Some(refusal.to_string().as_str())
        );
    }
}
