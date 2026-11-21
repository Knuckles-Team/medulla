//! Research claims, their evidence and sealed research artifacts
//! (MEDULLA-DECISION-ENGINE-R652.1).
//!
//! Pure serde types shared by the claim kernels in `medulla-epistemic`, the
//! `Claims` handler in `medulla-h-decide` and the `Method::Claims` wire body.
//! Every record carries `tenant_id`. `weight_ppm` is parts per million in
//! `[0, WEIGHT_PPM_MAX]`; the bound is checked by [`Evidence::check`] so an
//! out-of-range weight reaches the caller as `EVIDENCE_WEIGHT_OUT_OF_RANGE`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use medulla_types::contract::ServerErrorCode;
use medulla_types::refusal_code::RefusalCode;

/// Largest evidence weight, in parts per million.
pub const WEIGHT_PPM_MAX: u32 = 1_000_000;

/// The refusal codes of the claim surface (MEDULLA-DECISION-ENGINE-R652.4),
/// `(code, meaning)`, sorted by code. Each is a registered [`ServerErrorCode`].
pub const REFUSALS: &[(RefusalCode, &str)] = &[
    (
        RefusalCode::new("ARTIFACT_SEAL_VIOLATION"),
        "an unsealed artifact is presented as sealed, or its digest does not match on read",
    ),
    (
        RefusalCode::new("CLAIM_GUARD_NOT_MET"),
        "the log-odds guard of the requested transition fails",
    ),
    (
        RefusalCode::new("CLAIM_ILLEGAL_TRANSITION"),
        "the requested edge is outside the lifecycle table",
    ),
    (
        RefusalCode::new("CLAIM_NOT_FOUND"),
        "no claim or artifact with this id exists for the tenant",
    ),
    (
        RefusalCode::new("EVIDENCE_NO_CHANNEL"),
        "the evidence names no channel",
    ),
    (
        RefusalCode::new("EVIDENCE_WEIGHT_OUT_OF_RANGE"),
        "the evidence weight is above 1000000 ppm",
    ),
    (
        RefusalCode::new("PROOF_RECEIPT_INVALID"),
        "a proof receipt is malformed, off the process channel, or does not support the claim",
    ),
    (
        RefusalCode::new("PROOF_SORRY_PRESENT"),
        "a pass proof receipt whose proof holds an unfinished step",
    ),
];

/// The lifecycle state of a claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ClaimState {
    Proposed,
    Supported,
    Contested,
    Accepted,
    Retired,
}

impl ClaimState {
    /// The machine discriminant of this state (MEDULLA-FEDERATED-QUERY-R738.3).
    pub const fn kind(self) -> medulla_types::claim_machine::ClaimStateKind {
        use medulla_types::claim_machine::ClaimStateKind as K;
        match self {
            ClaimState::Proposed => K::Proposed,
            ClaimState::Supported => K::Supported,
            ClaimState::Contested => K::Contested,
            ClaimState::Accepted => K::Accepted,
            ClaimState::Retired => K::Retired,
        }
    }

    /// The state of a machine discriminant.
    pub const fn from_kind(kind: medulla_types::claim_machine::ClaimStateKind) -> Self {
        use medulla_types::claim_machine::ClaimStateKind as K;
        match kind {
            K::Proposed => ClaimState::Proposed,
            K::Supported => ClaimState::Supported,
            K::Contested => ClaimState::Contested,
            K::Accepted => ClaimState::Accepted,
            K::Retired => ClaimState::Retired,
        }
    }

    /// The state a new claim takes: the initial state of `ClaimMachine`.
    pub const fn initial() -> Self {
        Self::from_kind(medulla_types::claim_machine::ClaimStateKind::initial())
    }

    /// Apply `event` through `ClaimMachine`: the next state, or `TRANSITION_REFUSED`.
    pub fn apply(
        self,
        event: medulla_types::claim_machine::ClaimEvent,
    ) -> Result<Self, medulla_types::transition_refused::TransitionRefused> {
        self.kind().apply(event).map(Self::from_kind)
    }

