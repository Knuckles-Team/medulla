//! Claim-text policy check (MEDULLA-BUSINESS-OPS-R002.6).
//!
//! Kind `phrase_policy`. Input: a text, its locale and a `:ClaimPolicy` revision. The text
//! is split into sentences, each sentence is a claim, and each is normalized (case folded,
//! full-width forms folded to their ordinary width, whitespace collapsed) before the
//! policy's phrase sets are matched by their declared mode: exact (anywhere in the
//! sentence), word (bounded by non-letters) or stem (word by word after a suffix fold).
//! Output: `fail` with each forbidden span; `pass` only when no forbidden phrase matches
//! and every claim sentence carries an approved phrase; otherwise `needs_review` with the
//! claim sentences no approved phrase covers. The result names the policy revision.
//! Only phrases whose locale is empty or shares the language of the text apply.

use super::rule_spec::{RuleRefusal, RuleTrace};

/// The refusal code for a text over [`MAX_TEXT_BYTES`].
pub const PHRASE_TEXT_TOO_LARGE: &str = "PHRASE_TEXT_TOO_LARGE";
/// The refusal code for a policy with more than [`MAX_PHRASES`] phrases or an empty phrase.
pub const PHRASE_POLICY_INVALID: &str = "PHRASE_POLICY_INVALID";
/// The refusal code for a match mode that is not exact, word or stem.
pub const PHRASE_MODE_INVALID: &str = "PHRASE_MODE_INVALID";

/// The largest text one check reads.
pub const MAX_TEXT_BYTES: usize = 1 << 20;
/// The most phrases one policy holds.
pub const MAX_PHRASES: usize = 10_000;

/// How a phrase matches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The phrase occurs anywhere in the sentence.
    Exact,
    /// The phrase occurs bounded by non-letters.
    Word,
    /// The phrase occurs word by word after the suffix fold.
    Stem,
}

impl Mode {
    /// The mode with the stored name `name`.
    pub fn parse(name: &str) -> Result<Self, RuleRefusal> {
        match name {
            "exact" => Ok(Self::Exact),
            "word" => Ok(Self::Word),
            "stem" => Ok(Self::Stem),
            other => Err(RuleRefusal::new(
                PHRASE_MODE_INVALID,
                format!("`{other}` is not exact, word or stem"),
            )),
        }
    }
}

/// One approved or forbidden phrase.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Phrase {
    /// The phrase text.
    pub text: String,
    /// The locale the phrase applies to; empty applies to every locale.
    pub locale: String,
    /// How it matches.
    pub mode: Mode,
}

/// A `:ClaimPolicy` revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    /// The revision that was read.
    pub revision: String,
    /// The phrases the policy forbids.
    pub forbidden: Vec<Phrase>,
    /// The phrases the policy approves.
    pub approved: Vec<Phrase>,
}

/// The verdict on one text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// No forbidden phrase and every claim approved.
    Pass,
    /// A forbidden phrase matched.
    Fail,
    /// No forbidden phrase, but a claim no approved phrase covers.
    NeedsReview,
}

impl Verdict {
    /// The wire name: `pass`, `fail` or `needs_review`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::NeedsReview => "needs_review",
        }
    }
}

/// One forbidden span.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    /// The index of the claim sentence.
    pub sentence: usize,
    /// The byte offset in the normalized sentence where the match starts.
    pub start: usize,
    /// The byte offset in the normalized sentence where the match ends.
    pub end: usize,
    /// The forbidden phrase that matched.
    pub phrase: String,
}

/// The result of one check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhraseVerdict {
    /// The verdict.
    pub verdict: Verdict,
    /// Each forbidden span.
    pub forbidden: Vec<Span>,
    /// The normalized claim sentences no approved phrase covers; empty on `fail`.
    pub unmatched: Vec<String>,
    /// The policy revision the check read.
    pub policy_revision: String,
    /// The rule digest, the input nodes and the steps.
    pub trace: RuleTrace,
}

fn fold_char(c: char) -> char {
    match c {
        '\u{3000}' => ' ',
        '\u{3002}' => '.',
        '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
        other => other,
    }
}

/// Case fold, fold full-width forms and collapse whitespace.
pub fn normalize(text: &str) -> String {
    let folded: String = text
        .chars()
        .map(fold_char)
        .flat_map(char::to_lowercase)
        .collect();
    folded.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn stem(word: &str) -> &str {
    for suffix in ["edly", "ing", "ed", "es", "ly", "s"] {
        if let Some(root) = word.strip_suffix(suffix) {
            if root.chars().count() >= 3 {
                return root;
            }
        }
    }
    word
}

/// The words of a normalized sentence with their byte offsets.
fn words(sentence: &str) -> Vec<(usize, usize, &str)> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (index, c) in sentence.char_indices() {
        if c.is_alphanumeric() {
            start.get_or_insert(index);
        } else if let Some(begin) = start.take() {
            out.push((begin, index, &sentence[begin..index]));
        }
    }
    if let Some(begin) = start {
        out.push((begin, sentence.len(), &sentence[begin..]));
    }
    out
}

