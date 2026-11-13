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
use medulla_source_types::source_ingestion::column_profile::{
    ColumnAdmission, ColumnExclusion, ColumnExclusionReason, EmbeddingSetEntry, SqlColumnProfile,
    SqlColumnType, Q32_ONE,
};
use medulla_source_types::source_ingestion::web_document::{AdmissionOutcome, UnitAdmission};
use medulla_source_types::source_ingestion::SourceRecord;
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
    demoted: &BTreeSet<ContentClass>,
    seen: &mut BTreeSet<String>,
    knobs: &AdmissionKnobs,
) -> UnitAdmission {
    let outcome = decide(class, text, known_exact, demoted, seen, knobs);
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
    demoted: &BTreeSet<ContentClass>,
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
    // Reusing a held vector costs nothing, so an exact repeat is kept even in a
    // demoted class; only a span that would be embedded fresh is skipped.
    if demoted.contains(&class) {
        return AdmissionOutcome::Demoted;
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

/// Decide one SQL column (MEDULLA-DECISION-ENGINE-R079.4). Foreign keys,
/// booleans, timestamps and numeric types never enter the embedding set. A
/// text column whose distinct ratio is below the knob is an enumeration and
/// stays out; free text is admitted only when measured over at least
/// `knobs.column_min_samples` values.
pub fn admit_sql_column(profile: &SqlColumnProfile, knobs: &AdmissionKnobs) -> ColumnAdmission {
    use ColumnExclusionReason as Why;
    if profile.is_foreign_key {
        return ColumnAdmission::Excluded(Why::ForeignKey);
    }
    match profile.sql_type {
        SqlColumnType::Boolean => return ColumnAdmission::Excluded(Why::Boolean),
        SqlColumnType::Timestamp => return ColumnAdmission::Excluded(Why::Timestamp),
        SqlColumnType::Numeric => return ColumnAdmission::Excluded(Why::Numeric),
        SqlColumnType::Text => {}
    }
    // ratio_q32 / 2^32 < micros / 10^6, compared without division.
    let below = u128::from(profile.distinct_ratio_q32) * 1_000_000
        < u128::from(knobs.column_min_distinct_micros) * u128::from(Q32_ONE);
    if below {
        return ColumnAdmission::Excluded(Why::Enumeration);
    }
    if profile.sample_count < knobs.column_min_samples {
        return ColumnAdmission::Excluded(Why::InsufficientSamples);
    }
    ColumnAdmission::Admitted
}

/// The column values that survive admission, and the columns that do not
/// (MEDULLA-DECISION-ENGINE-R079.6). Medulla owns this set; the connector only
/// supplies the profiles.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EmbeddingSet {
    /// The column values that enter the embedding set.
    pub entries: Vec<EmbeddingSetEntry>,
    /// The columns excluded from the set, each with its reason.
    pub exclusions: Vec<ColumnExclusion>,
}

/// Apply [`admit_sql_column`] to every profiled column of `records`. A column
/// value enters the set when its column is admitted and the payload holds a
/// non-empty string for it; each excluded column is named once per stream and
/// reason.
pub fn build_embedding_set<'a>(
    records: impl IntoIterator<Item = &'a SourceRecord>,
    knobs: &AdmissionKnobs,
) -> EmbeddingSet {
    let mut entries = Vec::new();
    let mut exclusions = BTreeSet::new();
    for record in records {
        if record.column_profiles.is_empty() {
            continue;
        }
        let Ok(payload) = record.payload.value() else {
            continue;
        };
        for (column, profile) in &record.column_profiles {
            match admit_sql_column(profile, knobs) {
                ColumnAdmission::Excluded(reason) => {
                    exclusions.insert(ColumnExclusion {
                        stream: record.stream.clone(),
                        column: column.clone(),
                        reason,
                    });
                }
                ColumnAdmission::Admitted => {
                    let has_text = payload
                        .get(column)
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|text| !text.trim().is_empty());
                    if has_text {
                        entries.push(EmbeddingSetEntry {
                            stream: record.stream.clone(),
                            record_id: record.record_id.clone(),
                            column: column.clone(),
                        });
                    }
                }
            }
        }
    }
    entries.sort();
    EmbeddingSet {
        entries,
        exclusions: exclusions.into_iter().collect(),
    }
}

