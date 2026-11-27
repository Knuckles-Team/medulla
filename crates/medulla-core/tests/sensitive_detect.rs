//! Column scan for sensitive values (MEDULLA-TYPED-PACKS-R803.2).

use medulla_core::sensitive_scan::{
    chunk_candidates, passing, scan_chunks, scan_values, NoResources, ScanError, ScanLimits,
};
use medulla_query_types::sensitive_detect::{
    builtin_detectors, DetectorRule, SensitiveDetectionMethod, SensitiveDetector,
};

const VALID_CARDS: [&str; 8] = [
    "4111111111111111",
    "5500005555555559",
    "340000000000009",
    "4012888888881881",
    "378282246310005",
    "6011111111111117",
    "3530111333300000",
    "5105 1051 0510 5100",
];
const INVALID_CARDS: [&str; 2] = ["4111111111111112", "1234567812345678"];
const VALID_IBAN: &str = "GB82 WEST 1234 5698 7654 32";
const CHANGED_IBAN: &str = "GB82 WEST 1234 5698 7654 33";

fn detector(id: &str, rule: DetectorRule) -> SensitiveDetector {
    SensitiveDetector {
        id: id.into(),
        category: "probe".into(),
        method: rule.method(),
        rule,
        threshold: 0.8,
    }
}

// spec: MEDULLA-TYPED-PACKS-R803.2
#[test]
fn scan_values_counts_hits_without_values() {
    let detectors = builtin_detectors();
    let limits = ScanLimits::default();
    let now_ms = 1_700_000_000_123;

    // Ten card values (eight Luhn-valid) and nulls that are skipped.
    let mut card_column: Vec<Option<&str>> = vec![None];
    card_column.extend(VALID_CARDS.iter().map(|v| Some(*v)));
    card_column.push(None);
    card_column.extend(INVALID_CARDS.iter().map(|v| Some(*v)));
    let cards = scan_values(
        "column:payments.card_number",
        &card_column,
        &detectors,
        &limits,
        &NoResources,
        now_ms,
    )
    .unwrap();
    assert_eq!(cards.len(), detectors.len());
    let card = cards
        .iter()
        .find(|d| d.detector == "detector:card_number")
        .unwrap();
    assert_eq!(card.sample_hits, 8);
    assert_eq!(card.sample_size, 10);
    assert_eq!(card.ratio, 0.8);
    assert_eq!(card.scanned_at_ms, now_ms);
    let kept: Vec<&str> = passing(&cards, &detectors)
        .iter()
        .map(|d| d.detector.as_str())
        .collect();
    assert!(kept.contains(&"detector:card_number"));
    assert!(!kept.contains(&"detector:iban"));

    // An email column matches the email pattern whole.
    let emails = ["ada@example.org", "bob@example.org", "not an email"];
    let email_column: Vec<Option<&str>> = emails.iter().map(|v| Some(*v)).collect();
    let found = scan_values(
        "column:users.email",
        &email_column,
        &detectors,
        &limits,
        &NoResources,
        now_ms,
    )
    .unwrap();
    let email = found
        .iter()
        .find(|d| d.detector == "detector:email_address")
        .unwrap();
    assert_eq!((email.sample_hits, email.sample_size), (2, 3));

    // A valid IBAN hits and a one-digit-changed one does not.
    let iban_only = [detectors[2].clone()];
    let valid = scan_values(
        "column:a",
        &[Some(VALID_IBAN)],
        &iban_only,
        &limits,
        &NoResources,
        now_ms,
    )
    .unwrap();
    assert_eq!(valid[0].sample_hits, 1);
    let changed = scan_values(
        "column:a",
        &[Some(CHANGED_IBAN)],
        &iban_only,
        &limits,
        &NoResources,
        now_ms,
    )
    .unwrap();
    assert_eq!(changed[0].sample_hits, 0);

    // The sample is bounded by max_sample_values.
    let many: Vec<Option<&str>> = (0..150).map(|_| Some("4111111111111111")).collect();
    let bounded = ScanLimits {
        max_sample_values: 100,
        ..ScanLimits::default()
    };
    let capped = scan_values(
        "column:a",
        &many,
        &detectors,
        &bounded,
        &NoResources,
        now_ms,
    )
    .unwrap();
    assert!(capped.iter().all(|d| d.sample_size == 100));

    // A dictionary with no resources and an unparsable regex refuse before any detection.
    let dictionary = detector(
        "detector:terms",
        DetectorRule::Dictionary {
            terms_digest: "sha256:terms".into(),
        },
    );
    let refused = scan_values(
        "column:a",
        &card_column,
        &[dictionary],
        &limits,
        &NoResources,
        now_ms,
    )
    .unwrap_err();
    assert!(matches!(refused, ScanError::ResourceMissing { .. }));
    assert_eq!(refused.code(), "DETECTOR_RESOURCE_MISSING");
    let broken = detector(
        "detector:broken",
        DetectorRule::Pattern { regex: "(".into() },
    );
    let refused = scan_values(
        "column:a",
        &card_column,
        &[broken],
        &limits,
        &NoResources,
        now_ms,
    )
    .unwrap_err();
    assert_eq!(refused.code(), "DETECTOR_PATTERN_INVALID");
    let mismatched = SensitiveDetector {
        method: SensitiveDetectionMethod::Checksum,
        ..detector(
            "detector:mismatch",
            DetectorRule::Pattern { regex: "a".into() },
        )
    };
    assert_eq!(
        scan_values(
            "column:a",
            &card_column,
            &[mismatched],
            &limits,
            &NoResources,
            now_ms
        )
        .unwrap_err()
        .code(),
        "DETECTOR_RULE_MISMATCH"
    );
    let bad_limits = ScanLimits {
        max_sample_values: 1,
        ..ScanLimits::default()
    };
    assert_eq!(
        scan_values(
            "column:a",
            &card_column,
            &detectors,
            &bad_limits,
            &NoResources,
            now_ms
        )
        .unwrap_err()
        .code(),
        "SCAN_LIMITS_INVALID"
    );

    // No input value appears in any output.
    let debug = format!("{cards:?} {found:?} {valid:?} {changed:?}");
    for value in VALID_CARDS
        .iter()
        .chain(INVALID_CARDS.iter())
        .chain(emails.iter())
        .chain([VALID_IBAN, CHANGED_IBAN].iter())
    {
        assert!(!debug.contains(value), "{value} leaked into the output");
    }
}

