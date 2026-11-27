//! Skill package validation: the model (MEDULLA-DECISION-ENGINE-R160.1).
//!
//! `PublishSkillPackage` publishes a rendered skill package only when its run
//! history validates it. These are the types that identity, history, verdict and
//! receipt travel in. Nothing here reads a clock or a store.

use medulla_catalog_types::fleet_catalog::rendered::RenderedSkillPackage;
use medulla_types::contract::{Digest256, OpaqueId};
use medulla_types::refusal_code::RefusalCode;

use super::knob_families::package::knob_values;
use serde::{Deserialize, Serialize};

/// `PACKAGE_DIGEST_MISMATCH`: the rendered digest does not parse, or a run names another package.
pub const PACKAGE_DIGEST_MISMATCH: RefusalCode = RefusalCode::new("PACKAGE_DIGEST_MISMATCH");
/// `PACKAGE_RUN_SET_INVALID`: two history entries share a `run_id`.
pub const PACKAGE_RUN_SET_INVALID: RefusalCode = RefusalCode::new("PACKAGE_RUN_SET_INVALID");
/// `PACKAGE_RUN_HISTORY_MISSING`: fewer than `package.min_runs` runs are recorded.
pub const PACKAGE_RUN_HISTORY_MISSING: RefusalCode =
    RefusalCode::new("PACKAGE_RUN_HISTORY_MISSING");
/// `PACKAGE_VALIDATION_FAILED`: the pass share is below `package.min_pass_rate`.
pub const PACKAGE_VALIDATION_FAILED: RefusalCode = RefusalCode::new("PACKAGE_VALIDATION_FAILED");

/// The four codes of package publication: the one refusal list of `PublishSkillPackage`
/// (MEDULLA-DECISION-ENGINE-R160.4).
pub const REFUSALS: &[RefusalCode] = &[
    PACKAGE_DIGEST_MISMATCH,
    PACKAGE_RUN_SET_INVALID,
    PACKAGE_RUN_HISTORY_MISSING,
    PACKAGE_VALIDATION_FAILED,
];

/// The one refusal type of package publication. Each variant is the trigger of one numbered
/// step of `validate_rendered_package` (MEDULLA-DECISION-ENGINE-R160.2); a refusal writes no
/// receipt and the catalog stays unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicationRefusal {
    /// The rendered digest does not parse, or a run names a different package digest.
    DigestMismatch,
    /// Two history entries share a `run_id`.
    RunSetInvalid,
    /// Fewer runs are recorded than `package.min_runs`.
    RunHistoryMissing,
    /// The share of passing runs is below `package.min_pass_rate`.
    ValidationFailed { passed: u64, total: u64 },
}

impl PublicationRefusal {
    /// The wire code of this refusal.
    pub fn code(&self) -> RefusalCode {
        match self {
            Self::DigestMismatch => PACKAGE_DIGEST_MISMATCH,
            Self::RunSetInvalid => PACKAGE_RUN_SET_INVALID,
            Self::RunHistoryMissing => PACKAGE_RUN_HISTORY_MISSING,
            Self::ValidationFailed { .. } => PACKAGE_VALIDATION_FAILED,
        }
    }
}

impl std::fmt::Display for PublicationRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let code = self.code().as_str();
        match self {
            Self::DigestMismatch => write!(
                f,
                "{code}: the rendered digest is not a digest, or a run names another package"
            ),
            Self::RunSetInvalid => write!(f, "{code}: two runs share one run id"),
            Self::RunHistoryMissing => write!(f, "{code}: too few runs are recorded"),
            Self::ValidationFailed { passed, total } => {
                write!(f, "{code}: {passed} of {total} runs passed")
            }
        }
    }
}

impl std::error::Error for PublicationRefusal {}

/// Domain of the run-history digest.
pub const RUN_HISTORY_DIGEST_DOMAIN: &[u8] = b"eg/package-run-history/v1";

/// Which package content, rendered from which graph revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PackageIdentity {
    pub content_digest: Digest256,
    pub graph_revision: u64,
}

impl PackageIdentity {
    /// The identity of a rendered package: its digest and graph revision.
    pub fn of_rendered(package: &RenderedSkillPackage) -> Result<Self, String> {
        Ok(Self {
            content_digest: Digest256::parse(&package.digest)?,
            graph_revision: package.graph_revision,
        })
    }
}

/// One run of a package and whether it passed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RunHistoryEntry {
    pub run_id: OpaqueId,
    pub package_digest: Digest256,
    pub passed: bool,
}

