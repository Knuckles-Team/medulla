//! The closed refusal vocabulary of the decision surface.
//!
//! Each code is the `"CODE: detail"` prefix of an error response, so a caller
//! branches on the code and reads the detail, rather than matching prose.

use medulla_types::closed_error_codes;

closed_error_codes! {
    /// Every typed refusal the assembly, commit and statistical decision
    /// surfaces can answer with.
    pub enum DecisionErrorCode {
        /// The catalog moved between the assembly read and the commit.
        StaleCatalog => "STALE_CATALOG",
        /// Re-deriving the record on commit produced a different digest.
        DecisionReplayMismatch => "DECISION_REPLAY_MISMATCH",
        /// The certificate in the record does not verify against its model.
        CertificateRejected => "CERTIFICATE_REJECTED",
        /// A pinned candidate's definition digest no longer matches.
        CandidateFactsChanged => "CANDIDATE_FACTS_CHANGED",
        /// The record declares a schema version this engine does not serve.
        DecisionRecordVersionUnsupported => "DECISION_RECORD_VERSION_UNSUPPORTED",
        /// The encoded record is larger than the durable record bound.
        RecordTooLarge => "RECORD_TOO_LARGE",
        /// A request tried to widen, not tighten, the pinned policy.
        PolicyLoosening => "POLICY_LOOSENING",
        /// Exploration was requested for a question that forbids it.
        ExplorationForbidden => "EXPLORATION_FORBIDDEN",
        /// Publishing a decision head needs its evaluation receipt digest.
        EvaluationReceiptRequired => "EVALUATION_RECEIPT_REQUIRED",
        /// The inputs of an assembly are not self-consistent: an unsorted or
        /// out-of-scope candidate list, a digest that does not match what it
        /// digests, or a component kind no agent slot accepts.
        AssemblyInputsInvalid => "ASSEMBLY_INPUTS_INVALID",
        /// The integer programme built from valid inputs was refused by the
        /// solver's own model validation (a coefficient outside its range).
        AssemblyModelInvalid => "ASSEMBLY_MODEL_INVALID",
        /// A pinned decision policy's body is not readable by this engine.
        PolicyBodyUnavailable => "POLICY_BODY_UNAVAILABLE",
        /// The candidate scope holds more components than one record may.
        CandidateScopeTooLarge => "CANDIDATE_SCOPE_TOO_LARGE",
        /// The record is structurally valid but not the decision the engine
        /// would commit: a derivation or evidence class does not re-check.
        DerivationRejected => "DERIVATION_REJECTED",
        /// A required input media type is not declared by the selected
        /// model profile, or the profile declares no input capabilities.
        ModelMediaCapabilityUnknown => "MODEL_MEDIA_CAPABILITY_UNKNOWN",
        /// MEDULLA-DECISION-ENGINE-R036.103: a durable reservation holds the authorization image.
        RbacReserved => "RBAC_RESERVED",
        /// MEDULLA-DECISION-ENGINE-R036.103: the reserve call named a revision that is no longer current.
        RbacRevisionStale => "RBAC_REVISION_STALE",
        /// MEDULLA-DECISION-ENGINE-R036.103: a release named a transaction that holds no reservation.
        RbacReservationUnknown => "RBAC_RESERVATION_UNKNOWN",
        /// MEDULLA-DECISION-ENGINE-R036.106: the RBAC file is a predecessor layout.
        RbacLayoutOutdated => "RBAC_LAYOUT_OUTDATED",
        /// MEDULLA-DECISION-ENGINE-R036.106: the RBAC file changed after its upgrade inspection.
        RbacUpgradeSourceChanged => "RBAC_UPGRADE_SOURCE_CHANGED",
        /// MEDULLA-DECISION-ENGINE-R036.106: a revoke named a grant that does not exist.
        RbacGrantUnknown => "RBAC_GRANT_UNKNOWN",
        /// MEDULLA-DECISION-ENGINE-R036.104: a reserved component head changed.
        AdmissionHeadChanged => "ADMISSION_HEAD_CHANGED",
        /// MEDULLA-DECISION-ENGINE-R036.104: a component the admission rests on was withdrawn.
        AdmissionComponentWithdrawn => "ADMISSION_COMPONENT_WITHDRAWN",
        /// MEDULLA-DECISION-ENGINE-R036.104: a changed intent was sent for an existing transaction.
        AdmissionIntentChanged => "ADMISSION_INTENT_CHANGED",
        /// MEDULLA-DECISION-ENGINE-R036.104: a component write is fenced by a reservation.
        AdmissionWriteBlocked => "ADMISSION_WRITE_BLOCKED",
        /// MEDULLA-DECISION-ENGINE-R036.104: a new attempt on an aborted transaction.
        AdmissionAborted => "ADMISSION_ABORTED",
        /// MEDULLA-DECISION-ENGINE-R036.104: a decision for an unknown transaction.
        AdmissionNotReserved => "ADMISSION_NOT_RESERVED",
    }
}