    /// Move to `to` along its edge of `ClaimMachine`, or refuse.
    pub fn move_to(
        self,
        to: Self,
    ) -> Result<Self, medulla_types::transition_refused::TransitionRefused> {
        self.kind().move_to(to.kind()).map(Self::from_kind)
    }

    /// Every state, in table order.
    pub const ALL: [ClaimState; 5] = [
        ClaimState::Proposed,
        ClaimState::Supported,
        ClaimState::Contested,
        ClaimState::Accepted,
        ClaimState::Retired,
    ];

    /// The row/column index of this state in the lifecycle table.
    pub const fn index(self) -> usize {
        match self {
            ClaimState::Proposed => 0,
            ClaimState::Supported => 1,
            ClaimState::Contested => 2,
            ClaimState::Accepted => 3,
            ClaimState::Retired => 4,
        }
    }

    /// The snake_case token of the state, as on the wire.
    pub const fn as_str(self) -> &'static str {
        match self {
            ClaimState::Proposed => "proposed",
            ClaimState::Supported => "supported",
            ClaimState::Contested => "contested",
            ClaimState::Accepted => "accepted",
            ClaimState::Retired => "retired",
        }
    }
}

/// One logged lifecycle transition of a claim.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct LifecycleTransition {
    pub tenant_id: String,
    pub claim_id: String,
    pub from: ClaimState,
    pub to: ClaimState,
    pub at_ms: u64,
    pub actor: String,
}

/// The channel a piece of evidence arrived through.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum EvidenceChannel {
    Outcome,
    Optimization,
    Health,
    Process,
    Mined,
}

/// The verdict of an external proof checker.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ProofVerdict {
    Pass,
    Fail,
}

/// Typed evidence that an external checker verified a formal declaration
/// (MEDULLA-DECISION-ENGINE-R652.10). Medulla never runs the checker: a
/// connector runs it and sends the receipt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProofReceipt {
    pub checker_id: String,
    pub toolchain_version: String,
    pub declaration: String,
    /// SHA-256 of the checked artifact: 64 lower-case hex characters.
    pub artifact_digest: String,
    /// The axioms the proof depends on, sorted and deduplicated.
    pub axioms: Vec<String>,
    /// True when the proof holds an unfinished step.
    pub has_sorry: bool,
    pub verdict: ProofVerdict,
}

impl ProofReceipt {
    /// Checks the receipt: every string field is non-empty, the digest is 64
    /// lower-case hex characters, and a `pass` verdict never carries
    /// `has_sorry`.
    pub fn check(&self) -> Result<(), ServerErrorCode> {
        let digest_ok = self.artifact_digest.len() == 64
            && self
                .artifact_digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if self.checker_id.is_empty()
            || self.toolchain_version.is_empty()
            || self.declaration.is_empty()
            || !digest_ok
        {
            return Err(ServerErrorCode::ProofReceiptInvalid);
        }
        if self.verdict == ProofVerdict::Pass && self.has_sorry {
            return Err(ServerErrorCode::ProofSorryPresent);
        }
        Ok(())
    }

    /// The receipt with `axioms` sorted and deduplicated.
    pub fn normalised(mut self) -> Self {
        self.axioms.sort();
        self.axioms.dedup();
        self
    }
}

/// One piece of evidence for or against a claim.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct Evidence {
    pub tenant_id: String,
    pub evidence_id: String,
    pub claim_id: String,
    pub channel: EvidenceChannel,
    /// True when the evidence supports the claim, false when it opposes it.
    pub supports: bool,
    /// Weight in parts per million, `[0, WEIGHT_PPM_MAX]`.
    pub weight_ppm: u32,
    pub source_digest: String,
    /// A machine-checked proof receipt; such evidence moves no log-odds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proof: Option<ProofReceipt>,
}

