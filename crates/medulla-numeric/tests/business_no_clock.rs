//! MEDULLA-BUSINESS-OPS-R002.10: no file under `src/business/` reads the system clock or a
//! source of randomness. Every rule takes its as-of time as an argument and returns the
//! same value for the same inputs on every host.

use std::path::Path;

/// Identifiers that read a clock or a source of randomness.
const BANNED_TOKENS: &[&str] = &[
    "SystemTime",
    "Instant",
    "UNIX_EPOCH",
    "Utc",
    "Local",
    "chrono",
    "getrandom",
    "thread_rng",
    "OsRng",
    "RandomState",
    "StdRng",
    "SmallRng",
    "ChaCha8Rng",
    "ChaCha20Rng",
    "rand",
    "rand_chacha",
    "rand_distr",
    "Uuid",
    "uuid",
];

/// Paths and calls that read a clock.
const BANNED_FRAGMENTS: &[&str] = &["std::time", "::now(", ".elapsed(", "time::OffsetDateTime"];

fn business_sources() -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/business");
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&dir)
        .expect("src/business exists")
        .flatten()
    {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "rs") {
            let text = std::fs::read_to_string(&path).expect("a business source reads");
            files.push((
                path.file_name().unwrap().to_string_lossy().into_owned(),
                text,
            ));
        }
    }
    files.sort();
    files
}

// spec: MEDULLA-BUSINESS-OPS-R002.10
#[test]
fn business_module_has_no_clock_or_random() {
    let files = business_sources();
    // The module's files are all scanned, not an empty directory.
    for expected in [
        "mod.rs",
        "recurrence.rs",
        "reconcile.rs",
        "reorder.rs",
        "rule_spec.rs",
        "phrase_policy.rs",
        "threshold.rs",
    ] {
        assert!(
            files.iter().any(|(name, _)| name == expected),
            "{expected} is scanned"
        );
    }
    let mut found = Vec::new();
    for (name, text) in &files {
        for (number, line) in text.lines().enumerate() {
            // Comments and documentation may name the things the code must not do.
            let code = line.split("//").next().unwrap_or_default();
            for fragment in BANNED_FRAGMENTS {
                if code.contains(fragment) {
                    found.push(format!("{name}:{}: {fragment}", number + 1));
                }
            }
            for token in code.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
                if BANNED_TOKENS.contains(&token) {
                    found.push(format!("{name}:{}: {token}", number + 1));
                }
            }
        }
    }
    assert!(
        found.is_empty(),
        "business rule code reads a clock or a random source: {found:?}"
    );
}

/// The scan itself catches a planted clock read and a planted random source.
#[test]
fn the_scan_finds_a_planted_clock_and_a_planted_random_source() {
    let planted = [
        "let t = std::time::SystemTime::now();",
        "let r = thread_rng();",
    ];
    let mut hits = 0;
    for line in planted {
        let code = line.split("//").next().unwrap_or_default();
        let fragment = BANNED_FRAGMENTS.iter().any(|f| code.contains(f));
        let token = code
            .split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .any(|t| BANNED_TOKENS.contains(&t));
        if fragment || token {
            hits += 1;
        }
    }
    assert_eq!(hits, 2);
}