fn language(locale: &str) -> String {
    normalize(locale)
        .split(['-', '_'])
        .next()
        .unwrap_or_default()
        .to_string()
}

fn applies_in(phrase: &Phrase, text_language: &str) -> bool {
    phrase.locale.is_empty() || language(&phrase.locale) == text_language
}

/// Every `(start, end)` of `phrase` in the normalized `sentence`.
fn find(sentence: &str, phrase: &Phrase) -> Vec<(usize, usize)> {
    let needle = normalize(&phrase.text);
    if needle.is_empty() {
        return Vec::new();
    }
    match phrase.mode {
        Mode::Exact => sentence
            .match_indices(&needle)
            .map(|(start, found)| (start, start + found.len()))
            .collect(),
        Mode::Word => sentence
            .match_indices(&needle)
            .map(|(start, found)| (start, start + found.len()))
            .filter(|&(start, end)| {
                let before = sentence[..start].chars().next_back();
                let after = sentence[end..].chars().next();
                !before.is_some_and(char::is_alphanumeric)
                    && !after.is_some_and(char::is_alphanumeric)
            })
            .collect(),
        Mode::Stem => {
            let text_words = words(sentence);
            let needle_words = words(&needle);
            if needle_words.is_empty() || needle_words.len() > text_words.len() {
                return Vec::new();
            }
            text_words
                .windows(needle_words.len())
                .filter(|window| {
                    window
                        .iter()
                        .zip(&needle_words)
                        .all(|(a, b)| stem(a.2) == stem(b.2))
                })
                .map(|window| (window[0].0, window[window.len() - 1].1))
                .collect()
        }
    }
}

fn sentences(text: &str) -> Vec<String> {
    text.split(|c: char| matches!(c, '.' | '!' | '?' | '\n' | '\u{3002}' | '\u{FF01}' | '\u{FF1F}'))
        .map(normalize)
        .filter(|sentence| !sentence.is_empty())
        .collect()
}

