//! Canonical hash of a Turtle payload (MEDULLA-TYPED-PACKS-R809).
//!
//! The payload is parsed, canonicalized with RDFC-1.0 (SHA-256 blank-node
//! hashing) and the sorted canonical N-Quads document is hashed with SHA-256.
//! The digest is stable under prefix changes, statement order and blank-node
//! relabelling.

use oxrdf::dataset::{CanonicalizationAlgorithm, CanonicalizationHashAlgorithm};
use oxrdf::{Dataset, GraphName, Quad};
use sha2::{Digest, Sha256};

/// The canonical digest of `turtle`: 64 lowercase hex characters, or the parse
/// failure (`TURTLE_INVALID` at the served boundary).
pub fn canonical_turtle_hash(turtle: &str, max_triples: usize) -> Result<String, String> {
    let triples = crate::mapping::parse_turtle_bounded(turtle, max_triples)?;
    let mut dataset = Dataset::new();
    for triple in triples {
        dataset.insert(&Quad::new(
            triple.subject,
            triple.predicate,
            triple.object,
            GraphName::DefaultGraph,
        ));
    }
    dataset.canonicalize(CanonicalizationAlgorithm::Rdfc10 {
        hash_algorithm: CanonicalizationHashAlgorithm::Sha256,
    });
    let mut lines: Vec<String> = dataset
        .iter()
        .map(|quad| format!("{} {} {} .\n", quad.subject, quad.predicate, quad.object))
        .collect();
    lines.sort();
    let digest = Sha256::digest(lines.concat().as_bytes());
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::canonical_turtle_hash;

    // spec: MEDULLA-TYPED-PACKS-R809
    #[test]
    fn hash_is_stable_under_prefix_order_and_blank_node_labels() {
        let first = "@prefix ex: <http://example.org/> .\n\
                     ex:a ex:p _:x . _:x ex:q \"v\" . ex:b ex:p ex:a .";
        let second = "@prefix e: <http://example.org/> .\n\
                      e:b e:p e:a . _:other e:q \"v\" . e:a e:p _:other .";
        let one = canonical_turtle_hash(first, 1000).unwrap();
        assert_eq!(one.len(), 64);
        assert_eq!(one, canonical_turtle_hash(second, 1000).unwrap());
        let different = "<http://example.org/a> <http://example.org/p> \"other\" .";
        assert_ne!(one, canonical_turtle_hash(different, 1000).unwrap());
        assert!(canonical_turtle_hash("not turtle at all", 1000).is_err());
    }
}
