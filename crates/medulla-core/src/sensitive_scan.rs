//! Column scan for sensitive values (MEDULLA-TYPED-PACKS-R803.2).
//!
//! Pure functions over `medulla_query_types::sensitive_detect`: a bounded sample of one
//! column's values is matched against each detector and the hits are counted. No I/O, no
//! clock read, no store. No value, fragment or hash of a value is kept in any output or
//! error: a detection holds only counts.

use std::collections::BTreeSet;

use medulla_query_types::sensitive_detect::{
    ChecksumAlgorithm, Detection, DetectorRule, InvalidDetector, SensitiveDetector,
};
use regex::{Regex, RegexBuilder};

/// The refusal code of a `Dictionary` or `Model` rule whose stored resource is absent.
pub const DETECTOR_RESOURCE_MISSING: &str = "DETECTOR_RESOURCE_MISSING";
/// The refusal code of scan limits outside their knob ranges.
pub const SCAN_LIMITS_INVALID: &str = "SCAN_LIMITS_INVALID";

/// `knob sensitive.max_sample_values` range.
const MAX_SAMPLE_VALUES_RANGE: (usize, usize) = (100, 1_000_000);
/// `knob sensitive.regex_size_limit` range, bytes.
const REGEX_SIZE_LIMIT_RANGE: (usize, usize) = (65_536, 16_777_216);
/// `knob sensitive.model_sample_values` range.
const MODEL_SAMPLE_VALUES_RANGE: (usize, usize) = (10, 5_000);

/// The bounds of one scan. The caller reads the knobs and builds the struct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanLimits {
    /// `sensitive.max_sample_values`: non-null values sampled per column.
    pub max_sample_values: usize,
    /// `sensitive.regex_size_limit`: compiled size bound of one pattern, bytes.
    pub regex_size_limit: usize,
    /// `sensitive.model_sample_values`: sampled values a `Model` rule scores.
    pub model_sample_values: usize,
}

impl Default for ScanLimits {
    fn default() -> Self {
        Self {
            max_sample_values: 10_000,
            regex_size_limit: 1_048_576,
            model_sample_values: 200,
        }
    }
}

impl ScanLimits {
    /// Refuse a limit outside its knob range.
    pub fn validate(&self) -> Result<(), ScanError> {
        let within = |value: usize, (low, high): (usize, usize)| (low..=high).contains(&value);
        if !within(self.max_sample_values, MAX_SAMPLE_VALUES_RANGE) {
            return Err(ScanError::LimitsInvalid {
                knob: "sensitive.max_sample_values",
            });
        }
        if !within(self.regex_size_limit, REGEX_SIZE_LIMIT_RANGE) {
            return Err(ScanError::LimitsInvalid {
                knob: "sensitive.regex_size_limit",
            });
        }
        if !within(self.model_sample_values, MODEL_SAMPLE_VALUES_RANGE) {
            return Err(ScanError::LimitsInvalid {
                knob: "sensitive.model_sample_values",
            });
        }
        Ok(())
    }
}

/// The stored resources a `Dictionary` or `Model` rule reads.
pub trait DetectorResources {
    /// The stored term set of a `Dictionary` rule; terms are already case-folded.
    fn dictionary(&self, terms_digest: &str) -> Option<&BTreeSet<String>>;
    /// The cosine of `value` to the nearest exemplar of a `Model` rule's set.
    fn max_exemplar_cosine(&self, exemplar_set: &str, value: &str) -> Option<f32>;
}

/// No stored resources: every `Dictionary` and `Model` rule is missing its resource.
pub struct NoResources;

impl DetectorResources for NoResources {
    fn dictionary(&self, _terms_digest: &str) -> Option<&BTreeSet<String>> {
        None
    }

    fn max_exemplar_cosine(&self, _exemplar_set: &str, _value: &str) -> Option<f32> {
        None
    }
}