/// Key prefix of every recorded package run (MEDULLA-DECISION-ENGINE-R160.3).
pub const PACKAGE_RUN_PREFIX: &str = "package_run/";
/// Key prefix of every package publication receipt.
pub const PACKAGE_PUBLICATION_PREFIX: &str = "package_publication/";
/// Most run rows one package's history reads.
pub const MAX_PACKAGE_RUNS: usize = 4096;
/// Most publication receipts one tenant's catalog read loads.
pub const MAX_PUBLISHED_PACKAGES: usize = 65536;

/// The key prefix of the runs recorded for one package digest.
pub fn package_run_prefix(digest: &Digest256) -> String {
    format!("{PACKAGE_RUN_PREFIX}{}/", digest.to_hex())
}

/// The key of one recorded run: the package prefix then the run id, so a
/// prefix read returns the history in ascending `run_id` order.
pub fn package_run_key(digest: &Digest256, run_id: &OpaqueId) -> String {
    format!("{}{}", package_run_prefix(digest), run_id.as_str())
}

/// The key of the publication receipt of one package digest.
pub fn package_publication_key(digest: &Digest256) -> String {
    format!("{PACKAGE_PUBLICATION_PREFIX}{}", digest.to_hex())
}

/// The body of `Method::PublishSkillPackage`. The request carries no tenant:
/// the tenant is the verified one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PublishSkillPackageRequest {
    pub package_id: OpaqueId,
    pub package: RenderedSkillPackage,
}

/// Validate a rendered package against its recorded run history. Pure: it reads no clock,
/// store or tenant policy, and returns the validation, not the receipt; the handler mints the
/// [`PublicationReceipt`] from it. `history` is the tenant-visible run history in recorded
/// order, so tenant scoping is the caller's. The first failing step refuses
/// (MEDULLA-DECISION-ENGINE-R160.2):
///
/// 1. an unparseable `rendered.digest` refuses `DigestMismatch`;
/// 2. two entries with one `run_id` refuse `RunSetInvalid`;
/// 3. fewer than `package.min_runs` entries refuse `RunHistoryMissing`;
/// 4. an entry whose `package_digest` differs from the rendered digest refuses `DigestMismatch`;
/// 5. `passed * 1000000 / len` below `package.min_pass_rate` refuses `ValidationFailed`.
pub fn validate_rendered_package(
    rendered: &RenderedSkillPackage,
    history: &[RunHistoryEntry],
) -> Result<RenderedPackageValidation, PublicationRefusal> {
    let knobs = knob_values();
    let rendered_digest =
        Digest256::parse(&rendered.digest).map_err(|_| PublicationRefusal::DigestMismatch)?;
    let mut seen = std::collections::BTreeSet::new();
    if !history
        .iter()
        .all(|entry| seen.insert(entry.run_id.as_str()))
    {
        return Err(PublicationRefusal::RunSetInvalid);
    }
    let total = history.len() as u64;
    if total < knobs.min_runs {
        return Err(PublicationRefusal::RunHistoryMissing);
    }
    if history
        .iter()
        .any(|entry| entry.package_digest != rendered_digest)
    {
        return Err(PublicationRefusal::DigestMismatch);
    }
    let passed = history.iter().filter(|entry| entry.passed).count() as u64;
    if passed * 1_000_000 / total < knobs.min_pass_rate_ppm {
        return Err(PublicationRefusal::ValidationFailed { passed, total });
    }
    // The only way the digest fails is a history beyond the framing limit: not a valid run set.
    let run_history_digest =
        run_history_digest(history).map_err(|_| PublicationRefusal::RunSetInvalid)?;
    Ok(RenderedPackageValidation {
        rendered_digest,
        run_history_digest,
        validation_passed: true,
    })
}

/// The digest of an ordered run history: SHA-256, domain-framed, over each
/// entry's `run_id` and `package_digest` in order. `passed` is the verdict the
/// digest vouches for, not part of the identity of the runs.
pub fn run_history_digest(history: &[RunHistoryEntry]) -> Result<Digest256, String> {
    let mut fields: Vec<&[u8]> = Vec::with_capacity(history.len() * 2);
    for entry in history {
        fields.push(entry.run_id.as_str().as_bytes());
        fields.push(entry.package_digest.as_bytes());
    }
    Digest256::framed(RUN_HISTORY_DIGEST_DOMAIN, &fields)
}

