//! The blob body of one ANN generation image (MEDULLA-UNIFIED-DATA-PLANE-R040.6).
//!
//! The external S6 worker uploads the image as one content-addressed blob:
//! the five image parts in the fixed `PARTS` order, each framed by its
//! little-endian `u64` length, under a version prefix. The engine decodes the
//! body it read and verified itself; a malformed body refuses before any
//! activation work.

use super::rows::PARTS;
use super::{refused, SemanticCodeError};
use medulla_semantic_index::semantic::SemanticGenerationImage;

const IMAGE_BODY_PREFIX: &[u8] = b"eg/semantic-ann-image-body/v1\0";

fn parts(image: &SemanticGenerationImage) -> [&[u8]; 5] {
    [
        image.index.codes.meta.as_slice(),
        image.index.codes.codes.as_slice(),
        image.index.codes.refine.as_slice(),
        image.index.ids.as_slice(),
        image.manifest.as_slice(),
    ]
}

/// Encode `image` as its blob body.
pub fn encode_image_body(image: &SemanticGenerationImage) -> Vec<u8> {
    let mut body = IMAGE_BODY_PREFIX.to_vec();
    for part in parts(image) {
        body.extend_from_slice(&(part.len() as u64).to_le_bytes());
        body.extend_from_slice(part);
    }
    body
}

fn take_part<'a>(rest: &mut &'a [u8]) -> Result<Vec<u8>, SemanticCodeError> {
    let (len, tail) = rest
        .split_first_chunk::<8>()
        .ok_or_else(|| refused("ANN image body is truncated"))?;
    let len = usize::try_from(u64::from_le_bytes(*len))
        .map_err(|_| refused("ANN image body part is too large"))?;
    if tail.len() < len {
        return Err(refused("ANN image body part is truncated"));
    }
    let (part, tail) = tail.split_at(len);
    *rest = tail;
    Ok(part.to_vec())
}

/// Decode a blob body into the image; trailing bytes refuse.
pub fn decode_image_body(body: &[u8]) -> Result<SemanticGenerationImage, SemanticCodeError> {
    let mut rest = body
        .strip_prefix(IMAGE_BODY_PREFIX)
        .ok_or_else(|| refused("ANN image body has no version prefix"))?;
    let mut decoded = Vec::with_capacity(PARTS.len());
    for _ in PARTS {
        decoded.push(take_part(&mut rest)?);
    }
    if !rest.is_empty() {
        return Err(refused("ANN image body has trailing bytes"));
    }
    let [meta, codes, refine, ids, manifest]: [Vec<u8>; 5] = decoded
        .try_into()
        .map_err(|_| refused("ANN image body part count"))?;
    Ok(SemanticGenerationImage {
        index: medulla_semantic_index::semantic_ann::AnnIndexImage {
            codes: medulla_ann::durable_codes::AnnCodeArtifact {
                meta,
                codes,
                refine,
            },
            ids,
        },
        manifest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image() -> SemanticGenerationImage {
        decode_image_body(&{
            let mut body = IMAGE_BODY_PREFIX.to_vec();
            for part in [&b"m"[..], b"cc", b"", b"iiii", b"{}"] {
                body.extend_from_slice(&(part.len() as u64).to_le_bytes());
                body.extend_from_slice(part);
            }
            body
        })
        .unwrap()
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R040.6
    #[test]
    fn image_body_round_trips_and_refuses_malformed() {
        let image = image();
        let body = encode_image_body(&image);
        assert_eq!(decode_image_body(&body).unwrap(), image);
        let mut trailing = body.clone();
        trailing.push(0);
        let cases: [&[u8]; 3] = [&body[..body.len() - 1], &trailing, &body[1..]];
        for case in cases {
            assert!(decode_image_body(case).is_err());
        }
    }
}
