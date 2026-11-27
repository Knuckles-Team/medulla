//! Shared wire refusal codes. Domain-specific vocabularies remain beside their
//! result types and are included in the generated error catalog.

use crate::closed_error_codes;

closed_error_codes! {
    /// Refusals emitted before a request reaches its domain handler.
    pub enum EngineErrorCode {
        InvalidArgument => "INVALID_ARGUMENT",
        AccessDenied => "ACCESS_DENIED",
        Conflict => "CONFLICT",
        IdempotencyConflict => "IDEMPOTENCY_CONFLICT",
        Redirected => "REDIRECTED",
        OperationRedirected => "OPERATION_REDIRECTED",
        ReadOnly => "READ_ONLY",
        AuthTenantMismatch => "AUTH_TENANT_MISMATCH",
        AuthAudienceMismatch => "AUTH_AUDIENCE_MISMATCH",
        AuthPolicyVersionMismatch => "AUTH_POLICY_VERSION_MISMATCH",
        AuthExpired => "AUTH_EXPIRED",
        AuthBadSignature => "AUTH_BAD_SIGNATURE",
        AuthUnknownPrincipal => "AUTH_UNKNOWN_PRINCIPAL",
        NodeMismatch => "NODE_MISMATCH",
        EngineInternalMethod => "ENGINE_INTERNAL_METHOD",
        CarrierInnerMethodRefused => "CARRIER_INNER_METHOD_REFUSED",
        PolicyNativeAuthorityRequired => "POLICY_NATIVE_AUTHORITY_REQUIRED",
        CapacityDenied => "CAPACITY_DENIED",
        /// A capacity cell's resource class does not admit the request's priority: a build-slot
        /// cell admits only `land_gate`, `train` and `lane`, and no other class admits them
        /// (MEDULLA-PLACEMENT-R786.1).
        PriorityNotAllowedForResource => "PRIORITY_NOT_ALLOWED_FOR_RESOURCE",
        UqlBudgetExceeded => "UQL_BUDGET_EXCEEDED",
        UqlUnsupported => "UQL_UNSUPPORTED",
        UqlCredentialBearingSpec => "UQL_CREDENTIAL_BEARING_SPEC",
        EngineUnavailable => "ENGINE_UNAVAILABLE",
        EngineResourceExhausted => "ENGINE_RESOURCE_EXHAUSTED",
        EngineDeadlineExceeded => "ENGINE_DEADLINE_EXCEEDED",
        /// A spec artifact line matched no known form (MEDULLA-SDLC-R014.1).
        SpecArtifactUnparseable => "SPEC_ARTIFACT_UNPARSEABLE",
        /// A plan heading `Phase <N>` names a phase outside the plan template's `0`, `1`
        /// and `2` (MEDULLA-SDLC-R014.7).
        PlanPhaseUnknown => "PLAN_PHASE_UNKNOWN",
        /// A requirement row names an ID that resolves to no row (MEDULLA-SDLC-R012.2).
        RequirementUnresolved => "REQUIREMENT_UNRESOLVED",
        /// A contract names a row that no spec holds (MEDULLA-SDLC-R014.4).
        ContractRowUnresolved => "CONTRACT_ROW_UNRESOLVED",
        /// A requirement row cites a `Principle:` key that resolves to no constitution
        /// principle of its repository (MEDULLA-SDLC-R014.6.2).
        PrincipleUnresolved => "PRINCIPLE_UNRESOLVED",
    }
}
