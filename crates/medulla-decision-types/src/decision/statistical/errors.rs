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
        /// A replayed event's pinned component snapshot is not the revision
        /// the component held at the event's time (MEDULLA-DECISION-ENGINE-R088.2.2).
        ReplaySnapshotChanged => "REPLAY_SNAPSHOT_CHANGED",
        /// A replay event's sequence number skips ahead of the next expected
        /// one (MEDULLA-DECISION-ENGINE-R088.2.1).
        ReplaySequenceGap => "REPLAY_SEQUENCE_GAP",
        /// A replay event's sequence number is behind the last applied one
        /// and is not a repeat of an applied event.
        ReplayOutOfOrder => "REPLAY_OUT_OF_ORDER",
        /// A replay event repeats an applied sequence number with different
        /// content.
        ReplayDuplicateConflict => "REPLAY_DUPLICATE_CONFLICT",
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
        /// A schema-mapping question names no column, or an empty one
        /// (MEDULLA-DECISION-ENGINE-R033.2.2).
        SchemaMappingColumnMissing => "SCHEMA_MAPPING_COLUMN_MISSING",
        /// A schema-mapping column names an SQL type outside the closed set
        /// (MEDULLA-DECISION-ENGINE-R033.2.2).
        SchemaMappingSqlTypeUnknown => "SCHEMA_MAPPING_SQL_TYPE_UNKNOWN",
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
        /// A graph evidence row names a `source` that is not an IRI
        /// (MEDULLA-DECISION-ENGINE-R121.2).
        GraphProvenanceMalformed => "GRAPH_PROVENANCE_MALFORMED",
        /// A recorded graph version is older than the node history the graph still
        /// retains, so a row cannot be re-read at it (MEDULLA-DECISION-ENGINE-R121.2).
        GraphVersionRetired => "GRAPH_VERSION_RETIRED",
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
        /// A disclosure event request names no known trigger
        /// (MEDULLA-DECISION-ENGINE-R143.4).
        DisclosureTriggerMissing => "DISCLOSURE_TRIGGER_MISSING",
        /// A disclosure event's `expected_hash` differs from the computed hash
        /// (MEDULLA-DECISION-ENGINE-R143.4).
        DisclosureHashMismatch => "DISCLOSURE_HASH_MISMATCH",
        /// A disclosure event option carries no `instructions` text
        /// (MEDULLA-DECISION-ENGINE-R143.4).
        DisclosureSetUndefined => "DISCLOSURE_SET_UNDEFINED",
        /// A non-initial disclosure trigger found no prior event of its set
        /// (MEDULLA-DECISION-ENGINE-R143.4).
        DisclosurePriorMissing => "DISCLOSURE_PRIOR_MISSING",
        /// The `initial` disclosure came with an event already recorded
        /// (MEDULLA-DECISION-ENGINE-R143.4).
        DisclosurePriorUnexpected => "DISCLOSURE_PRIOR_UNEXPECTED",
        /// The prior event belongs to another instruction set
        /// (MEDULLA-DECISION-ENGINE-R143.4).
        DisclosureSetMismatch => "DISCLOSURE_SET_MISMATCH",
        /// A refinement's instructions hash equals the prior hash
        /// (MEDULLA-DECISION-ENGINE-R143.4).
        DisclosureHashUnchanged => "DISCLOSURE_HASH_UNCHANGED",
        /// A `timeout` disclosure before `disclosure.timeout` seconds elapsed
        /// (MEDULLA-DECISION-ENGINE-R143.4).
        DisclosureTimeoutNotElapsed => "DISCLOSURE_TIMEOUT_NOT_ELAPSED",
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
        /// A changeset review names no base or head revision
        /// (MEDULLA-DECISION-ENGINE-R138.101.2).
        ReviewRevisionMissing => "REVIEW_REVISION_MISSING",
        /// A changeset review names or cites a flaw class outside the five.
        ReviewClassUnsupported => "REVIEW_CLASS_UNSUPPORTED",
        /// A finding cites a class the review did not claim to cover.
        ReviewClassUnreviewed => "REVIEW_CLASS_UNREVIEWED",
        /// A finding carries no evidence, path or cited lines.
        ReviewProofAbsent => "REVIEW_PROOF_ABSENT",
        /// The changed-files digest is not the digest of the changed paths.
        ReviewChangesetDigestMismatch => "REVIEW_CHANGESET_DIGEST_MISMATCH",
        /// A finding cites a path the changeset does not contain.
        ReviewFindingOutsideChangeset => "REVIEW_FINDING_OUTSIDE_CHANGESET",
        /// A temporal-basis question has no usable temporal context
        /// (MEDULLA-DECISION-ENGINE-R146.4).
        TemporalContextMissing => "TEMPORAL_CONTEXT_MISSING",
        /// A temporal query's window is inverted (MEDULLA-DECISION-ENGINE-R146.4).
        TemporalWindowInvalid => "TEMPORAL_WINDOW_INVALID",
        /// A temporal-basis question's options are not event, narrative and
        /// storage (MEDULLA-DECISION-ENGINE-R146.4).
        TemporalBasisUnmappable => "TEMPORAL_BASIS_UNMAPPABLE",
        /// A delegation quality window ended before the maximum window age
        /// (MEDULLA-DECISION-ENGINE-R157.4).
        DelegationWindowStale => "DELEGATION_WINDOW_STALE",
        /// A delegation quality window is shorter than the minimum window
        /// (MEDULLA-DECISION-ENGINE-R157.4).
        DelegationWindowUnstable => "DELEGATION_WINDOW_UNSTABLE",
        /// A delegation quality threshold names an unknown metric or a value
        /// outside `[0, 1000000]` (MEDULLA-DECISION-ENGINE-R157.4).
        DelegationThresholdUndefined => "DELEGATION_THRESHOLD_UNDEFINED",
        /// A delegation quality window holds fewer observations than the
        /// minimum (MEDULLA-DECISION-ENGINE-R157.4).
        DelegationInsufficientSamples => "DELEGATION_INSUFFICIENT_SAMPLES",
        /// A distilled score is out of range or its fidelity bounds are
        /// inverted (MEDULLA-DECISION-ENGINE-R147.1).
        DistilledScoreInvalid => "DISTILLED_SCORE_INVALID",
        /// A candidate has no stored distilled score
        /// (MEDULLA-DECISION-ENGINE-R147.4).
        DistilledScoreMissing => "DISTILLED_SCORE_MISSING",
        /// The requested scorer revision is not the stored one
        /// (MEDULLA-DECISION-ENGINE-R147.4).
        ScorerRevisionStale => "SCORER_REVISION_STALE",
        /// A distilled ranking `top_k` is 0 or above `distilled.max_top_k`
        /// (MEDULLA-DECISION-ENGINE-R147.4).
        TopKInvalid => "TOP_K_INVALID",
        /// A distilled scorer holds no stored score
        /// (MEDULLA-DECISION-ENGINE-R147.4).
        DistilledScorerEmpty => "DISTILLED_SCORER_EMPTY",
        /// A used distilled score's low fidelity bound is under the floor
        /// (MEDULLA-DECISION-ENGINE-R147.4).
        DistilledFidelityBelowFloor => "DISTILLED_FIDELITY_BELOW_FLOOR",
        /// A ranking evaluation names no queries (MEDULLA-DECISION-ENGINE-R100.100.1).
        RankingQuerySetEmpty => "RANKING_QUERY_SET_EMPTY",
        /// A ranking evaluation names more queries than its knob allows
        /// (MEDULLA-DECISION-ENGINE-R100.100.1).
        RankingQuerySetTooLarge => "RANKING_QUERY_SET_TOO_LARGE",
        /// A pinned set's digest differs from the digest of its content
        /// (MEDULLA-DECISION-ENGINE-R100.100.1).
        RankingDigestMismatch => "RANKING_DIGEST_MISMATCH",
        /// The cutoff `k` is 0 or above its knob maximum
        /// (MEDULLA-DECISION-ENGINE-R100.100.1).
        RankingKOutOfRange => "RANKING_K_OUT_OF_RANGE",
        /// Fewer algorithm runs than the knob requires
        /// (MEDULLA-DECISION-ENGINE-R100.100.1).
        RankingAlgorithmsTooFew => "RANKING_ALGORITHMS_TOO_FEW",
        /// A query has no relevance pair (MEDULLA-DECISION-ENGINE-R100.100.1).
        RankingLabelsMissing => "RANKING_LABELS_MISSING",
        /// A run reports another corpus digest than the request's
        /// (MEDULLA-DECISION-ENGINE-R100.100.1).
        RankingCorpusDrift => "RANKING_CORPUS_DRIFT",
        /// A run ranks fewer, more or other queries than the request names
        /// (MEDULLA-DECISION-ENGINE-R100.100.1).
        RankingResultsIncomplete => "RANKING_RESULTS_INCOMPLETE",
        /// A run's input digest differs from the request's input digest
        /// (MEDULLA-DECISION-ENGINE-R100.100.1).
        RankingSettingsMismatch => "RANKING_SETTINGS_MISMATCH",
        /// A normalized-to-original offset map is not ordered, does not cover
        /// the text, or addresses bytes inside a character
        /// (MEDULLA-DECISION-ENGINE-R100.100.3).
        ContextMapInvalid => "CONTEXT_MAP_INVALID",
        /// A context search query has no tokens
        /// (MEDULLA-DECISION-ENGINE-R100.100.3).
        ContextQueryEmpty => "CONTEXT_QUERY_EMPTY",
        /// A context span is longer than `context_search.max_span_bytes`
        /// (MEDULLA-DECISION-ENGINE-R100.100.3).
        ContextSpanTooLarge => "CONTEXT_SPAN_TOO_LARGE",
        /// A context-search cursor is malformed, fails its check value, or
        /// belongs to another tenant or query (MEDULLA-DECISION-ENGINE-R100.100.4).
        ContextCursorInvalid => "CONTEXT_CURSOR_INVALID",
        /// A context-search cursor was issued against an older snapshot
        /// revision than the one being read (MEDULLA-DECISION-ENGINE-R100.100.4).
        ContextCursorStale => "CONTEXT_CURSOR_STALE",
        /// The task hierarchy is deeper than `knob htn.max_depth`
        /// (MEDULLA-DECISION-ENGINE-R751.752).
        RewardHtnTooDeep => "REWARD_HTN_TOO_DEEP",
        /// A hierarchy parent is not below its child, or a child's step range
        /// lies outside its parent's.
        RewardHtnInvalid => "REWARD_HTN_INVALID",
        /// Proposed rewards miss the trajectory reward by more than the tolerance.
        RewardSumMismatch => "REWARD_SUM_MISMATCH",
        /// An advantage was asked of fewer than two steps or of rewards with no
        /// deviation.
        RewardAdvantageUndefined => "REWARD_ADVANTAGE_UNDEFINED",
        /// More steps than `knob reward.max_steps`.
        RewardTooManySteps => "REWARD_TOO_MANY_STEPS",
        /// Fewer evaluations than the promotion window.
        CurriculumWindowIncomplete => "CURRICULUM_WINDOW_INCOMPLETE",
        /// A promotion threshold under `knob curriculum.promote_floor`.
        CurriculumThresholdBelowFloor => "CURRICULUM_THRESHOLD_BELOW_FLOOR",
        /// A drift window above `knob drift.max_samples`
        /// (MEDULLA-DECISION-ENGINE-R752.752).
        DriftWindowTooLarge => "DRIFT_WINDOW_TOO_LARGE",
        /// A population spread whose pairwise count is not `replicas*(replicas-1)/2`
        /// or whose entries lie outside [0, 1].
        PopulationSpreadInvalid => "POPULATION_SPREAD_INVALID",
        /// A step verification names no trace digest to bind its verdict to
        /// (MEDULLA-DECISION-ENGINE-R753.752).
        VerifyVerdictUnbound => "VERIFY_VERDICT_UNBOUND",
        /// The named trace digest differs from the recomputed one.
        VerifyTraceMismatch => "VERIFY_TRACE_MISMATCH",
        /// A trajectory join over no trajectory.
        JoinEmptySet => "JOIN_EMPTY_SET",
        /// An admission over no probe.
        ProbeSetEmpty => "PROBE_SET_EMPTY",
        /// An admission over more probes than `knob verify.probe_budget`.
        ProbeBudgetExceeded => "PROBE_BUDGET_EXCEEDED",
        /// A rule promotion names fewer human resolutions than
        /// `rule_promotion.min_overrides` (MEDULLA-DECISION-ENGINE-R129.5).
        RulePromotionTooFewOverrides => "RULE_PROMOTION_TOO_FEW_OVERRIDES",
        /// A rule promotion's human agreement is below
        /// `rule_promotion.min_agreement` (MEDULLA-DECISION-ENGINE-R129.5).
        RulePromotionAgreementLow => "RULE_PROMOTION_AGREEMENT_LOW",
        /// A rule promotion contradicts a published invariant; nothing is
        /// activated (MEDULLA-DECISION-ENGINE-R129.5).
        RuleContradictsInvariant => "RULE_CONTRADICTS_INVARIANT",
    }
}

impl StatisticalErrorCode {
    /// `"CODE: detail"`, the one refusal text form.
    pub fn refusal(self, detail: impl std::fmt::Display) -> String {
        format!("{}: {detail}", self.as_str())
    }
}