/// What the validation of one rendered package saw and decided.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RenderedPackageValidation {
    pub rendered_digest: Digest256,
    pub run_history_digest: Digest256,
    pub validation_passed: bool,
}

/// The receipt of one publication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PublicationReceipt {
    pub package_id: OpaqueId,
    pub validation_result: RenderedPackageValidation,
    /// Unix milliseconds.
    pub publish_timestamp_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_catalog_types::fleet_catalog::rendered::RENDERED_SKILL_PACKAGE_SCHEMA_VERSION;

    fn entry(run: &str, content: &[u8], passed: bool) -> RunHistoryEntry {
        RunHistoryEntry {
            run_id: OpaqueId::new(run).unwrap(),
            package_digest: Digest256::sha256(content),
            passed,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R160.1
    #[test]
    fn validation_types_round_trip() {
        let package = RenderedSkillPackage {
            schema_version: RENDERED_SKILL_PACKAGE_SCHEMA_VERSION,
            uri: "skill://example".into(),
            digest: Digest256::sha256(b"package").to_hex(),
            ontology_refs: vec!["https://knuckles-team.github.io/ontology/core#Skill".into()],
            graph_revision: 42,
        };
        let identity = PackageIdentity::of_rendered(&package).unwrap();
        assert_eq!(identity.graph_revision, 42);
        assert_eq!(identity.content_digest, Digest256::sha256(b"package"));
        let mut bad = package.clone();
        bad.digest = "NOT-A-DIGEST".into();
        assert!(PackageIdentity::of_rendered(&bad).is_err());

        let history = vec![
            entry("run-1", b"package", true),
            entry("run-2", b"package", false),
        ];
        let digest = run_history_digest(&history).unwrap();
        // Order matters; the verdict flag does not change which runs were seen.
        let reversed: Vec<RunHistoryEntry> = history.iter().rev().cloned().collect();
        assert_ne!(digest, run_history_digest(&reversed).unwrap());
        let mut flipped = history.clone();
        flipped[1].passed = true;
        assert_eq!(digest, run_history_digest(&flipped).unwrap());
        assert_ne!(digest, run_history_digest(&history[..1]).unwrap());

        let validation = RenderedPackageValidation {
            rendered_digest: identity.content_digest,
            run_history_digest: digest,
            validation_passed: false,
        };
        let receipt = PublicationReceipt {
            package_id: OpaqueId::new("pkg-1").unwrap(),
            validation_result: validation,
            publish_timestamp_ms: 1_700_000_000_000,
        };

        macro_rules! round_trip {
            ($value:expr, $ty:ty) => {{
                let json = serde_json::to_value(&$value).unwrap();
                assert_eq!(serde_json::from_value::<$ty>(json).unwrap(), $value);
                let packed = rmp_serde::to_vec_named(&$value).unwrap();
                assert_eq!(rmp_serde::from_slice::<$ty>(&packed).unwrap(), $value);
            }};
        }
        round_trip!(identity, PackageIdentity);
        round_trip!(history[0], RunHistoryEntry);
        round_trip!(receipt.validation_result, RenderedPackageValidation);
        round_trip!(receipt, PublicationReceipt);

        // Unknown fields and malformed digests are refused.
        let mut wire = serde_json::to_value(&receipt).unwrap();
        wire["extra"] = serde_json::json!(true);
        assert!(serde_json::from_value::<PublicationReceipt>(wire).is_err());
        let mut wire = serde_json::to_value(&identity).unwrap();
        wire["content_digest"] = serde_json::json!("abc");
        assert!(serde_json::from_value::<PackageIdentity>(wire).is_err());
    }

    // spec: MEDULLA-DECISION-ENGINE-R160.4
    #[test]
    fn refusal_codes_are_unique_and_registered() {
        let refusals = [
            (
                PublicationRefusal::DigestMismatch,
                "PACKAGE_DIGEST_MISMATCH",
            ),
            (PublicationRefusal::RunSetInvalid, "PACKAGE_RUN_SET_INVALID"),
            (
                PublicationRefusal::RunHistoryMissing,
                "PACKAGE_RUN_HISTORY_MISSING",
            ),
            (
                PublicationRefusal::ValidationFailed {
                    passed: 1,
                    total: 3,
                },
                "PACKAGE_VALIDATION_FAILED",
            ),
        ];
        for (refusal, code) in &refusals {
            assert_eq!(refusal.code().as_str(), *code);
            assert!(refusal.to_string().starts_with(&format!("{code}: ")));
            assert!(REFUSALS.contains(&refusal.code()));
            assert_eq!(crate::decision::codes::resolve(code), Some(refusal.code()));
        }
        let unique: std::collections::BTreeSet<_> = REFUSALS.iter().collect();
        assert_eq!(unique.len(), REFUSALS.len());
        assert_eq!(REFUSALS.len(), 4);
        // `EXACT_METHOD_ERRORS` lists exactly these codes for `PublishSkillPackage`.
        let routing = include_str!("../../../medulla-capabilities/src/error_routing.rs");
        let entry = routing
            .split("\"PublishSkillPackage\",")
            .nth(1)
            .expect("an EXACT_METHOD_ERRORS entry for PublishSkillPackage")
            .split("&[")
            .nth(1)
            .and_then(|rest| rest.split(']').next())
            .unwrap();
        let listed: std::collections::BTreeSet<&str> =
            entry.split('"').skip(1).step_by(2).collect();
        let declared: std::collections::BTreeSet<&str> =
            REFUSALS.iter().map(|code| code.as_str()).collect();
        assert_eq!(listed, declared);
        // The codes the spec says do not exist stay unregistered.
        assert_eq!(
            crate::decision::codes::resolve("PACKAGE_RUN_SET_INCOMPLETE"),
            None
        );
        assert_eq!(
            crate::decision::codes::resolve("PACKAGE_POLICY_MISMATCH"),
            None
        );
    }

    fn rendered(content: &[u8]) -> RenderedSkillPackage {
        RenderedSkillPackage {
            schema_version: RENDERED_SKILL_PACKAGE_SCHEMA_VERSION,
            uri: "skill://example".into(),
            digest: Digest256::sha256(content).to_hex(),
            ontology_refs: vec!["https://knuckles-team.github.io/ontology/core#Skill".into()],
            graph_revision: 7,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R160.2
    #[test]
    fn validation_gates_publication() {
        let package = rendered(b"package");
        // A passing history.
        let good = vec![
            entry("run-1", b"package", true),
            entry("run-2", b"package", true),
        ];
        let validation = validate_rendered_package(&package, &good).unwrap();
        assert!(validation.validation_passed);
        assert_eq!(validation.rendered_digest, Digest256::sha256(b"package"));
        assert_eq!(
            validation.run_history_digest,
            run_history_digest(&good).unwrap()
        );

        // A pass share below the knob: one of two is 500000 < 1000000.
        let half = vec![
            entry("run-1", b"package", true),
            entry("run-2", b"package", false),
        ];
        assert_eq!(
            validate_rendered_package(&package, &half),
            Err(PublicationRefusal::ValidationFailed {
                passed: 1,
                total: 2
            })
        );
        // A digest mismatch.
        let other = vec![entry("run-1", b"other", true)];
        assert_eq!(
            validate_rendered_package(&package, &other),
            Err(PublicationRefusal::DigestMismatch)
        );
        // A short history.
        assert_eq!(
            validate_rendered_package(&package, &[]),
            Err(PublicationRefusal::RunHistoryMissing)
        );
        // A duplicate run.
        let duplicate = vec![
            entry("run-1", b"package", true),
            entry("run-1", b"package", true),
        ];
        assert_eq!(
            validate_rendered_package(&package, &duplicate),
            Err(PublicationRefusal::RunSetInvalid)
        );
        // An unparseable rendered digest.
        let mut bad = package.clone();
        bad.digest = "NOT-A-DIGEST".into();
        assert_eq!(
            validate_rendered_package(&bad, &good),
            Err(PublicationRefusal::DigestMismatch)
        );
        // The first failing step wins: a duplicate run id beats a short history, a mismatch
        // and a low pass share; an unparseable digest beats a duplicate.
        let everything = vec![
            entry("run-1", b"other", false),
            entry("run-1", b"other", false),
        ];
        assert_eq!(
            validate_rendered_package(&package, &everything),
            Err(PublicationRefusal::RunSetInvalid)
        );
        assert_eq!(
            validate_rendered_package(&bad, &everything),
            Err(PublicationRefusal::DigestMismatch)
        );
        let mismatch_and_failed = vec![entry("run-1", b"other", false)];
        assert_eq!(
            validate_rendered_package(&package, &mismatch_and_failed),
            Err(PublicationRefusal::DigestMismatch)
        );
        // The knobs are the declared defaults.
        let knobs = knob_values();
        assert_eq!((knobs.min_runs, knobs.min_pass_rate_ppm), (1, 1_000_000));
    }
}