impl Evidence {
    /// Checks the weight bound and, when the evidence carries a proof receipt,
    /// the receipt: it must sit on the `process` channel, support the claim and
    /// be well formed.
    pub fn check(&self) -> Result<(), ServerErrorCode> {
        if self.weight_ppm > WEIGHT_PPM_MAX {
            return Err(ServerErrorCode::EvidenceWeightOutOfRange);
        }
        if let Some(proof) = &self.proof {
            if self.channel != EvidenceChannel::Process || !self.supports {
                return Err(ServerErrorCode::ProofReceiptInvalid);
            }
            proof.check()?;
        }
        Ok(())
    }
}

/// A set of claims researched together.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct Cohort {
    pub tenant_id: String,
    pub cohort_id: String,
    pub claim_ids: Vec<String>,
}

/// A perspective's stance on a claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum Stance {
    Supports,
    Opposes,
}

/// The stance of each named perspective on one claim.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PerspectiveMap {
    pub tenant_id: String,
    pub claim_id: String,
    pub stances: BTreeMap<String, Stance>,
}

/// The SHA-256 seal of a research artifact: 64 lower-case hex characters.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ArtifactSeal {
    pub digest: String,
}

/// A compiled, sealed research artifact over a cohort of claims.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ResearchArtifact {
    pub tenant_id: String,
    pub artifact_id: String,
    pub cohort_id: String,
    pub claim_ids: Vec<String>,
    pub code_specs: Vec<String>,
    pub exploration_edges: Vec<(String, String)>,
    /// Absent until compiled; the seal covers every other field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seal: Option<ArtifactSeal>,
}

/// A feed item to grade against the fleet topic set.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FeedItem {
    pub text: String,
    pub fleet_topics: Vec<String>,
}

/// The grade of one feed item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FeedGrade {
    pub evidence_id: String,
    pub grade_ppm: u32,
    pub relevant: bool,
}

/// One claim-surface operation (`Method::Claims`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ClaimOp {
    /// Create a claim in `proposed`.
    Propose { claim_id: String },
    /// Move a claim along one lifecycle edge.
    Advance { claim_id: String, to: ClaimState },
    /// Add one piece of evidence, with optional perspective stances and an
    /// optional feed item to grade.
    RecordEvidence {
        evidence_id: String,
        claim_id: String,
        #[serde(default)]
        channel: Option<EvidenceChannel>,
        supports: bool,
        weight_ppm: u32,
        source_digest: String,
        #[serde(default)]
        stances: BTreeMap<String, Stance>,
        #[serde(default)]
        feed: Option<FeedItem>,
        /// A machine-checked proof receipt (MEDULLA-DECISION-ENGINE-R652.10).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        proof: Option<ProofReceipt>,
    },
    /// Seal and store a research artifact over a cohort.
    CompileArtifact {
        artifact_id: String,
        cohort_id: String,
        claim_ids: Vec<String>,
        code_specs: Vec<String>,
        exploration_edges: Vec<(String, String)>,
    },
    /// Read one claim or one sealed artifact (exactly one id).
    Get {
        #[serde(default)]
        claim_id: Option<String>,
        #[serde(default)]
        artifact_id: Option<String>,
    },
}

impl ClaimOp {
    /// Whether this operation changes durable state.
    pub fn is_mutation(&self) -> bool {
        !matches!(self, ClaimOp::Get { .. })
    }

    /// The authorization action this operation needs.
    pub fn authz_action(&self) -> &'static str {
        if self.is_mutation() {
            "agent:claim-write"
        } else {
            "agent:claim-read"
        }
    }
}