/// Why a scan was refused. Nothing is scanned when any detector or limit is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanError {
    /// A detector configuration is invalid.
    Detector(InvalidDetector),
    /// A `Dictionary` or `Model` rule names a resource the caller does not hold.
    ResourceMissing { detector: String },
    /// A scan limit is outside its knob range.
    LimitsInvalid { knob: &'static str },
}

impl ScanError {
    /// The stable refusal code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Detector(invalid) => invalid.code(),
            Self::ResourceMissing { .. } => DETECTOR_RESOURCE_MISSING,
            Self::LimitsInvalid { .. } => SCAN_LIMITS_INVALID,
        }
    }
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Detector(invalid) => invalid.fmt(f),
            Self::ResourceMissing { detector } => {
                write!(f, "{DETECTOR_RESOURCE_MISSING}: {detector}")
            }
            Self::LimitsInvalid { knob } => write!(f, "{SCAN_LIMITS_INVALID}: {knob}"),
        }
    }
}

impl std::error::Error for ScanError {}

impl From<InvalidDetector> for ScanError {
    fn from(invalid: InvalidDetector) -> Self {
        Self::Detector(invalid)
    }
}

/// What a detector needs to test one value, resolved before anything is scanned.
enum Matcher<'a> {
    /// The pattern, anchored to the whole value.
    Pattern(Regex),
    Checksum(ChecksumAlgorithm),
    Dictionary(&'a BTreeSet<String>),
    Model(&'a str),
}

fn luhn(value: &str) -> bool {
    let digits: Vec<u32> = value
        .chars()
        .filter(|c| *c != ' ' && *c != '-')
        .map(|c| c.to_digit(10).filter(|_| c.is_ascii_digit()))
        .collect::<Option<_>>()
        .unwrap_or_default();
    if !(12..=19).contains(&digits.len()) {
        return false;
    }
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(index, digit)| {
            if index % 2 == 1 {
                let doubled = digit * 2;
                if doubled > 9 {
                    doubled - 9
                } else {
                    doubled
                }
            } else {
                *digit
            }
        })
        .sum();
    sum % 10 == 0
}

fn iban_mod97(value: &str) -> bool {
    let compact: Vec<char> = value
        .chars()
        .filter(|c| *c != ' ')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if !(15..=34).contains(&compact.len()) || !compact.iter().all(char::is_ascii_alphanumeric) {
        return false;
    }
    if !compact[..2].iter().all(char::is_ascii_alphabetic)
        || !compact[2..4].iter().all(char::is_ascii_digit)
    {
        return false;
    }
    let mut remainder: u32 = 0;
    for c in compact[4..].iter().chain(compact[..4].iter()) {
        remainder = match c.to_digit(36) {
            Some(number) if number >= 10 => (remainder * 100 + number) % 97,
            Some(number) => (remainder * 10 + number) % 97,
            None => return false,
        };
    }
    remainder == 1
}

fn checksum_hit(algorithm: ChecksumAlgorithm, value: &str) -> bool {
    match algorithm {
        ChecksumAlgorithm::Luhn => luhn(value),
        ChecksumAlgorithm::IbanMod97 => iban_mod97(value),
    }
}

fn compile_pattern(
    detector: &SensitiveDetector,
    regex: &str,
    limits: &ScanLimits,
) -> Result<Regex, InvalidDetector> {
    let invalid = |reason: String| InvalidDetector::PatternInvalid {
        detector: detector.id.clone(),
        reason,
    };
    // The pattern as written must compile; the value then has to match it whole.
    RegexBuilder::new(regex)
        .size_limit(limits.regex_size_limit)
        .build()
        .map_err(|error| invalid(error.to_string()))?;
    RegexBuilder::new(&format!("\\A(?:{regex})\\z"))
        .size_limit(limits.regex_size_limit)
        .build()
        .map_err(|error| invalid(error.to_string()))
}

