//! The closed refusal vocabulary of the statistical decision surface.
//!
//! Kept beside the statistical types rather than folded into the assembly
//! surface's `DecisionErrorCode`: these refusals are about feature schemas,
//! heads, datasets and jobs, none of which the assembly layer has.

use medulla_types::closed_error_codes;

closed_error_codes! {
    /// Every typed refusal `Decide`, `DecisionFit` and `DecisionEval` answer with.
    pub enum StatisticalErrorCode {
        /// A pinned component is missing, of the wrong kind, or at another revision.
        ComponentPinMismatch => "COMPONENT_PIN_MISMATCH",
        /// A feature schema body is absent, malformed or of an unserved version.
        FeatureSchemaInvalid => "FEATURE_SCHEMA_INVALID",
        /// A head body is absent, malformed, or reads another feature schema.
        HeadInvalid => "HEAD_INVALID",
        /// The visible candidate set exceeds the decision bound.
        CandidateSetTooLarge => "CANDIDATE_SET_TOO_LARGE",
        /// A graph candidate plan ranks or truncates before visibility.
        CandidatePlanRefused => "CANDIDATE_PLAN_REFUSED",
        /// A typed parameter a feature names is absent or of the wrong type.
        ParameterInvalid => "PARAMETER_INVALID",
        /// A labelled dataset is malformed or does not match its pin.
        DatasetInvalid => "DATASET_INVALID",
        /// No admissible labelled item remains after the training filter.
        NoAdmissibleLabels => "NO_ADMISSIBLE_LABELS",
        /// The same idempotency key was reused for a different request.
        IdempotencyConflict => "IDEMPOTENCY_CONFLICT",
        /// No job with that id exists for this tenant.
        JobNotFound => "JOB_NOT_FOUND",
        /// An evaluation names a draft or head this tenant does not hold.
        EvalCandidateNotFound => "EVAL_CANDIDATE_NOT_FOUND",
        /// A decision-head publish names a receipt that is absent, failed, or
        /// qualified a different head.
        EvaluationReceiptMismatch => "EVALUATION_RECEIPT_MISMATCH",
        /// A numeric kernel refused its input.
        NumericRefused => "NUMERIC_REFUSED",
        /// A replay step read data recorded at or after the step it decided.
        LookAhead => "LOOK_AHEAD",
        /// A replay was asked of a policy-dependent environment or of
        /// bandit-labelled data; the off-policy estimators are the answer.
        ReplayPolicyDependent => "REPLAY_POLICY_DEPENDENT",
        /// A replay declared fewer trials than its search log records.
        TrialsUnderstated => "TRIALS_UNDERSTATED",
        /// A replay's folds, budget or data cannot be replayed.
        ReplaySpecInvalid => "REPLAY_SPEC_INVALID",
        /// A declared ingestion-lane candidate names a lane outside the
        /// fixed registry (`IngestionLane::ALL`), refused before any
        /// decision runs (MEDULLA-DECISION-ENGINE-R030).
        UnsupportedIngestionLane => "UNSUPPORTED_INGESTION_LANE",
        /// A fit request names a calibration method the fit does not serve:
        /// only `Conformal`, `Temperature` and `Isotonic` are accepted
        /// (MEDULLA-DECISION-ENGINE-R032.2.1).
        CalibrationMethodUnsupported => "CALIBRATION_METHOD_UNSUPPORTED",
        /// Every stage of a route pipeline declined the question
        /// (MEDULLA-DECISION-ENGINE-R035.750).
        RouteNoStage => "ROUTE_NO_STAGE",
        /// An isotonic calibration was asked of a head over more than two
        /// classes (MEDULLA-DECISION-ENGINE-R032.2.1).
        IsotonicRequiresBinary => "ISOTONIC_REQUIRES_BINARY",
        /// A declared enrichment-schedule candidate names a negative
        /// `enrichment.cost_q32` (MEDULLA-DECISION-ENGINE-R031).
        UnsupportedEnrichmentCost => "UNSUPPORTED_ENRICHMENT_COST",
        /// A run outcome cites a row that none of its referenced decisions
        /// returned (MEDULLA-DECISION-ENGINE-R100.102.4).
        CitationNotRetrieved => "CITATION_NOT_RETRIEVED",
        /// An importance weight lies outside [0, 1]
        /// (MEDULLA-DECISION-ENGINE-R151.4).
        ImportanceOutOfRange => "IMPORTANCE_OUT_OF_RANGE",
        /// An importance entry's evidence digest is empty or malformed
        /// (MEDULLA-DECISION-ENGINE-R151.4).
        ImportanceEvidenceMissing => "IMPORTANCE_EVIDENCE_MISSING",
        /// A penalty names no feature importance at all
        /// (MEDULLA-DECISION-ENGINE-R151.4).
        ImportanceFeaturesEmpty => "IMPORTANCE_FEATURES_EMPTY",
        /// An importance entry names a feature absent from the pinned feature
        /// schema (MEDULLA-DECISION-ENGINE-R151.4).
        ImportanceUnknownFeature => "IMPORTANCE_UNKNOWN_FEATURE",
        /// A generation cycle's model dimension is not the shadow
        /// generation's (MEDULLA-DECISION-ENGINE-R174.4).
        GenerationDimensionMismatch => "GENERATION_DIMENSION_MISMATCH",
        /// A generation cycle's evaluation receipt failed, so the cycle ended
        /// `rejected` and the pointer did not move
        /// (MEDULLA-DECISION-ENGINE-R174.4).
        GenerationReceiptFailed => "GENERATION_RECEIPT_FAILED",
        /// The logical graph's pointer moved while a cycle activated
        /// (MEDULLA-DECISION-ENGINE-R174.4).
        GenerationPointerConflict => "GENERATION_POINTER_CONFLICT",
        /// A thought-search question names no reasoning strategy in the
        /// pinned policy for its task class (MEDULLA-DECISION-ENGINE-R750.752).
        ThoughtStrategyUnknown => "THOUGHT_STRATEGY_UNKNOWN",
        /// A thought-search budget is 0 or above its knob maximum
        /// (MEDULLA-DECISION-ENGINE-R750.752).
        ThoughtBudgetInvalid => "THOUGHT_BUDGET_INVALID",
        /// A graph merge makes a thought its own descendant
        /// (MEDULLA-DECISION-ENGINE-R750.752).
        ThoughtMergeCycle => "THOUGHT_MERGE_CYCLE",
        /// A thought score is not on scale Q32 (MEDULLA-DECISION-ENGINE-R750.752).
        ThoughtScoreNotFixedPoint => "THOUGHT_SCORE_NOT_FIXED_POINT",
        /// A thought has a duplicate id, a missing parent or a depth that
        /// disagrees with its parent (MEDULLA-DECISION-ENGINE-R750.752).
        ThoughtNodeInvalid => "THOUGHT_NODE_INVALID",
        /// A context source is named twice, in the declared options or in
        /// `context.requested_sources` (MEDULLA-DECISION-ENGINE-R029.2.2).
        ContextSourceDuplicate => "CONTEXT_SOURCE_DUPLICATE",
        /// A context-budget question carries no positive `context.token_budget`
        /// (MEDULLA-DECISION-ENGINE-R029.2.2).
        ContextBudgetMissing => "CONTEXT_BUDGET_MISSING",
        /// A context source declares no usable `context.tokens`
        /// (MEDULLA-DECISION-ENGINE-R029.2.2).
        ContextTokensUnknown => "CONTEXT_TOKENS_UNKNOWN",
        /// An evaluation's gold-labelled set misses the empirical coverage the
        /// decision acts at (MEDULLA-DECISION-ENGINE-R032.2.3).
        ConformalCoverageUnmet => "CONFORMAL_COVERAGE_UNMET",
        /// An evaluation's gold-labelled set holds fewer pairs than
        /// `resolve_entity.gold_min_pairs` (MEDULLA-DECISION-ENGINE-R032.2.3).
        GoldSetTooSmall => "GOLD_SET_TOO_SMALL",
        /// A pinned instruction digest differs from the latest version of its
        /// set (MEDULLA-DECISION-ENGINE-R137.100).
        DisclosureVersionStale => "DISCLOSURE_VERSION_STALE",
        /// A named instruction set has no version in the caller's tenant
        /// graph (MEDULLA-DECISION-ENGINE-R137.100).
        DisclosureUnauthorized => "DISCLOSURE_UNAUTHORIZED",
        /// A scoped adapter fit names a corpus or capability scope that none
        /// of the tenant's judged retrievals belongs to
        /// (MEDULLA-DECISION-ENGINE-R174.7.4).
        AdapterScopeUnknown => "ADAPTER_SCOPE_UNKNOWN",
        /// Fewer held-out outcomes than the fit's evaluation needs
        /// (MEDULLA-DECISION-ENGINE-R174.7.4).
        AdapterInsufficientOutcomes => "ADAPTER_INSUFFICIENT_OUTCOMES",
        /// The fit's space digest is not the graph's embedding space
        /// (MEDULLA-DECISION-ENGINE-R174.7.4).
        AdapterSpaceMismatch => "ADAPTER_SPACE_MISMATCH",
        /// Rank times dimension exceeds `adapter.max_resident_bytes`
        /// (MEDULLA-DECISION-ENGINE-R174.7.4).
        AdapterMemoryBudget => "ADAPTER_MEMORY_BUDGET",
        /// Activating would leave more active adapters than
        /// `adapter.max_per_tenant` (MEDULLA-DECISION-ENGINE-R174.7.4).
        AdapterTooMany => "ADAPTER_TOO_MANY",
        /// A freshness refresh names no ACL policy
        /// (MEDULLA-DECISION-ENGINE-R174.9.4).
        RefreshAclPolicyMissing => "REFRESH_ACL_POLICY_MISSING",
        /// A freshness refresh names no privacy policy
        /// (MEDULLA-DECISION-ENGINE-R174.9.4).
        RefreshPrivacyPolicyMissing => "REFRESH_PRIVACY_POLICY_MISSING",
        /// A freshness refresh asks for more requests than its budget allows
        /// in the window (MEDULLA-DECISION-ENGINE-R174.9.4).
        RefreshRateBudgetExceeded => "REFRESH_RATE_BUDGET_EXCEEDED",
        /// A freshness refresh would spend more cost units than its budget
        /// (MEDULLA-DECISION-ENGINE-R174.9.4).
        RefreshCostBudgetExceeded => "REFRESH_COST_BUDGET_EXCEEDED",
        /// The logical graph has no registered search source
        /// (MEDULLA-DECISION-ENGINE-R174.9.4).
        SearchSourceUnregistered => "SEARCH_SOURCE_UNREGISTERED",
    }
}

impl StatisticalErrorCode {
    /// `"CODE: detail"`, the one refusal text form.
    pub fn refusal(self, detail: impl std::fmt::Display) -> String {
        format!("{}: {detail}", self.as_str())
    }
}
