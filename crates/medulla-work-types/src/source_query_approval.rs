//! Source-query approvals issued by the dedicated native issuer
//! (MEDULLA-FEDERATED-QUERY-R085.2). The lease grant holds identity and digests
//! only, never SQL text, parameter values or credentials, because control lease
//! reads serve grants to `lease:read` holders. The SQL and parameters travel in
//! the returned [`ApprovedSourceQuery`].
use serde::{Deserialize, Serialize};

use crate::control_lease::{ControlLeaseIssueOutcome, IssueControlLeaseRequest};
use medulla_result_types::source_query::{
    ApprovedSourceQuery, SourceQueryError, SourceQueryFailure, SourceQueryOperation,
    SourceQueryRequest, SourceQueryStep, SOURCE_QUERY_SCHEMA_VERSION,
};

/// The control lease kind of a source-query approval.
pub const SOURCE_QUERY_APPROVAL_KIND: &str = "source.query.approval";
/// The key of the grant object that holds the [`SourceQueryApprovalGrant`].
pub const SOURCE_QUERY_APPROVAL_GRANT_KEY: &str = "source_query_approval";

/// The authority the engine derived from the verified request context. A wire
/// value is overwritten by admission, never trusted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SourceQueryAdmission {
    pub approval_id: String,
    pub tenant_ref: String,
    pub owner_ref: String,
    pub actor_ref: String,
    pub authority_digest: String,
    pub policy_digest: String,
    pub identity_digest: String,
    pub approval_ttl_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ApproveSourceQueryRequest {
    pub tenant: String,
    pub query: SourceQueryRequest,
    #[serde(default)]
    pub admission: Option<SourceQueryAdmission>,
}

/// The identity and digests a lease grant carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SourceQueryApprovalGrant {
    pub schema_version: u16,
    pub approval_id: String,
    pub query_id: String,
    pub source_ref: String,
    pub tenant_ref: String,
    pub owner_ref: String,
    pub actor_ref: String,
    pub authority_digest: String,
    pub policy_digest: String,
    pub identity_digest: String,
    pub operation_digest: String,
    pub parameter_digest: String,
    pub query_digest: String,
}

/// The reply of `ApproveSourceQuery`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SourceQueryApprovalIssued {
    pub outcome: ControlLeaseIssueOutcome,
    pub approved: Option<ApprovedSourceQuery>,
    pub changed_work_item_ids: Vec<String>,
}

/// A refusal in the `SourceQueryFailure` shape (MEDULLA-FEDERATED-QUERY-R085.7), failed step
/// `approval`, naming the query by its operation digest. The text starts with the closed code
/// `SOURCE_QUERY_<KIND>` and never carries SQL text or a parameter value.
pub fn approval_refusal(kind: SourceQueryError, query: &SourceQueryRequest) -> String {
    let identity = query
        .operation_digest()
        .unwrap_or_else(|_| "0".repeat(64));
    source_query_refusal(kind, &identity, SourceQueryStep::Approval)
}

/// A refusal in the `SourceQueryFailure` shape (MEDULLA-FEDERATED-QUERY-R085.7) for `step`,
/// naming the query by `identity`; an identity that is not a 64-hex digest is replaced by 64
/// zeros so no literal can ride in the reply. The text starts with `SOURCE_QUERY_<KIND>`.
pub fn source_query_refusal(kind: SourceQueryError, identity: &str, step: SourceQueryStep) -> String {
    let failure = SourceQueryFailure::new(kind, identity, step, None)
        .or_else(|_| SourceQueryFailure::new(kind, &"0".repeat(64), step, None))
        .expect("64 zeros are a 64-hex digest");
    format!("SOURCE_QUERY_{}: {failure}", kind.to_string().to_uppercase())
}

impl SourceQueryApprovalGrant {
    /// The grant an approval implies: identity and digests only, never SQL or parameters.
    pub fn of(approved: &ApprovedSourceQuery) -> Self {
        Self {
            schema_version: approved.schema_version,
            approval_id: approved.approval_id.clone(),
            query_id: approved.query_id.clone(),
            source_ref: approved.request.source_ref.clone(),
            tenant_ref: approved.tenant_ref.clone(),
            owner_ref: approved.owner_ref.clone(),
            actor_ref: approved.actor_ref.clone(),
            authority_digest: approved.authority_digest.clone(),
            policy_digest: approved.policy_digest.clone(),
            identity_digest: approved.identity_digest.clone(),
            operation_digest: approved.operation_digest.clone(),
            parameter_digest: approved.parameter_digest.clone(),
            query_digest: approved.query_digest.clone(),
        }
    }
}

impl ApproveSourceQueryRequest {
    /// Build the lease and the approval at engine commit time. `admission: None` refuses
    /// `ACCESS_DENIED`. The lease is checked with `validate_body`, not `validate`, because
    /// the generic issue path stays closed to this kind.
    pub fn lease_at(
        &self,
        committed_at_ms: u64,
    ) -> Result<(IssueControlLeaseRequest, ApprovedSourceQuery), String> {
        let admission = self
            .admission
            .as_ref()
            .ok_or("ACCESS_DENIED: source query admission is missing")?;
        let invalid = || approval_refusal(SourceQueryError::InvalidQuery, &self.query);
        let operation_digest = self.query.operation_digest().map_err(|_| invalid())?;
        let parameter_digest = self.query.parameter_digest().map_err(|_| invalid())?;
        let hard_expires_at_ms = committed_at_ms.saturating_add(admission.approval_ttl_ms);
        let expires_at_ms = self.query.requested_expires_at_ms.min(hard_expires_at_ms);
        if expires_at_ms <= committed_at_ms {
            return Err(invalid());
        }
        let mut approved = ApprovedSourceQuery {
            schema_version: SOURCE_QUERY_SCHEMA_VERSION,
            approval_id: admission.approval_id.clone(),
            query_id: operation_digest.clone(),
            request: self.query.clone(),
            tenant_ref: admission.tenant_ref.clone(),
            owner_ref: admission.owner_ref.clone(),
            actor_ref: admission.actor_ref.clone(),
            authority_digest: admission.authority_digest.clone(),
            policy_digest: admission.policy_digest.clone(),
            identity_digest: admission.identity_digest.clone(),
            allowed_operations: vec![SourceQueryOperation::ReadSql],
            issued_at_ms: committed_at_ms,
            expires_at_ms,
            hard_expires_at_ms,
            operation_digest,
            parameter_digest,
            query_digest: String::new(),
        };
        approved.query_digest = approved.compute_query_digest().map_err(|_| invalid())?;
        approved.validate().map_err(|kind| approval_refusal(kind, &self.query))?;
        let grant = SourceQueryApprovalGrant::of(&approved);
        let mut body = serde_json::Map::new();
        body.insert(
            SOURCE_QUERY_APPROVAL_GRANT_KEY.into(),
            serde_json::to_value(&grant).map_err(|error| error.to_string())?,
        );
        let lease = IssueControlLeaseRequest {
            tenant: self.tenant.clone(),
            lease_id: approved.approval_id.clone(),
            kind: SOURCE_QUERY_APPROVAL_KIND.into(),
            grant: body,
            pending_call_registration: None,
            issued_at_ms: committed_at_ms,
            expires_at_ms,
            hard_expires_at_ms,
            idempotency_key: self.query.idempotency_key.clone(),
        };
        lease.validate_body()?;
        Ok((lease, approved))
    }
}
