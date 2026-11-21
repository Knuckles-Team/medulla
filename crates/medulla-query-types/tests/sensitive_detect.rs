use medulla_query_types::sensitive_detect::{
    builtin_detectors, ChecksumAlgorithm, Detection, DetectorRule, InvalidDetector,
    SensitiveDetectionMethod, SensitiveDetector, DEFAULT_THRESHOLD,
};

/// Verifies MEDULLA-TYPED-PACKS-R803.1: Model
// spec: MEDULLA-TYPED-PACKS-R803.1
#[test]
fn records_round_trip() {
    let detector = SensitiveDetector {
        id: "detector:card-number".into(),
        category: "payment_card".into(),
        method: SensitiveDetectionMethod::Checksum,
        rule: DetectorRule::Checksum {
            algorithm: ChecksumAlgorithm::Luhn,
        },
        threshold: 0.75,
    };
    let detection = Detection {
        asset: "column:payments.card_number".into(),
        detector: detector.id.clone(),
        sample_hits: 3,
        ratio: 0.3,
        sample_size: 10,
        scanned_at_ms: 1_700_000_000_000,
    };

    for encoded in [
        serde_json::to_string(&detector).unwrap(),
        serde_json::to_string(&detection).unwrap(),
    ] {
        assert!(serde_json::from_str::<serde_json::Value>(&encoded).is_ok());
    }
    assert_eq!(
        serde_json::from_str::<SensitiveDetector>(&serde_json::to_string(&detector).unwrap())
            .unwrap(),
        detector
    );
    assert_eq!(
        serde_json::from_str::<Detection>(&serde_json::to_string(&detection).unwrap()).unwrap(),
        detection
    );

    let unknown_method = serde_json::to_string(&detector)
        .unwrap()
        .replace("checksum", "heuristic");
    assert!(serde_json::from_str::<SensitiveDetector>(&unknown_method).is_err());

    let unknown_field = serde_json::to_string(&detection)
        .unwrap()
        .replace('}', ",\"matched_values\":[\"must not be stored\"]}");
    assert!(serde_json::from_str::<Detection>(&unknown_field).is_err());
}

fn detector(rule: DetectorRule, threshold: f64) -> SensitiveDetector {
    SensitiveDetector {
        id: "detector:probe".into(),
        category: "probe".into(),
        method: rule.method(),
        rule,
        threshold,
    }
}

// spec: MEDULLA-TYPED-PACKS-R803.1
#[test]
fn detector_rules_round_trip_and_validate() {
    let rules = [
        DetectorRule::Pattern {
            regex: "^a+$".into(),
        },
        DetectorRule::Checksum {
            algorithm: ChecksumAlgorithm::IbanMod97,
        },
        DetectorRule::Dictionary {
            terms_digest: "sha256:abc".into(),
        },
        DetectorRule::Model {
            exemplar_set: "exemplars:cards".into(),
        },
    ];
    for rule in rules {
        let probe = detector(rule, 0.5);
        let encoded = serde_json::to_string(&probe).unwrap();
        assert_eq!(
            serde_json::from_str::<SensitiveDetector>(&encoded).unwrap(),
            probe
        );
        assert_eq!(probe.validate(), Ok(()));
    }

    // An unknown rule kind and an unknown field are refused.
    let unknown_kind = r#"{"id":"d","category":"c","method":"pattern","rule":{"kind":"fuzzy"},"threshold":0.5}"#;
    assert!(serde_json::from_str::<SensitiveDetector>(unknown_kind).is_err());
    let unknown_field = r#"{"id":"d","category":"c","method":"pattern","rule":{"kind":"pattern","regex":"a","flags":"i"},"threshold":0.5}"#;
    assert!(serde_json::from_str::<SensitiveDetector>(unknown_field).is_err());

    let pattern = DetectorRule::Pattern {
        regex: "^a+$".into(),
    };
    assert_eq!(
        detector(pattern.clone(), 1.5).validate().unwrap_err().code(),
        "DETECTOR_THRESHOLD_INVALID"
    );
    assert_eq!(
        detector(pattern.clone(), f64::NAN)
            .validate()
            .unwrap_err()
            .code(),
        "DETECTOR_THRESHOLD_INVALID"
    );
    let mut mismatched = detector(
        DetectorRule::Checksum {
            algorithm: ChecksumAlgorithm::Luhn,
        },
        0.5,
    );
    mismatched.method = SensitiveDetectionMethod::Pattern;
    assert_eq!(
        mismatched.validate().unwrap_err().code(),
        "DETECTOR_RULE_MISMATCH"
    );
    let empty = detector(DetectorRule::Pattern { regex: String::new() }, 0.5)
        .validate()
        .unwrap_err();
    assert!(matches!(empty, InvalidDetector::PatternInvalid { .. }));
    assert_eq!(empty.code(), "DETECTOR_PATTERN_INVALID");
    assert_eq!(empty.to_string(), "DETECTOR_PATTERN_INVALID: detector:probe");

    let ids: Vec<String> = builtin_detectors().into_iter().map(|d| d.id).collect();
    assert_eq!(
        ids,
        [
            "detector:card_number",
            "detector:email_address",
            "detector:iban",
            "detector:ipv4_address"
        ]
    );
    for builtin in builtin_detectors() {
        assert_eq!(builtin.threshold, DEFAULT_THRESHOLD);
        assert_eq!(builtin.validate(), Ok(()));
    }
}