/// The content class a snake_case label names (the inverse of [`class_label`]).
pub fn class_from_label(label: &str) -> Option<ContentClass> {
    serde_json::from_value(serde_json::Value::String(label.to_string())).ok()
}

/// How one content class has fared in retrieval (MEDULLA-DECISION-ENGINE-R079.5).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClassUsage {
    /// Chunks of the class currently embedded.
    pub embedded_units: usize,
    /// Of the cited span ids inside the window, how many are chunks of the class.
    pub citations: usize,
}

/// The content class of every embedded chunk, by chunk id, read from the
/// stored `DocumentEnrichment` records: a record lists its kept chunk ids and
/// one admission verdict (ordinal, class) per span, and a chunk id ends in its
/// span ordinal. No new stored property carries the class.
pub fn chunk_classes(records: &[(String, Vec<u8>)]) -> BTreeMap<String, ContentClass> {
    let mut out = BTreeMap::new();
    for (_, bytes) in records {
        let Ok(record) = medulla_types::msgpack::decode_property_value(bytes) else {
            continue;
        };
        let admissions: Vec<UnitAdmission> =
            serde_json::from_value(record["admissions"].clone()).unwrap_or_default();
        let by_ordinal: BTreeMap<u32, ContentClass> = admissions
            .iter()
            .filter_map(|a| Some((a.ordinal, class_from_label(&a.class)?)))
            .collect();
        for id in record["chunk_ids"].as_array().into_iter().flatten() {
            let Some(id) = id.as_str() else { continue };
            let ordinal = id.rsplit(':').next().and_then(|n| n.parse::<u32>().ok());
            if let Some(class) = ordinal.and_then(|o| by_ordinal.get(&o)) {
                out.insert(id.to_string(), *class);
            }
        }
    }
    out
}

/// Join the cited span ids to each chunk's class. Counts only: no content
/// leaves the store.
pub fn class_usage(
    chunk_classes: &BTreeMap<String, ContentClass>,
    cited: &BTreeSet<String>,
) -> BTreeMap<ContentClass, ClassUsage> {
    let mut usage: BTreeMap<ContentClass, ClassUsage> = BTreeMap::new();
    for class in chunk_classes.values() {
        usage.entry(*class).or_default().embedded_units += 1;
    }
    for id in cited {
        if let Some(class) = chunk_classes.get(id) {
            usage.entry(*class).or_default().citations += 1;
        }
    }
    usage
}