/// Check `text` in `locale` against `policy`. `trace` carries the rule digest and the policy
/// node; the check's steps are appended to it.
pub fn check_text(
    text: &str,
    locale: &str,
    policy: &Policy,
    mut trace: RuleTrace,
) -> Result<PhraseVerdict, RuleRefusal> {
    if text.len() > MAX_TEXT_BYTES {
        return Err(RuleRefusal::new(
            PHRASE_TEXT_TOO_LARGE,
            format!("the text holds {} bytes; the ceiling is {MAX_TEXT_BYTES}", text.len()),
        ));
    }
    if policy.forbidden.len() + policy.approved.len() > MAX_PHRASES
        || policy
            .forbidden
            .iter()
            .chain(&policy.approved)
            .any(|phrase| normalize(&phrase.text).is_empty())
    {
        return Err(RuleRefusal::new(
            PHRASE_POLICY_INVALID,
            format!("a policy holds at most {MAX_PHRASES} non-empty phrases"),
        ));
    }
    let text_language = language(locale);
    let forbidden: Vec<&Phrase> = policy
        .forbidden
        .iter()
        .filter(|phrase| applies_in(phrase, &text_language))
        .collect();
    let approved: Vec<&Phrase> = policy
        .approved
        .iter()
        .filter(|phrase| applies_in(phrase, &text_language))
        .collect();
    let claims = sentences(text);
    trace.step(format!(
        "{} claim sentences; {} forbidden and {} approved phrases apply in `{text_language}`",
        claims.len(),
        forbidden.len(),
        approved.len()
    ));

    let mut spans = Vec::new();
    let mut unmatched = Vec::new();
    for (index, claim) in claims.iter().enumerate() {
        for phrase in &forbidden {
            for (start, end) in find(claim, phrase) {
                spans.push(Span {
                    sentence: index,
                    start,
                    end,
                    phrase: normalize(&phrase.text),
                });
            }
        }
        if !approved.iter().any(|phrase| !find(claim, phrase).is_empty()) {
            unmatched.push(claim.clone());
        }
    }
    spans.sort_by(|a, b| (a.sentence, a.start, a.end, &a.phrase).cmp(&(b.sentence, b.start, b.end, &b.phrase)));
    let verdict = if !spans.is_empty() {
        unmatched.clear();
        Verdict::Fail
    } else if unmatched.is_empty() {
        Verdict::Pass
    } else {
        Verdict::NeedsReview
    };
    trace.step(format!(
        "verdict {} under policy revision {}",
        verdict.as_str(),
        policy.revision
    ));
    Ok(PhraseVerdict {
        verdict,
        forbidden: spans,
        unmatched,
        policy_revision: policy.revision.clone(),
        trace,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::business::rule_spec::{load, InputRef};
    use std::collections::BTreeMap;

    fn trace() -> RuleTrace {
        RuleTrace::new(
            &load("phrase_policy", 1, BTreeMap::new()).unwrap(),
            vec![InputRef::new("policy:c1", 5)],
        )
    }

    fn phrase(text: &str, mode: Mode) -> Phrase {
        Phrase {
            text: text.to_string(),
            locale: "en".to_string(),
            mode,
        }
    }

    fn policy() -> Policy {
        Policy {
            revision: "rev-5".to_string(),
            forbidden: vec![
                phrase("cures cancer", Mode::Exact),
                phrase("guaranteed", Mode::Word),
                phrase("whiten", Mode::Stem),
            ],
            approved: vec![
                phrase("supports healthy skin", Mode::Exact),
                phrase("made in small batches", Mode::Exact),
            ],
        }
    }

    // spec: MEDULLA-BUSINESS-OPS-R002.6
    #[test]
    fn forbidden_fails_unknown_needs_review_approved_passes() {
        // Approved claims pass; case, width and spacing are folded.
        let approved = check_text(
            "Supports  Healthy Skin. Made in small batches!",
            "en-US",
            &policy(),
            trace(),
        )
        .unwrap();
        assert_eq!(approved.verdict, Verdict::Pass);
        assert!(approved.forbidden.is_empty());
        assert!(approved.unmatched.is_empty());
        assert_eq!(approved.policy_revision, "rev-5");
        assert_eq!(approved.verdict.as_str(), "pass");
        assert_eq!(
            approved.trace.rule_digest,
            load("phrase_policy", 1, BTreeMap::new()).unwrap().digest
        );

        // Full-width letters fold to ordinary width.
        let wide = check_text(
            "\u{FF33}upports healthy skin",
            "en",
            &policy(),
            trace(),
        )
        .unwrap();
        assert_eq!(wide.verdict, Verdict::Pass);

        // A claim that is neither approved nor forbidden needs review and is listed.
        let unknown = check_text(
            "Supports healthy skin. Reduces redness in a week.",
            "en",
            &policy(),
            trace(),
        )
        .unwrap();
        assert_eq!(unknown.verdict, Verdict::NeedsReview);
        assert_eq!(unknown.verdict.as_str(), "needs_review");
        assert_eq!(unknown.unmatched, ["reduces redness in a week"]);
        assert!(unknown.forbidden.is_empty());

        // A forbidden phrase fails and names each span.
        let forbidden = check_text(
            "Supports healthy skin. It CURES cancer and is guaranteed.",
            "en",
            &policy(),
            trace(),
        )
        .unwrap();
        assert_eq!(forbidden.verdict, Verdict::Fail);
        assert_eq!(forbidden.verdict.as_str(), "fail");
        assert_eq!(forbidden.forbidden.len(), 2);
        assert_eq!(forbidden.forbidden[0].phrase, "cures cancer");
        assert_eq!(forbidden.forbidden[0].sentence, 1);
        assert_eq!(forbidden.forbidden[1].phrase, "guaranteed");
        let sentence = "it cures cancer and is guaranteed";
        let first = &forbidden.forbidden[0];
        assert_eq!(&sentence[first.start..first.end], "cures cancer");
        let second = &forbidden.forbidden[1];
        assert_eq!(&sentence[second.start..second.end], "guaranteed");
        assert!(forbidden.unmatched.is_empty());

        // Word mode is bounded by non-letters: "guaranteedly" is not the word.
        let bounded = check_text("Supports healthy skin guaranteedly.", "en", &policy(), trace()).unwrap();
        assert!(bounded.forbidden.is_empty());

        // Stem mode matches word forms: "whitening" and "whitens" are "whiten".
        let stemmed = check_text("Whitening teeth in a day.", "en", &policy(), trace()).unwrap();
        assert_eq!(stemmed.verdict, Verdict::Fail);
        assert_eq!(stemmed.forbidden[0].phrase, "whiten");

        // A phrase of another language does not apply.
        let german = check_text("Garantiert wirksam.", "de-DE", &policy(), trace()).unwrap();
        assert!(german.forbidden.is_empty());
        assert_eq!(german.verdict, Verdict::NeedsReview);

        // An empty text has no claim to review.
        let empty = check_text("  ", "en", &policy(), trace()).unwrap();
        assert_eq!(empty.verdict, Verdict::Pass);

        // The same inputs give the same result.
        assert_eq!(
            check_text("It cures cancer.", "en", &policy(), trace()).unwrap(),
            check_text("It cures cancer.", "en", &policy(), trace()).unwrap()
        );

        // Refusals.
        assert_eq!(Mode::parse("fuzzy").unwrap_err().code, PHRASE_MODE_INVALID);
        assert_eq!(Mode::parse("stem"), Ok(Mode::Stem));
        let too_big = "a".repeat(MAX_TEXT_BYTES + 1);
        assert_eq!(
            check_text(&too_big, "en", &policy(), trace()).unwrap_err().code,
            PHRASE_TEXT_TOO_LARGE
        );
        let mut blank = policy();
        blank.forbidden.push(phrase("   ", Mode::Exact));
        assert_eq!(
            check_text("x", "en", &blank, trace()).unwrap_err().code,
            PHRASE_POLICY_INVALID
        );
    }
}