// spec: MEDULLA-TYPED-PACKS-R803.7
#[test]
fn chunk_scan_counts_hit_chunks_over_candidates_without_values() {
    // A card written as four tokens is one candidate of the chunk.
    let candidates = chunk_candidates("Paid with (4111 1111 1111 1111), thanks.");
    assert!(candidates.contains(&"4111 1111 1111 1111".to_string()));
    assert!(candidates.contains(&"thanks".to_string()));
    assert!(!candidates.iter().any(|candidate| candidate.contains(',')));

    let detectors = builtin_detectors();
    let limits = ScanLimits::default();
    let chunks = [
        "first order paid with 4111 1111 1111 1111 today",
        "nothing sensitive in this chunk",
        "second order card 4111111111111111.",
        "also nothing here",
    ];
    let detections =
        scan_chunks("doc:1", &chunks, &detectors, &limits, 500, &NoResources, 7).unwrap();
    let card = detections
        .iter()
        .find(|detection| passing(std::slice::from_ref(detection), &detectors).len() == 1)
        .expect("the card detector passes");
    assert_eq!(card.sample_hits, 2);
    assert_eq!(card.sample_size, 4);
    assert!((card.ratio - 0.5).abs() < f64::EPSILON);
    assert!(!format!("{card:?}").contains("4111"));

    // Only the first `max_chunks` chunks are scanned.
    let capped = scan_chunks("doc:1", &chunks, &detectors, &limits, 2, &NoResources, 7).unwrap();
    assert!(capped.iter().all(|detection| detection.sample_size == 2));
}
