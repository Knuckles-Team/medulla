//! Feature matching, canonical names and deduplication
//! (MEDULLA-DECISION-ENGINE-R707.2).
//!
//! Pure and deterministic: integers only (parts per million, millibits), no
//! platform float transcendental, no hash-map iteration order. A pair scores
//! only inside a block of the same ontology label, and a block is capped at
//! `match.max_block` features, taken in feature id order.
//!
//! A pair score is `w_token * jaccard + w_label * label + w_embedding * cosine`
//! on parts per million, where the embedding cosine arrives as an input. A name
//! whose Shannon entropy reaches `match.entropy_mbit` is too random to say
//! anything on its own (a hash, a generated id): its token similarity counts as
//! zero, so such a name never merges two features without the other evidence.

use std::collections::{BTreeMap, BTreeSet};

use medulla_decision_types::decision::feature_match::{
    FeatureMatrixRow, LedgerEntry, LedgerEvent, Match,
};
use medulla_types::transition_refused::TransitionRefused;

/// One part per million: similarity `1.0`.
pub const PPM: u32 = 1_000_000;

/// The registry knobs of matching (`match.*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchParams {
    pub w_token: u32,
    pub w_label: u32,
    pub w_embedding: u32,
    pub max_block: usize,
    pub score_min: u32,
    pub entropy_mbit: u32,
}

impl MatchParams {
    /// The weights must sum to one million.
    pub fn validate(&self) -> Result<(), String> {
        let sum = u64::from(self.w_token) + u64::from(self.w_label) + u64::from(self.w_embedding);
        if sum == u64::from(PPM) {
            Ok(())
        } else {
            Err(format!(
                "match weights sum to {sum} ppm, not {PPM}: w_token, w_label and w_embedding"
            ))
        }
    }
}

/// Lower-case `name` and collapse every run of separators (anything that is
/// not a letter or digit) into one `_`, with none at the ends.
pub fn canonical_id(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut pending = false;
    for ch in name.chars() {
        if ch.is_alphanumeric() {
            if pending && !out.is_empty() {
                out.push('_');
            }
            pending = false;
            out.extend(ch.to_lowercase());
        } else {
            pending = true;
        }
    }
    out
}

/// `log2(x)` on Q20 (`x > 0`), by integer squaring: exact enough for entropy
/// and identical on every platform.
fn log2_q20(x: u64) -> i128 {
    let k = 63 - x.leading_zeros();
    let mut m: u128 = if k <= 60 {
        u128::from(x) << (60 - k)
    } else {
        u128::from(x) >> (k - 60)
    };
    let mut frac: u128 = 0;
    for bit in (0..20).rev() {
        m = (m * m) >> 60;
        if m >= (2_u128 << 60) {
            frac |= 1 << bit;
            m >>= 1;
        }
    }
    ((i128::from(k)) << 20) | frac as i128
}

/// Shannon entropy of `text`'s characters, in millibits per character.
pub fn entropy_mbit(text: &str) -> u32 {
    let mut counts: BTreeMap<char, u64> = BTreeMap::new();
    for ch in text.chars() {
        *counts.entry(ch).or_default() += 1;
    }
    let n: u64 = counts.values().sum();
    if n < 2 {
        return 0;
    }
    let weighted: i128 = counts.values().map(|c| i128::from(*c) * log2_q20(*c)).sum();
    let h_q20 = log2_q20(n) - weighted / i128::from(n);
    ((h_q20.max(0) * 1000) >> 20) as u32
}

/// A feature prepared once: its canonical name, tokens and entropy.
struct Prepared<'a> {
    row: &'a FeatureMatrixRow,
    tokens: BTreeSet<String>,
    random_name: bool,
}

fn prepare<'a>(row: &'a FeatureMatrixRow, params: &MatchParams) -> Prepared<'a> {
    let canonical = canonical_id(&row.name);
    Prepared {
        row,
        tokens: canonical.split('_').map(str::to_string).collect(),
        random_name: entropy_mbit(&canonical) >= params.entropy_mbit,
    }
}

fn jaccard_ppm(a: &BTreeSet<String>, b: &BTreeSet<String>) -> u32 {
    let union = a.union(b).count() as u64;
    if union == 0 {
        return 0;
    }
    let shared = a.intersection(b).count() as u64;
    (shared * u64::from(PPM) / union) as u32
}

fn score(a: &Prepared<'_>, b: &Prepared<'_>, embedding_ppm: u32, params: &MatchParams) -> u32 {
    let token = if a.random_name || b.random_name {
        0
    } else {
        jaccard_ppm(&a.tokens, &b.tokens)
    };
    let label = if !a.row.label.is_empty() && a.row.label == b.row.label {
        PPM
    } else {
        0
    };
    let total = u64::from(params.w_token) * u64::from(token)
        + u64::from(params.w_label) * u64::from(label)
        + u64::from(params.w_embedding) * u64::from(embedding_ppm.min(PPM));
    (total / u64::from(PPM)).min(u64::from(PPM)) as u32
}