/// The classes retrieval has not used (MEDULLA-DECISION-ENGINE-R079.5): an
/// admitted class with no citation in the window and at least
/// `knobs.demote_min_units` embedded units. An excluded class is never listed;
/// it is never embedded in the first place.
pub fn demoted_classes(
    usage: &BTreeMap<ContentClass, ClassUsage>,
    knobs: &AdmissionKnobs,
) -> BTreeSet<ContentClass> {
    usage
        .iter()
        .filter(|(class, used)| {
            class.admits() && used.citations == 0 && used.embedded_units >= knobs.demote_min_units
        })
        .map(|(class, _)| *class)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_decision_types::decision::policy::DecisionPolicy;

    fn knobs() -> AdmissionKnobs {
        AdmissionKnobs::from_policy(&DecisionPolicy::engine_default())
    }

    fn run(class: ContentClass, text: &str, seen: &mut BTreeSet<String>) -> AdmissionOutcome {
        admit_unit(
            0,
            class,
            text,
            &BTreeSet::new(),
            &BTreeSet::new(),
            seen,
            &knobs(),
        )
        .outcome
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
            &BTreeSet::new(),
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
    fn profile(sql_type: SqlColumnType, fk: bool, ratio: f64, samples: u32) -> SqlColumnProfile {
        SqlColumnProfile {
            sql_type,
            is_foreign_key: fk,
            distinct_ratio_q32: (ratio * Q32_ONE as f64) as u64,
            sample_count: samples,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R079.4
    #[test]
    fn each_column_kind_and_the_cardinality_boundaries() {
        use ColumnExclusionReason as Why;
        let k = knobs();
        let text = |r, n| profile(SqlColumnType::Text, false, r, n);
        assert_eq!(
            admit_sql_column(&profile(SqlColumnType::Text, true, 0.9, 500), &k),
            ColumnAdmission::Excluded(Why::ForeignKey)
        );
        assert_eq!(
            admit_sql_column(&profile(SqlColumnType::Boolean, false, 0.9, 500), &k),
            ColumnAdmission::Excluded(Why::Boolean)
        );
        assert_eq!(
            admit_sql_column(&profile(SqlColumnType::Timestamp, false, 0.9, 500), &k),
            ColumnAdmission::Excluded(Why::Timestamp)
        );
        assert_eq!(
            admit_sql_column(&profile(SqlColumnType::Numeric, false, 0.9, 500), &k),
            ColumnAdmission::Excluded(Why::Numeric)
        );
        assert_eq!(
            admit_sql_column(&text(0.4, 500), &k),
            ColumnAdmission::Excluded(Why::Enumeration)
        );
        // A distinct ratio exactly at the knob is not below it.
        assert_eq!(
            admit_sql_column(&text(0.5, 500), &k),
            ColumnAdmission::Admitted
        );
        assert_eq!(
            admit_sql_column(&text(0.9, 99), &k),
            ColumnAdmission::Excluded(Why::InsufficientSamples)
        );
        assert_eq!(
            admit_sql_column(&text(0.9, 100), &k),
            ColumnAdmission::Admitted
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R079.5
    #[test]
    fn a_class_is_demoted_only_when_unused_and_at_volume() {
        let k = knobs();
        let usage = |embedded_units, citations| ClassUsage {
            embedded_units,
            citations,
        };
        let at_volume = k.demote_min_units;
        let all = BTreeMap::from([
            (ContentClass::Prose, usage(at_volume, 0)),
            (ContentClass::Document, usage(at_volume - 1, 0)),
            (ContentClass::Comment, usage(at_volume * 2, 1)),
            (ContentClass::Generated, usage(at_volume * 2, 0)),
        ]);
        assert_eq!(
            demoted_classes(&all, &k),
            BTreeSet::from([ContentClass::Prose]),
            "volume boundary, a single citation and an excluded class"
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R079.5
    #[test]
    fn a_demoted_class_skips_fresh_spans_but_keeps_exact_reuse_and_other_classes() {
        let demoted = BTreeSet::from([ContentClass::Prose]);
        let outcome = |class, known: &BTreeSet<String>| {
            admit_unit(
                0,
                class,
                PROSE,
                known,
                &demoted,
                &mut BTreeSet::new(),
                &knobs(),
            )
            .outcome
        };
        assert_eq!(
            outcome(ContentClass::Prose, &BTreeSet::new()),
            AdmissionOutcome::Demoted
        );
        assert_eq!(
            outcome(ContentClass::Document, &BTreeSet::new()),
            AdmissionOutcome::Admitted
        );
        assert_eq!(
            outcome(ContentClass::Prose, &BTreeSet::from([exact_digest(PROSE)])),
            AdmissionOutcome::SkippedExact
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R079.5
    #[test]
    fn usage_joins_cited_ids_to_the_class_recorded_for_each_chunk() {
        let record = |ids: &[&str], classes: &[&str]| {
            let admissions: Vec<_> = classes
                .iter()
                .enumerate()
                .map(|(n, class)| {
                    serde_json::json!({"ordinal": n, "class": class, "outcome": "admitted"})
                })
                .collect();
            let value = serde_json::json!({"chunk_ids": ids, "admissions": admissions});
            (
                "enrichment:x".to_string(),
                rmp_serde::to_vec_named(&value).unwrap(),
            )
        };
        let records = vec![
            record(
                &["chunk:a:0123:0", "chunk:a:0123:1"],
                &["prose", "document"],
            ),
            record(&["chunk:b:4567:0"], &["prose"]),
        ];
        let classes = chunk_classes(&records);
        assert_eq!(classes.len(), 3);
        let cited = BTreeSet::from(["chunk:a:0123:1".to_string(), "chunk:gone:0:0".to_string()]);
        let usage = class_usage(&classes, &cited);
        assert_eq!(
            usage[&ContentClass::Prose],
            ClassUsage {
                embedded_units: 2,
                citations: 0
            }
        );
        assert_eq!(
            usage[&ContentClass::Document],
            ClassUsage {
                embedded_units: 1,
                citations: 1
            }
        );
    }
}
