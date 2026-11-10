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