/// Resolve every detector to its matcher: every pattern compiles and every `Dictionary` and
/// `Model` resource resolves before anything is scanned. `probe` is a value a `Model` rule's
/// set is asked about, so a set the caller does not hold is refused up front.
fn compile_matchers<'a>(
    detectors: &'a [SensitiveDetector],
    limits: &ScanLimits,
    resources: &'a dyn DetectorResources,
    probe: Option<&str>,
) -> Result<Vec<Matcher<'a>>, ScanError> {
    let mut matchers = Vec::with_capacity(detectors.len());
    for detector in detectors {
        let missing = || ScanError::ResourceMissing {
            detector: detector.id.clone(),
        };
        matchers.push(match &detector.rule {
            DetectorRule::Pattern { regex } => {
                Matcher::Pattern(compile_pattern(detector, regex, limits)?)
            }
            DetectorRule::Checksum { algorithm } => Matcher::Checksum(*algorithm),
            DetectorRule::Dictionary { terms_digest } => {
                Matcher::Dictionary(resources.dictionary(terms_digest).ok_or_else(missing)?)
            }
            DetectorRule::Model { exemplar_set } => {
                // A set the caller does not hold answers `None` for any value.
                if let Some(first) = probe {
                    resources
                        .max_exemplar_cosine(exemplar_set, first)
                        .ok_or_else(missing)?;
                }
                Matcher::Model(exemplar_set)
            }
        });
    }
    Ok(matchers)
}

/// Whether one whole value hits a non-`Model` matcher: the one definition of each rule.
/// `Model` matchers score the sampled values and are handled by the caller.
fn value_hits(matcher: &Matcher<'_>, value: &str) -> bool {
    match matcher {
        Matcher::Pattern(regex) => regex.is_match(value.trim()),
        Matcher::Checksum(algorithm) => checksum_hit(*algorithm, value),
        Matcher::Dictionary(terms) => terms.contains(&value.trim().to_lowercase()),
        Matcher::Model(_) => false,
    }
}

/// Scan one column. `values` are the column in profile read order; the sample is the first
/// `max_sample_values` non-null ones. Order: the limits, then every detector, then every
/// pattern compiles, then every `Dictionary` and `Model` resource resolves; nothing is
/// scanned unless all of them pass. One [`Detection`] per detector, in detector order, also
/// below its threshold.
pub fn scan_values(
    asset: &str,
    values: &[Option<&str>],
    detectors: &[SensitiveDetector],
    limits: &ScanLimits,
    resources: &dyn DetectorResources,
    now_ms: i64,
) -> Result<Vec<Detection>, ScanError> {
    limits.validate()?;
    for detector in detectors {
        detector.validate()?;
    }
    let sample: Vec<&str> = values
        .iter()
        .flatten()
        .copied()
        .take(limits.max_sample_values)
        .collect();
    let matchers = compile_matchers(detectors, limits, resources, sample.first().copied())?;

    let mut detections = Vec::with_capacity(detectors.len());
    for (detector, matcher) in detectors.iter().zip(&matchers) {
        let (hits, scanned) = match matcher {
            Matcher::Model(exemplar_set) => {
                let scored = &sample[..sample.len().min(limits.model_sample_values)];
                let threshold = detector.threshold as f32;
                let hits = scored
                    .iter()
                    .filter(|value| {
                        resources
                            .max_exemplar_cosine(exemplar_set, value)
                            .is_some_and(|cosine| cosine >= threshold)
                    })
                    .count();
                (hits, scored.len())
            }
            other => (
                sample
                    .iter()
                    .filter(|value| value_hits(other, value))
                    .count(),
                sample.len(),
            ),
        };
        detections.push(Detection {
            asset: asset.to_string(),
            detector: detector.id.clone(),
            sample_hits: hits as u64,
            ratio: if scanned == 0 {
                0.0
            } else {
                hits as f64 / scanned as f64
            },
            sample_size: scanned as u64,
            scanned_at_ms: now_ms,
        });
    }
    Ok(detections)
}