/// The score of one pair, on parts per million. `embedding_ppm` is the cosine
/// of the two features' embeddings, clamped to `0..=1_000_000`.
pub fn score_pair(
    a: &FeatureMatrixRow,
    b: &FeatureMatrixRow,
    embedding_ppm: u32,
    params: &MatchParams,
) -> u32 {
    score(
        &prepare(a, params),
        &prepare(b, params),
        embedding_ppm,
        params,
    )
}

/// What deduplication found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dedup {
    /// Every scored pair at or above `match.score_min`, in feature id order.
    pub matches: Vec<Match>,
    /// The features that merge, each cluster in feature id order, clusters by
    /// their first id. A feature that matches nothing is not listed.
    pub clusters: Vec<Vec<String>>,
    /// Features past `match.max_block` in their block, which were not scored.
    pub unscored: u64,
}

fn find(parent: &mut BTreeMap<String, String>, id: &str) -> String {
    let mut root = id.to_string();
    while let Some(next) = parent.get(&root).filter(|p| **p != root) {
        root = next.clone();
    }
    root
}

/// Score every pair inside each same-label block and merge the pairs at or
/// above `match.score_min`. `embedding_ppm(a, b)` is the cosine of two feature
/// ids' embeddings.
pub fn dedup(
    rows: &[FeatureMatrixRow],
    embedding_ppm: &dyn Fn(&str, &str) -> u32,
    params: &MatchParams,
) -> Dedup {
    let mut blocks: BTreeMap<&str, Vec<&FeatureMatrixRow>> = BTreeMap::new();
    for row in rows {
        blocks.entry(row.label.as_str()).or_default().push(row);
    }
    let mut matches = Vec::new();
    let mut unscored = 0_u64;
    let mut parent: BTreeMap<String, String> = BTreeMap::new();
    for block in blocks.values_mut() {
        block.sort_by(|a, b| a.feature_id.cmp(&b.feature_id));
        block.dedup_by(|a, b| a.feature_id == b.feature_id);
        if block.len() > params.max_block {
            unscored += (block.len() - params.max_block) as u64;
            block.truncate(params.max_block);
        }
        let prepared: Vec<Prepared<'_>> = block.iter().map(|r| prepare(r, params)).collect();
        for (i, left) in prepared.iter().enumerate() {
            for right in &prepared[i + 1..] {
                let embedding = embedding_ppm(&left.row.feature_id, &right.row.feature_id);
                let value = score(left, right, embedding, params);
                if value < params.score_min {
                    continue;
                }
                matches.push(Match {
                    local: left.row.feature_id.clone(),
                    external: right.row.feature_id.clone(),
                    score_ppm: value,
                });
                for id in [&left.row.feature_id, &right.row.feature_id] {
                    parent.entry(id.clone()).or_insert_with(|| id.clone());
                }
                let a = find(&mut parent, &left.row.feature_id);
                let b = find(&mut parent, &right.row.feature_id);
                if a != b {
                    let (low, high) = if a < b { (a, b) } else { (b, a) };
                    parent.insert(high, low);
                }
            }
        }
    }
    matches.sort_by(|a, b| (&a.local, &a.external).cmp(&(&b.local, &b.external)));
    let ids: Vec<String> = parent.keys().cloned().collect();
    let mut grouped: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for id in ids {
        let root = find(&mut parent, &id);
        grouped.entry(root).or_default().push(id);
    }
    Dedup {
        matches,
        clusters: grouped.into_values().collect(),
        unscored,
    }
}

/// Why a ledger status change was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LedgerChangeError {
    /// The caller read another revision than the entry holds.
    RevisionConflict { expected: u64, found: u64 },
    /// The machine refused the event from the entry's status.
    Transition(TransitionRefused),
}

