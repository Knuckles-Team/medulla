//! `OntologyCanonicalHash`: the RDFC-1.0 SHA-256 digest of a Turtle payload
//! (MEDULLA-TYPED-PACKS-R809). Pure: the payload is the only input.

use medulla_protocol_types::protocol::{Response, ResultPayload};
use medulla_types::ontology_inspection::OntologyCanonicalHashResult;

/// The most triples one payload may carry (the bound of a schema document).
const MAX_TRIPLES: usize = 100_000;
/// The most bytes one payload may carry.
const MAX_BYTES: usize = 2 << 20;

pub(super) fn handle_ontology_canonical_hash(req_id: u64, turtle: &str) -> Response {
    match canonical_hash(turtle) {
        Ok(hash) => Response::ok(
            req_id,
            ResultPayload::of::<
                medulla_protocol_types::result_contract::reasoning::OntologyCanonicalHash,
            >(OntologyCanonicalHashResult { hash }),
        ),
        Err(detail) => Response::err(
            req_id,
            format!(
                "{}: {detail}",
                medulla_types::contract::ServerErrorCode::TurtleInvalid.as_str()
            ),
        ),
    }
}

fn canonical_hash(turtle: &str) -> Result<String, String> {
    if turtle.len() > MAX_BYTES {
        return Err(format!("payload exceeds the maximum of {MAX_BYTES} bytes"));
    }
    medulla_rdf::ontology_hash::canonical_turtle_hash(turtle, MAX_TRIPLES)
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-TYPED-PACKS-R809
    #[test]
    fn bad_turtle_is_refused_with_the_registered_code() {
        let refused = handle_ontology_canonical_hash(1, "<a> <b>");
        assert_eq!(refused.error.as_deref(), Some("TURTLE_INVALID"));
        let ok = handle_ontology_canonical_hash(2, "<http://e/a> <http://e/b> <http://e/c> .");
        assert!(ok.error.is_none());
    }
}
