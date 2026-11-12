//! Per-span embedding admission at document enrichment
//! (MEDULLA-DECISION-ENGINE-R079.2, R079.3).
//!
//! `classify_unit` (medulla-decision-types) names a span's content class;
//! [`admit_unit`] then decides, with no embedding call and no store access,
//! whether the span is embedded: an excluded class never is, a span whose
//! exact text was already embedded reuses that vector, and a span that is
//! too short, too long, too repetitive or a normalized repeat is skipped.

use std::collections::{BTreeMap, BTreeSet};

use medulla_decision_types::decision::ingestion_admission::{AdmissionKnobs, ContentClass};
use medulla_source_types::source_ingestion::web_document::{AdmissionOutcome, UnitAdmission};
use sha2::{Digest, Sha256};

/// Snake-case label of a content class, as the receipt records it.
pub fn class_label(class: ContentClass) -> String {
    serde_json::to_value(class)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// SHA-256 hex of the exact text.
pub fn exact_digest(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// SHA-256 hex of the lowercase text with whitespace runs collapsed.
pub fn normalized_digest(text: &str) -> String {
    let collapsed = text
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    hex::encode(Sha256::digest(collapsed.as_bytes()))
}

/// Shannon entropy of the characters of `text`, in bits per character.
pub fn char_entropy_bits(text: &str) -> f64 {
    let mut counts: BTreeMap<char, usize> = BTreeMap::new();
    let mut total = 0usize;
    for c in text.chars() {
        *counts.entry(c).or_default() += 1;
        total += 1;
    }
    if total == 0 {
        return 0.0;
    }
    counts
        .values()
        .map(|&n| {
            let p = n as f64 / total as f64;
            -p * p.log2()
        })
        .sum()
}

/// Decide one span. `known_exact` holds the exact digests of spans already
/// embedded for this Document; `seen` holds the normalized digests of the
/// spans this run has already decided and is extended with this span's.
pub fn admit_unit(
    ordinal: u32,
    class: ContentClass,
    text: &str,
    known_exact: &BTreeSet<String>,
    seen: &mut BTreeSet<String>,
    knobs: &AdmissionKnobs,
) -> UnitAdmission {
    let outcome = decide(class, text, known_exact, seen, knobs);
    UnitAdmission {
        ordinal,
        class: class_label(class),
        outcome,
    }
}

fn decide(
    class: ContentClass,
    text: &str,
    known_exact: &BTreeSet<String>,
    seen: &mut BTreeSet<String>,
    knobs: &AdmissionKnobs,
) -> AdmissionOutcome {
    if !class.admits() {
        return AdmissionOutcome::Excluded;
    }
    if known_exact.contains(&exact_digest(text)) {
        seen.insert(normalized_digest(text));
        return AdmissionOutcome::SkippedExact;
    }
    let chars = text.chars().count();
    if chars < knobs.min_unit_chars || chars > knobs.max_unit_chars {
        return AdmissionOutcome::SizeOutOfBounds;
    }
    if char_entropy_bits(text) < knobs.min_entropy_bits {
        return AdmissionOutcome::LowEntropy;
    }
    if !seen.insert(normalized_digest(text)) {
        return AdmissionOutcome::Duplicate;
    }
    AdmissionOutcome::Admitted
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_decision_types::decision::policy::DecisionPolicy;

    fn knobs() -> AdmissionKnobs {
        AdmissionKnobs::from_policy(&DecisionPolicy::engine_default())
    }

    fn run(class: ContentClass, text: &str, seen: &mut BTreeSet<String>) -> AdmissionOutcome {
        admit_unit(0, class, text, &BTreeSet::new(), seen, &knobs()).outcome
    }

    const PROSE: &str = "Chunks carry offsets so a citation can quote the source exactly.";

    // spec: MEDULLA-DECISION-ENGINE-R079.3
    #[test]
    fn each_skip_reason_and_the_admit_path() {
        let mut seen = BTreeSet::new();
        assert_eq!(
            run(ContentClass::Prose, PROSE, &mut seen),
            AdmissionOutcome::Admitted
        );
        let shouted = format!("  {}\n", PROSE.to_uppercase().replace(' ', "   "));
        assert_eq!(
            run(ContentClass::Prose, &shouted, &mut seen),
            AdmissionOutcome::Duplicate
        );
        assert_eq!(
            run(ContentClass::Prose, &"a".repeat(64), &mut seen),
            AdmissionOutcome::LowEntropy
        );
        assert_eq!(
            run(ContentClass::Prose, "too short", &mut seen),
            AdmissionOutcome::SizeOutOfBounds
        );
        let long = format!("{PROSE} {}", "variety of words ".repeat(600));
        assert_eq!(
            run(ContentClass::Prose, &long, &mut seen),
            AdmissionOutcome::SizeOutOfBounds
        );
        assert_eq!(
            run(ContentClass::Lockfile, PROSE, &mut BTreeSet::new()),
            AdmissionOutcome::Excluded
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R079.2
    #[test]
    fn exact_digest_of_an_embedded_span_is_skipped() {
        let known = BTreeSet::from([exact_digest(PROSE)]);
        let admission = admit_unit(
            3,
            ContentClass::Prose,
            PROSE,
            &known,
            &mut BTreeSet::new(),
            &knobs(),
        );
        assert_eq!(admission.outcome, AdmissionOutcome::SkippedExact);
        assert_eq!((admission.ordinal, admission.class.as_str()), (3, "prose"));
    }

    #[test]
    fn entropy_is_zero_for_one_symbol_and_one_bit_for_two() {
        assert_eq!(char_entropy_bits(""), 0.0);
        assert_eq!(char_entropy_bits("aaaa"), 0.0);
        assert!((char_entropy_bits("abab") - 1.0).abs() < 1e-9);
    }
}