/// Change a ledger entry's status by compare-and-set on `revision`: the entry
/// moves only when its revision is the one the caller read, through the one
/// transition function, and its revision advances by one. Nothing else about
/// the entry changes.
pub fn advance_ledger(
    entry: &LedgerEntry,
    expected_revision: u64,
    event: LedgerEvent,
) -> Result<LedgerEntry, LedgerChangeError> {
    if entry.revision != expected_revision {
        return Err(LedgerChangeError::RevisionConflict {
            expected: expected_revision,
            found: entry.revision,
        });
    }
    let status = entry
        .status
        .apply(event)
        .map_err(LedgerChangeError::Transition)?;
    Ok(LedgerEntry {
        status,
        revision: entry.revision + 1,
        ..entry.clone()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_decision_types::decision::feature_match::LedgerStatus;

    const PARAMS: MatchParams = MatchParams {
        w_token: 400_000,
        w_label: 300_000,
        w_embedding: 300_000,
        max_block: 256,
        score_min: 600_000,
        entropy_mbit: 3_500,
    };

    fn row(id: &str, project: &str, name: &str, label: &str) -> FeatureMatrixRow {
        FeatureMatrixRow {
            feature_id: id.into(),
            project: project.into(),
            name: name.into(),
            label: label.into(),
            token_digest: String::new(),
        }
    }

    fn no_embedding(_: &str, _: &str) -> u32 {
        0
    }

    // spec: MEDULLA-DECISION-ENGINE-R707.2
    #[test]
    fn blocked_scores_and_entropy_guard() {
        assert!(PARAMS.validate().is_ok());
        assert!(MatchParams {
            w_token: 500_000,
            ..PARAMS
        }
        .validate()
        .is_err());

        // canonical_id lowercases and collapses separators.
        assert_eq!(
            canonical_id("  Vector--Search_Index.v2 "),
            "vector_search_index_v2"
        );
        assert_eq!(canonical_id("---"), "");

        // Entropy: a repeated letter is 0, four equiprobable letters are 2 bits.
        assert_eq!(entropy_mbit("aaaaaaaa"), 0);
        assert!((1_990..=2_000).contains(&entropy_mbit("abcdabcd")));
        assert!(entropy_mbit("x9f3kq7z2m8c4v1b") >= PARAMS.entropy_mbit);

        // Pair score: identical names with one label score token + label.
        let a = row("a", "p1", "Vector Search", "ret");
        let b = row("b", "p2", "vector-search", "ret");
        assert_eq!(score_pair(&a, &b, 0, &PARAMS), 700_000);
        assert_eq!(score_pair(&a, &b, PPM, &PARAMS), PPM);
        // Half the tokens shared: jaccard 1/3.
        let c = row("c", "p2", "vector index", "ret");
        assert_eq!(
            score_pair(&a, &c, 0, &PARAMS),
            400_000 * 333_333 / PPM + 300_000
        );
        // A different label gets no label credit.
        let d = row("d", "p2", "vector search", "other");
        assert_eq!(score_pair(&a, &d, 0, &PARAMS), 400_000);

        // A random-looking name never merges on the name alone.
        let h1 = row("h1", "p1", "x9f3kq7z2m8c4v1b", "ret");
        let h2 = row("h2", "p2", "x9f3kq7z2m8c4v1b", "ret");
        assert_eq!(score_pair(&h1, &h2, 0, &PARAMS), 300_000);
        assert!(dedup(&[h1.clone(), h2.clone()], &no_embedding, &PARAMS)
            .matches
            .is_empty());
        let with_embedding = dedup(&[h1, h2], &|_, _| PPM, &PARAMS);
        assert_eq!(with_embedding.matches.len(), 1);

        // 10 features: only same-label, same-name pairs merge.
        let ten: Vec<_> = (0..10)
            .map(|i| row(&format!("f{i}"), "p", &format!("feature {}", i % 5), "ret"))
            .collect();
        let found = dedup(&ten, &no_embedding, &PARAMS);
        assert_eq!(found.matches.len(), 5);
        assert_eq!(found.clusters.len(), 5);
        assert!(found.clusters.iter().all(|c| c.len() == 2));
        assert_eq!(found.unscored, 0);

        // 1,000 features in one block: only the first 256 by id are scored.
        let block: Vec<_> = (0..1_000)
            .map(|i| row(&format!("g{i:04}"), "p", &format!("name {i}"), "ret"))
            .collect();
        let capped = dedup(&block, &no_embedding, &PARAMS);
        assert_eq!(capped.unscored, 744);
        assert!(capped
            .matches
            .iter()
            .all(|m| m.local.as_str() < "g0256" && m.external.as_str() < "g0256"));

        // 100,000 features across 5,000 labels, twenty per label, two names each.
        let many: Vec<_> = (0..100_000)
            .map(|i| {
                row(
                    &format!("m{i:06}"),
                    "p",
                    &format!("capability {}", (i / 5_000) % 2),
                    &format!("label{}", i % 5_000),
                )
            })
            .collect();
        let big = dedup(&many, &no_embedding, &PARAMS);
        assert_eq!(big.unscored, 0);
        // Per label, the 20 features hold two names, ten each: two clusters.
        assert_eq!(big.clusters.len(), 5_000 * 2);
        assert!(big.clusters.iter().all(|c| c.len() == 10));
        // The result does not depend on the order the rows arrive in.
        let mut reversed = many.clone();
        reversed.reverse();
        assert_eq!(dedup(&reversed, &no_embedding, &PARAMS), big);
    }

    // spec: MEDULLA-DECISION-ENGINE-R707.2
    #[test]
    fn status_changes_by_compare_and_set_on_revision() {
        let entry = LedgerEntry {
            feature_id: "f".into(),
            status: LedgerStatus::Open,
            closed_by: None,
            revision: 4,
            match_ids: vec!["m".into()],
            plan_id: None,
        };
        let planned = advance_ledger(&entry, 4, LedgerEvent::Plan).unwrap();
        assert_eq!(
            (planned.status, planned.revision),
            (LedgerStatus::Planned, 5)
        );
        assert_eq!(planned.match_ids, entry.match_ids);
        assert_eq!(
            advance_ledger(&entry, 3, LedgerEvent::Plan),
            Err(LedgerChangeError::RevisionConflict {
                expected: 3,
                found: 4
            })
        );
        assert!(matches!(
            advance_ledger(&entry, 4, LedgerEvent::Close),
            Err(LedgerChangeError::Transition(_))
        ));
    }
}
