//! Stable UQL parser diagnostics shared by the planner and served error catalog.

use crate::closed_error_codes;

closed_error_codes! {
    /// UQL parser and binding refusals, plus the served `WITH KNOWLEDGE` row-frame
    /// refusals (MEDULLA-FEDERATED-QUERY-R028.1), exposed on the wire.
    pub enum UqlCode {
        UnexpectedCharacter => "UQL_UNEXPECTED_CHARACTER",
        UnterminatedString => "UQL_UNTERMINATED_STRING",
        UnterminatedIdentifier => "UQL_UNTERMINATED_IDENTIFIER",
        InvalidNumber => "UQL_INVALID_NUMBER",
        EmptyParameterName => "UQL_EMPTY_PARAMETER_NAME",
        UnexpectedToken => "UQL_UNEXPECTED_TOKEN",
        UnknownStage => "UQL_UNKNOWN_STAGE",
        ExpectedInteger => "UQL_EXPECTED_INTEGER",
        TrailingTokens => "UQL_TRAILING_TOKENS",
        NestingTooDeep => "UQL_NESTING_TOO_DEEP",
        InvalidRange => "UQL_INVALID_RANGE",
        /// `PROPAGATE ... RISK HISTORY` (MEDULLA-FEDERATED-QUERY-R032.100): a layer weight
        /// outside `[0, 1]`, a duplicate layer, more than 16 layers or `to_ms <= from_ms`.
        RiskHistory => "UQL_E_RISK_HISTORY",
        DecisionClauseInUql => "DECISION_CLAUSE_IN_UQL",
        UnsupportedVersion => "UQL_UNSUPPORTED_VERSION",
        DuplicateBinding => "UQL_DUPLICATE_BINDING",
        UnknownBinding => "UQL_UNKNOWN_BINDING",
        UnusedBinding => "UQL_UNUSED_BINDING",
        NullLiteral => "UQL_NULL_LITERAL",
        StatementNotPipeline => "UQL_STATEMENT_NOT_PIPELINE",
        CredentialBearingSpec => "UQL_CREDENTIAL_BEARING_SPEC",
        UnknownChannel => "UQL_UNKNOWN_CHANNEL",
        UnknownFunction => "UQL_UNKNOWN_FUNCTION",
        UnboundParameter => "UQL_UNBOUND_PARAMETER",
        ParameterType => "UQL_PARAMETER_TYPE",
        UnusedParameter => "UQL_UNUSED_PARAMETER",
        FeatureNotInBuild => "UQL_FEATURE_NOT_IN_BUILD",
        /// `EVALUATE kernel ...` (MEDULLA-FEDERATED-QUERY-R726): the kernel name is not
        /// one of `medulla_query_types::series_expr::EVALUATE_KERNELS`. Refused at parse time.
        EvaluateKernel => "UQL_E_EVALUATE_KERNEL",
        /// `COMPANY CONTEXT CATEGORY name` (MEDULLA-FEDERATED-QUERY-R078.7): the name is
        /// not one of `organization`, `role`, `data_asset`, `permission`, `time_series`.
        /// Refused at parse time.
        CompanyCategory => "UQL_E_COMPANY_CATEGORY",
        /// `REQUIRE NONEMPTY 'CODE'` (MEDULLA-FEDERATED-QUERY-R720.7): the code does not match
        /// `^[A-Z][A-Z0-9_]{2,63}$`. Refused at parse time.
        RequireCodeInvalid => "UQL_E_REQUIRE_CODE_INVALID",
        /// `WITH KNOWLEDGE` (MEDULLA-FEDERATED-QUERY-R028.1): a row's provenance or
        /// policy frame was omitted — no snapshot binding. A typed absence, never an
        /// empty success; the annotated result is refused whole.
        KnowledgeFrameMissing => "KNOWLEDGE_FRAME_MISSING",
        /// `WITH KNOWLEDGE` (MEDULLA-FEDERATED-QUERY-R028.1): a row's frame came from
        /// a different snapshot than the pinned one. The annotated result is refused
        /// whole, never served partially.
        KnowledgeFrameStale => "KNOWLEDGE_FRAME_STALE",
    }
}