/// The characters trimmed from both ends of a chunk token before it is a candidate.
const CHUNK_TRIM: &[char] = &[
    '.', ',', ';', ':', '!', '?', '(', ')', '[', ']', '{', '}', '"', '\'', '<', '>',
];

/// The candidate values of one chunk (MEDULLA-TYPED-PACKS-R803.7): the chunk text split on
/// Unicode whitespace, each token trimmed of [`CHUNK_TRIM`], then every token and every run of
/// 2 to 4 consecutive tokens joined by single spaces.
pub fn chunk_candidates(chunk: &str) -> Vec<String> {
    let tokens: Vec<&str> = chunk
        .split_whitespace()
        .map(|token| token.trim_matches(CHUNK_TRIM))
        .filter(|token| !token.is_empty())
        .collect();
    let mut candidates = Vec::new();
    for start in 0..tokens.len() {
        for length in 1..=4usize {
            if start + length > tokens.len() {
                break;
            }
            candidates.push(tokens[start..start + length].join(" "));
        }
    }
    candidates
}

/// Scan the chunks of one Document (MEDULLA-TYPED-PACKS-R803.7). At most `max_chunks` chunks
/// are scanned. A chunk hits a detector when at least one candidate of the chunk hits it under
/// the whole-value rules of [`scan_values`]; a `Model` detector scores the whole chunk text and
/// only the first `model_sample_values` chunks. `sample_hits` counts hit chunks and `ratio` is
/// hit chunks over scanned chunks. The same validation order as [`scan_values`] holds.
pub fn scan_chunks(
    asset: &str,
    chunks: &[&str],
    detectors: &[SensitiveDetector],
    limits: &ScanLimits,
    max_chunks: usize,
    resources: &dyn DetectorResources,
    now_ms: i64,
) -> Result<Vec<Detection>, ScanError> {
    limits.validate()?;
    for detector in detectors {
        detector.validate()?;
    }
    let scanned_chunks = &chunks[..chunks.len().min(max_chunks)];
    let matchers = compile_matchers(
        detectors,
        limits,
        resources,
        scanned_chunks.first().copied(),
    )?;
    let candidates: Vec<Vec<String>> = scanned_chunks
        .iter()
        .map(|chunk| chunk_candidates(chunk))
        .collect();

    let mut detections = Vec::with_capacity(detectors.len());
    for (detector, matcher) in detectors.iter().zip(&matchers) {
        let (hits, scanned) = match matcher {
            Matcher::Model(exemplar_set) => {
                let scored =
                    &scanned_chunks[..scanned_chunks.len().min(limits.model_sample_values)];
                let threshold = detector.threshold as f32;
                let hits = scored
                    .iter()
                    .filter(|chunk| {
                        resources
                            .max_exemplar_cosine(exemplar_set, chunk)
                            .is_some_and(|cosine| cosine >= threshold)
                    })
                    .count();
                (hits, scored.len())
            }
            other => (
                candidates
                    .iter()
                    .filter(|chunk| chunk.iter().any(|candidate| value_hits(other, candidate)))
                    .count(),
                scanned_chunks.len(),
            ),
        };
        detections.push(Detection {
            asset: asset.to_string(),
            detector: detector.id.clone(),
            sample_hits: hits as u64,
            ratio: if scanned == 0 {
                0.0
            } else {
                hits as f64 / scanned as f64
            },
            sample_size: scanned as u64,
            scanned_at_ms: now_ms,
        });
    }
    Ok(detections)
}

/// The detections whose ratio reaches the threshold of the detector with the same id.
pub fn passing<'a>(
    detections: &'a [Detection],
    detectors: &[SensitiveDetector],
) -> Vec<&'a Detection> {
    detections
        .iter()
        .filter(|detection| {
            detection.sample_size > 0
                && detectors
                    .iter()
                    .find(|detector| detector.id == detection.detector)
                    .is_some_and(|detector| detection.ratio >= detector.threshold)
        })
        .collect()
}
