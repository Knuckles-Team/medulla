//! Lossless SKILL.md editing (MEDULLA-TYPED-PACKS-R100.100).
//!
//! [`apply_skill_patches`] is a pure byte-range function. It never parses or reserializes the
//! document, so untouched bytes, key order, spacing, comments, line endings and unknown
//! frontmatter fields stay identical. The caller validates the result under the SKILL.md-only
//! import (R019) and the restricted YAML profile (R029) before it publishes anything.

use medulla_agent_types::agent_component::{
    SkillSourcePatch, MAX_SKILL_PATCHES, SKILL_PATCH_ANCHOR_INVALID, SKILL_PATCH_OVERLAP,
    SKILL_SOURCE_STALE, SKILL_SPAN_UNKNOWN,
};
use sha2::{Digest, Sha256};

/// The largest edited source this function returns.
pub const MAX_EDITED_SKILL_BYTES: usize = 2 * 1024 * 1024;

/// The edited bytes and their digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditedSkill {
    /// The edited source.
    pub bytes: Vec<u8>,
    /// `sha256:<hex>` of `bytes`.
    pub digest: String,
}

/// `sha256:<hex>` of `bytes`.
pub fn skill_digest(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

/// Whether the source writes CRLF line endings anywhere.
fn uses_crlf(source: &[u8]) -> bool {
    source.windows(2).any(|pair| pair == b"\r\n")
}

/// A replacement keeps the line-ending style of the source: a CRLF source takes only CRLF
/// breaks, an LF source takes no carriage return.
fn keeps_line_endings(replacement: &str, crlf: bool) -> bool {
    let bytes = replacement.as_bytes();
    if crlf {
        bytes.iter().enumerate().all(|(index, byte)| match byte {
            b'\n' => index > 0 && bytes[index - 1] == b'\r',
            b'\r' => bytes.get(index + 1) == Some(&b'\n'),
            _ => true,
        })
    } else {
        !bytes.contains(&b'\r')
    }
}

/// Apply `patches` (ranges over the ORIGINAL `source`) and return the edited bytes.
///
/// Refusals, in order: `SKILL_SOURCE_STALE` when `source_digest` is not the digest of `source`;
/// `SKILL_SPAN_UNKNOWN` for no patch, too many, a range outside the source or an edited result
/// over [`MAX_EDITED_SKILL_BYTES`]; `SKILL_PATCH_ANCHOR_INVALID` for a boundary inside a
/// character or a replacement that breaks the source's line-ending style or holds a NUL;
/// `SKILL_PATCH_OVERLAP` for overlapping patches or two insertions at one point. Nothing is
/// returned on a refusal.
pub fn apply_skill_patches(
    source: &[u8],
    source_digest: &str,
    patches: &[SkillSourcePatch],
) -> Result<EditedSkill, String> {
    if skill_digest(source) != source_digest {
        return Err(format!(
            "{SKILL_SOURCE_STALE}: the source is not the one the patches were made against"
        ));
    }
    if patches.is_empty() || patches.len() > MAX_SKILL_PATCHES {
        return Err(format!(
            "{SKILL_SPAN_UNKNOWN}: an edit carries 1 to {MAX_SKILL_PATCHES} patches"
        ));
    }
    let text = std::str::from_utf8(source)
        .map_err(|_| format!("{SKILL_SPAN_UNKNOWN}: the source is not UTF-8"))?;
    let crlf = uses_crlf(source);
    let mut ranges: Vec<(usize, usize, &str)> = Vec::with_capacity(patches.len());
    for patch in patches {
        let (start, end) = match (
            usize::try_from(patch.range_start),
            usize::try_from(patch.range_end),
        ) {
            (Ok(start), Ok(end)) if start <= end && end <= source.len() => (start, end),
            _ => {
                return Err(format!(
                    "{SKILL_SPAN_UNKNOWN}: range {}..{} is outside the {}-byte source",
                    patch.range_start,
                    patch.range_end,
                    source.len()
                ))
            }
        };
        if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            return Err(format!(
                "{SKILL_PATCH_ANCHOR_INVALID}: range {start}..{end} splits a character"
            ));
        }
        if patch.replacement.contains('\0') || !keeps_line_endings(&patch.replacement, crlf) {
            return Err(format!(
                "{SKILL_PATCH_ANCHOR_INVALID}: the replacement at {start} breaks the line-ending \
                 style of the source"
            ));
        }
        ranges.push((start, end, patch.replacement.as_str()));
    }
    ranges.sort_by_key(|(start, end, _)| (*start, *end));
    for pair in ranges.windows(2) {
        let (previous, next) = (pair[0], pair[1]);
        let both_insert_here = previous.0 == previous.1 && next.0 == next.1 && previous.0 == next.0;
        if next.0 < previous.1 || both_insert_here {
            return Err(format!(
                "{SKILL_PATCH_OVERLAP}: ranges {}..{} and {}..{} overlap",
                previous.0, previous.1, next.0, next.1
            ));
        }
    }
    let mut edited = Vec::with_capacity(source.len());
    let mut cursor = 0;
    for (start, end, replacement) in ranges {
        edited.extend_from_slice(&source[cursor..start]);
        edited.extend_from_slice(replacement.as_bytes());
        cursor = end;
        if edited.len() > MAX_EDITED_SKILL_BYTES {
            return Err(format!(
                "{SKILL_SPAN_UNKNOWN}: the edited source is over {MAX_EDITED_SKILL_BYTES} bytes"
            ));
        }
    }
    edited.extend_from_slice(&source[cursor..]);
    if edited.len() > MAX_EDITED_SKILL_BYTES {
        return Err(format!(
            "{SKILL_SPAN_UNKNOWN}: the edited source is over {MAX_EDITED_SKILL_BYTES} bytes"
        ));
    }
    let digest = skill_digest(&edited);
    Ok(EditedSkill {
        bytes: edited,
        digest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = "---\r\nname: demo   # keep this comment\r\nx-unknown: [1,  2]\r\ndescription: old\r\n---\r\n\r\n1. first step\r\n2. second step\r\n";

    fn patch(source: &str, from: &str, replacement: &str) -> SkillSourcePatch {
        let start = source.find(from).unwrap();
        SkillSourcePatch {
            range_start: start as u64,
            range_end: (start + from.len()) as u64,
            replacement: replacement.to_string(),
        }
    }

    // spec: MEDULLA-TYPED-PACKS-R100.100
    #[test]
    fn untouched_bytes_stay_identical() {
        let digest = skill_digest(SOURCE.as_bytes());
        let patches = [
            patch(SOURCE, "old", "new description"),
            patch(SOURCE, "second step", "second step, edited"),
        ];
        let edited = apply_skill_patches(SOURCE.as_bytes(), &digest, &patches).unwrap();
        let expected = SOURCE
            .replace("description: old", "description: new description")
            .replace("second step", "second step, edited");
        assert_eq!(String::from_utf8(edited.bytes.clone()).unwrap(), expected);
        // The comment, the unknown field with its odd spacing and the CRLF endings survive.
        let text = String::from_utf8(edited.bytes).unwrap();
        assert!(text.contains("name: demo   # keep this comment\r\n"));
        assert!(text.contains("x-unknown: [1,  2]\r\n"));
        assert_eq!(edited.digest, skill_digest(text.as_bytes()));
    }

    // spec: MEDULLA-TYPED-PACKS-R100.100
    #[test]
    fn stale_overlapping_unanchored_and_foreign_ending_edits_refuse() {
        let digest = skill_digest(SOURCE.as_bytes());
        let one = patch(SOURCE, "first", "1st");
        assert!(
            apply_skill_patches(SOURCE.as_bytes(), &skill_digest(b"other"), &[one.clone()])
                .unwrap_err()
                .starts_with(SKILL_SOURCE_STALE)
        );
        let overlapping = [
            SkillSourcePatch {
                range_start: 10,
                range_end: 20,
                replacement: "a".into(),
            },
            SkillSourcePatch {
                range_start: 15,
                range_end: 25,
                replacement: "b".into(),
            },
        ];
        assert!(
            apply_skill_patches(SOURCE.as_bytes(), &digest, &overlapping)
                .unwrap_err()
                .starts_with(SKILL_PATCH_OVERLAP)
        );
        let two_inserts = [
            SkillSourcePatch {
                range_start: 5,
                range_end: 5,
                replacement: "a".into(),
            },
            SkillSourcePatch {
                range_start: 5,
                range_end: 5,
                replacement: "b".into(),
            },
        ];
        assert!(
            apply_skill_patches(SOURCE.as_bytes(), &digest, &two_inserts)
                .unwrap_err()
                .starts_with(SKILL_PATCH_OVERLAP)
        );
        let outside = [SkillSourcePatch {
            range_start: 0,
            range_end: SOURCE.len() as u64 + 1,
            replacement: String::new(),
        }];
        assert!(apply_skill_patches(SOURCE.as_bytes(), &digest, &outside)
            .unwrap_err()
            .starts_with(SKILL_SPAN_UNKNOWN));
        // A bare line feed breaks the CRLF style of the source.
        let lf = [patch(SOURCE, "first", "1st\nline")];
        assert!(apply_skill_patches(SOURCE.as_bytes(), &digest, &lf)
            .unwrap_err()
            .starts_with(SKILL_PATCH_ANCHOR_INVALID));
        // A boundary inside a multi-byte character is refused.
        let accented = "---\nname: caf\u{e9}\n---\n";
        let accent_digest = skill_digest(accented.as_bytes());
        let inside = [SkillSourcePatch {
            range_start: accented.find('\u{e9}').unwrap() as u64 + 1,
            range_end: accented.find('\u{e9}').unwrap() as u64 + 2,
            replacement: "e".into(),
        }];
        assert!(
            apply_skill_patches(accented.as_bytes(), &accent_digest, &inside)
                .unwrap_err()
                .starts_with(SKILL_PATCH_ANCHOR_INVALID)
        );
    }
}