/// A claim as stored and as read back: state, logs, evidence and pairs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ClaimRecord {
    pub tenant_id: String,
    pub claim_id: String,
    pub state: ClaimState,
    /// Signed log-odds in micro-nats over the relevant evidence.
    pub log_odds_micro: i64,
    pub transitions: Vec<LifecycleTransition>,
    pub evidence: Vec<Evidence>,
    pub stances: BTreeMap<String, Stance>,
    pub contradiction_pairs: Vec<(String, String)>,
    pub feeds: Vec<FeedGrade>,
}

/// The answer of `Claims.Get`: the claim or the artifact that was asked for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ClaimGetResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim: Option<ClaimRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<ResearchArtifact>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_evidence() -> Evidence {
        Evidence {
            tenant_id: "t1".to_string(),
            evidence_id: "e1".to_string(),
            claim_id: "c1".to_string(),
            channel: EvidenceChannel::Outcome,
            supports: true,
            weight_ppm: 750_000,
            source_digest: "sha256:aa".to_string(),
            proof: None,
        }
    }

    fn sample_receipt() -> ProofReceipt {
        ProofReceipt {
            checker_id: "kernel-checker".to_string(),
            toolchain_version: "1.2.3".to_string(),
            declaration: "Mathlib.Foo.bar".to_string(),
            artifact_digest: "ab".repeat(32),
            axioms: vec!["propext".to_string(), "quot.sound".to_string()],
            has_sorry: false,
            verdict: ProofVerdict::Pass,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R652.10
    #[test]
    fn proof_receipt_round_trips_and_refuses_invalid() {
        let mut evidence = Evidence {
            channel: EvidenceChannel::Process,
            proof: Some(sample_receipt()),
            ..sample_evidence()
        };
        // Existing record bytes are unchanged: no `proof` key without a receipt.
        let plain = serde_json::to_value(sample_evidence()).unwrap();
        assert!(plain.get("proof").is_none());
        // A receipt round-trips and passes the check.
        let json = serde_json::to_value(&evidence).unwrap();
        assert_eq!(json["proof"]["verdict"], "pass");
        let back: Evidence = serde_json::from_value(json).unwrap();
        assert_eq!(back, evidence);
        assert!(evidence.check().is_ok());
        // The receipt's axioms normalise to sorted, deduplicated order.
        let messy = ProofReceipt {
            axioms: vec!["b".into(), "a".into(), "b".into()],
            ..sample_receipt()
        }
        .normalised();
        assert_eq!(messy.axioms, ["a", "b"]);

        // Off the process channel, or not supporting: PROOF_RECEIPT_INVALID.
        let mut off_channel = evidence.clone();
        off_channel.channel = EvidenceChannel::Outcome;
        assert_eq!(off_channel.check(), Err(ServerErrorCode::ProofReceiptInvalid));
        let mut opposing = evidence.clone();
        opposing.supports = false;
        assert_eq!(opposing.check(), Err(ServerErrorCode::ProofReceiptInvalid));

        // A malformed receipt: an empty field or a bad digest.
        for broken in [
            ProofReceipt { checker_id: String::new(), ..sample_receipt() },
            ProofReceipt { toolchain_version: String::new(), ..sample_receipt() },
            ProofReceipt { declaration: String::new(), ..sample_receipt() },
            ProofReceipt { artifact_digest: "ab".repeat(31), ..sample_receipt() },
            ProofReceipt { artifact_digest: "AB".repeat(32), ..sample_receipt() },
            ProofReceipt { artifact_digest: "zz".repeat(32), ..sample_receipt() },
        ] {
            evidence.proof = Some(broken);
            assert_eq!(evidence.check(), Err(ServerErrorCode::ProofReceiptInvalid));
        }

        // A pass that still holds an unfinished step: PROOF_SORRY_PRESENT; a
        // failing verdict may carry it.
        evidence.proof = Some(ProofReceipt { has_sorry: true, ..sample_receipt() });
        assert_eq!(evidence.check(), Err(ServerErrorCode::ProofSorryPresent));
        evidence.proof = Some(ProofReceipt {
            has_sorry: true,
            verdict: ProofVerdict::Fail,
            ..sample_receipt()
        });
        assert!(evidence.check().is_ok());
    }

    // spec: MEDULLA-DECISION-ENGINE-R652.1
    #[test]
    fn claim_types_round_trip_and_deny_unknown_fields() {
        let transition = LifecycleTransition {
            tenant_id: "t1".to_string(),
            claim_id: "c1".to_string(),
            from: ClaimState::Proposed,
            to: ClaimState::Supported,
            at_ms: 42,
            actor: "alice".to_string(),
        };
        let cohort = Cohort {
            tenant_id: "t1".to_string(),
            cohort_id: "k1".to_string(),
            claim_ids: vec!["c1".to_string()],
        };
        let mut stances = BTreeMap::new();
        stances.insert("p1".to_string(), Stance::Supports);
        stances.insert("p2".to_string(), Stance::Opposes);
        let perspectives = PerspectiveMap {
            tenant_id: "t1".to_string(),
            claim_id: "c1".to_string(),
            stances,
        };
        let artifact = ResearchArtifact {
            tenant_id: "t1".to_string(),
            artifact_id: "a1".to_string(),
            cohort_id: "k1".to_string(),
            claim_ids: vec!["c1".to_string()],
            code_specs: vec!["spec-1".to_string()],
            exploration_edges: vec![("c1".to_string(), "c2".to_string())],
            seal: Some(ArtifactSeal {
                digest: "0".repeat(64),
            }),
        };
        let op = ClaimOp::Advance {
            claim_id: "c1".to_string(),
            to: ClaimState::Accepted,
        };

        macro_rules! round_trip {
            ($value:expr, $ty:ty) => {{
                let json = serde_json::to_value(&$value).unwrap();
                let back: $ty = serde_json::from_value(json.clone()).unwrap();
                assert_eq!(back, $value);
                let mut with_extra = json;
                with_extra
                    .as_object_mut()
                    .unwrap()
                    .insert("unexpected".to_string(), serde_json::json!(1));
                assert!(serde_json::from_value::<$ty>(with_extra).is_err());
            }};
        }
        round_trip!(transition, LifecycleTransition);
        round_trip!(sample_evidence(), Evidence);
        round_trip!(cohort, Cohort);
        round_trip!(perspectives, PerspectiveMap);
        round_trip!(artifact, ResearchArtifact);
        round_trip!(op, ClaimOp);
        round_trip!(
            ArtifactSeal {
                digest: "f".repeat(64)
            },
            ArtifactSeal
        );

        for state in ClaimState::ALL {
            let json = serde_json::to_value(state).unwrap();
            assert_eq!(json, serde_json::json!(state.as_str()));
            assert_eq!(ClaimState::ALL[state.index()], state);
        }
        for channel in ["outcome", "optimization", "health", "process", "mined"] {
            let parsed: EvidenceChannel =
                serde_json::from_value(serde_json::json!(channel)).unwrap();
            assert_eq!(
                serde_json::to_value(parsed).unwrap(),
                serde_json::json!(channel)
            );
        }
        assert!(serde_json::from_value::<EvidenceChannel>(serde_json::json!("other")).is_err());

        let mut heavy = sample_evidence();
        assert!(heavy.check().is_ok());
        heavy.weight_ppm = WEIGHT_PPM_MAX;
        assert!(heavy.check().is_ok());
        heavy.weight_ppm = WEIGHT_PPM_MAX + 1;
        assert_eq!(
            heavy.check(),
            Err(ServerErrorCode::EvidenceWeightOutOfRange)
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R652.4
    #[test]
    fn refusal_codes_are_registered_server_error_codes() {
        let mut previous = "";
        for (code, _) in REFUSALS {
            assert!(code.as_str() > previous, "REFUSALS is sorted by code");
            previous = code.as_str();
            assert!(ServerErrorCode::ALL
                .iter()
                .any(|c| c.as_str() == code.as_str()));
        }
    }
}
