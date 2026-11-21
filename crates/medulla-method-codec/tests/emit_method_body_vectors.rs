//! The method-body vectors are a pure function of the codec and the samples
//! (MEDULLA-CONTRACT-R061).

use medulla_method_codec::canonical_body;
use medulla_protocol_types::protocol_fixtures::method_bodies::typed_samples;

#[path = "../examples/support/method_vectors.rs"]
mod method_vectors;

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).expect("hex octet"))
        .collect()
}

// spec: MEDULLA-CONTRACT-R061
#[test]
fn emits_one_deterministic_vector_per_typed_sample() {
    let first = method_vectors::method_body_vectors();
    let second = method_vectors::method_body_vectors();
    assert_eq!(first, second, "the output is a pure function of the codec and the samples");

    let (vectors, omitted) = first;
    for (name, _) in typed_samples() {
        let in_vectors = vectors
            .iter()
            .filter(|vector| vector["name"] == name)
            .count();
        let in_omitted = omitted.iter().filter(|(omitted, _)| omitted == name).count();
        assert_eq!(
            in_vectors + in_omitted,
            1,
            "{name} appears once across vectors and omissions"
        );
    }
    assert_eq!(vectors.len() + omitted.len(), typed_samples().len());

    for vector in &vectors {
        let frame = unhex(vector["frame"].as_str().expect("a frame"));
        let body = canonical_body(&frame).expect("the frame decodes");
        assert_eq!(
            vector["body"].as_str().expect("a body"),
            method_vectors::hex(&body),
            "{}",
            vector["name"]
        );
    }
}
