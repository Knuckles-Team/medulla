//! Approved source queries (MEDULLA-FEDERATED-QUERY-R085.1): the records Medulla
//! issues so a connector may run exactly one read-only, natively
//! parameterized SQL statement against a registered source.
//!
//! Pure data and validation. Issuing, verifying and digesting approvals are
//! later children of MEDULLA-FEDERATED-QUERY-R085.

use serde::{Deserialize, Serialize};

/// Record version of every source-query record.
pub const SOURCE_QUERY_SCHEMA_VERSION: u16 = 1;
/// Upper bound on any single limit, so "unbounded" cannot be spelled as a huge value.
pub const MAX_SOURCE_QUERY_LIMIT: u64 = 1 << 40;
/// Most parameters one statement may bind.
pub const MAX_SOURCE_QUERY_PARAMETERS: usize = 1024;

/// The closed refusal kinds of a source-query record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SourceQueryError {
    #[error("unsupported_dialect")]
    UnsupportedDialect,
    #[error("invalid_query")]
    InvalidQuery,
    #[error("limit_exceeded")]
    LimitExceeded,
    #[error("unauthorized")]
    Unauthorized,
    #[error("approval_expired")]
    ApprovalExpired,
    #[error("approval_revoked")]
    ApprovalRevoked,
    #[error("stale_source")]
    StaleSource,
    #[error("query_failed")]
    QueryFailed,
    #[error("cancelled")]
    Cancelled,
    #[error("verification_unavailable")]
    VerificationUnavailable,
}

/// The step of a source query a failure happened in. Closed. A position, not a
/// lifecycle: a failure records it once and nothing moves it
/// (MEDULLA-FEDERATED-QUERY-R739.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SourceQueryStep {
    Validation,
    Approval,
    Verification,
    Execution,
    Delivery,
}

/// A failed source query as it appears in an error reply (MEDULLA-FEDERATED-QUERY-R085.7).
///
/// It names the closed kind, the failed step, the query by its digest and the last chunk
/// delivered before the failure. It never carries SQL text, a parameter value or a
/// credential: the identity is a 64-hex digest, and the formatted message is built
/// from the closed tokens and numbers only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[error("{kind} (query {query_identity}, failed step {failed_step:?}, last chunk {last_chunk:?})")]
pub struct SourceQueryFailure {
    pub kind: SourceQueryError,
    pub query_identity: String,
    pub failed_step: SourceQueryStep,
    /// Zero-based index of the last chunk delivered; `None` when none was.
    pub last_chunk: Option<u64>,
}

impl SourceQueryFailure {
    /// Build a failure. An identity that is not a lower-case 64-hex digest is refused,
    /// so no literal can ride in the identity field.
    pub fn new(
        kind: SourceQueryError,
        query_identity: &str,
        failed_step: SourceQueryStep,
        last_chunk: Option<u64>,
    ) -> Result<Self, SourceQueryError> {
        let failure = Self {
            kind,
            query_identity: query_identity.to_string(),
            failed_step,
            last_chunk,
        };
        failure.validate()?;
        Ok(failure)
    }

    /// Check the identity shape; the decoder of an error reply calls this too.
    pub fn validate(&self) -> Result<(), SourceQueryError> {
        let digest = self.query_identity.len() == 64
            && self
                .query_identity
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if digest {
            Ok(())
        } else {
            Err(SourceQueryError::InvalidQuery)
        }
    }
}

/// SQL dialects a source may declare. Closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SourceQueryDialect {
    Postgres,
    Mysql,
    Sqlite,
    Mssql,
    Clickhouse,
}

/// One natively bound parameter value. Closed kinds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum SourceQueryValue {
    Null,
    Bool(bool),
    Int64(i64),
    Float64(f64),
    Text(String),
    Bytes(
        #[cfg_attr(feature = "contract-schema", schemars(with = "Vec<u8>"))]
        #[serde(with = "serde_bytes")]
        Vec<u8>,
    ),
}

/// A parameter at a 1-based placeholder position. The typed value nests as
/// `{"kind", "value"}` so unknown fields still refuse at every level.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SourceQueryParameter {
    pub position: u32,
    pub value: SourceQueryValue,
}

/// Finite, positive execution limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SourceQueryLimits {
    pub max_rows: u64,
    pub max_bytes: u64,
    pub chunk_rows: u64,
    pub chunk_bytes: u64,
    pub queue_depth: u64,
    pub queue_bytes: u64,
    pub deadline_ms: u64,
    pub max_resident_bytes: u64,
}

impl SourceQueryLimits {
    pub fn validate(&self) -> Result<(), SourceQueryError> {
        let all = [
            self.max_rows,
            self.max_bytes,
            self.chunk_rows,
            self.chunk_bytes,
            self.queue_depth,
            self.queue_bytes,
            self.deadline_ms,
            self.max_resident_bytes,
        ];
        if all.iter().any(|v| *v == 0 || *v > MAX_SOURCE_QUERY_LIMIT) {
            return Err(SourceQueryError::LimitExceeded);
        }
        if self.chunk_rows > self.max_rows || self.chunk_bytes > self.max_bytes {
            return Err(SourceQueryError::LimitExceeded);
        }
        Ok(())
    }

    /// Clamp every caller value to the tenant `ceiling` (MEDULLA-FEDERATED-QUERY-R085.6).
    /// A zero caller value stays zero and refuses, as does a clamped set that
    /// [`Self::validate`] refuses; a value under its ceiling is kept as asked.
    pub fn clamp_to_ceiling(&self, ceiling: &SourceQueryLimits) -> Result<Self, SourceQueryError> {
        ceiling.validate()?;
        let clamped = Self {
            max_rows: self.max_rows.min(ceiling.max_rows),
            max_bytes: self.max_bytes.min(ceiling.max_bytes),
            chunk_rows: self.chunk_rows.min(ceiling.chunk_rows),
            chunk_bytes: self.chunk_bytes.min(ceiling.chunk_bytes),
            queue_depth: self.queue_depth.min(ceiling.queue_depth),
            queue_bytes: self.queue_bytes.min(ceiling.queue_bytes),
            deadline_ms: self.deadline_ms.min(ceiling.deadline_ms),
            max_resident_bytes: self.max_resident_bytes.min(ceiling.max_resident_bytes),
        };
        clamped.validate()?;
        Ok(clamped)
    }
}

/// A request for one read-only SQL operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SourceQueryRequest {
    pub schema_version: u16,
    pub source_ref: String,
    pub source_revision: String,
    pub schema_digest: String,
    pub snapshot_ref: String,
    pub dialect: SourceQueryDialect,
    pub sql: String,
    pub parameters: Vec<SourceQueryParameter>,
    pub limits: SourceQueryLimits,
    pub requested_expires_at_ms: u64,
    pub idempotency_key: String,
}

fn require(value: &str) -> Result<(), SourceQueryError> {
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        return Err(SourceQueryError::InvalidQuery);
    }
    Ok(())
}

/// Placeholders the statement declares: the highest `$n` for Postgres, the
/// count of `?` for the others. Quoted text is skipped.
fn placeholder_count(dialect: SourceQueryDialect, sql: &str) -> usize {
    let mut count = 0usize;
    let mut quoted = false;
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => quoted = !quoted,
            _ if quoted => {}
            '?' if dialect != SourceQueryDialect::Postgres => count += 1,
            '$' if dialect == SourceQueryDialect::Postgres => {
                let mut digits = String::new();
                while let Some(d) = chars.peek().filter(|d| d.is_ascii_digit()) {
                    digits.push(*d);
                    chars.next();
                }
                count = count.max(digits.parse().unwrap_or(0));
            }
            _ => {}
        }
    }
    count
}

impl SourceQueryRequest {
    /// Refuse missing identities, mismatched parameters and invalid limits.
    pub fn validate(&self) -> Result<(), SourceQueryError> {
        if self.schema_version != SOURCE_QUERY_SCHEMA_VERSION {
            return Err(SourceQueryError::InvalidQuery);
        }
        for field in [
            &self.source_ref,
            &self.source_revision,
            &self.schema_digest,
            &self.snapshot_ref,
            &self.sql,
            &self.idempotency_key,
        ] {
            require(field)?;
        }
        if self.requested_expires_at_ms == 0 {
            return Err(SourceQueryError::InvalidQuery);
        }
        self.limits.validate()?;
        self.validate_parameters()
    }

    fn validate_parameters(&self) -> Result<(), SourceQueryError> {
        if self.parameters.len() > MAX_SOURCE_QUERY_PARAMETERS
            || self.parameters.len() != placeholder_count(self.dialect, &self.sql)
        {
            return Err(SourceQueryError::InvalidQuery);
        }
        for (index, parameter) in self.parameters.iter().enumerate() {
            if parameter.position as usize != index + 1 {
                return Err(SourceQueryError::InvalidQuery);
            }
            if matches!(parameter.value, SourceQueryValue::Float64(v) if !v.is_finite()) {
                return Err(SourceQueryError::InvalidQuery);
            }
        }
        Ok(())
    }
}

/// The only operation an approval of this version allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SourceQueryOperation {
    ReadSql,
}

/// An immutable approval of one request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ApprovedSourceQuery {
    pub schema_version: u16,
    pub approval_id: String,
    pub query_id: String,
    pub request: SourceQueryRequest,
    pub tenant_ref: String,
    pub owner_ref: String,
    pub actor_ref: String,
    pub authority_digest: String,
    pub policy_digest: String,
    pub identity_digest: String,
    pub allowed_operations: Vec<SourceQueryOperation>,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
    pub hard_expires_at_ms: u64,
    pub operation_digest: String,
    pub parameter_digest: String,
    pub query_digest: String,
}

impl ApprovedSourceQuery {
    pub fn validate(&self) -> Result<(), SourceQueryError> {
        if self.schema_version != SOURCE_QUERY_SCHEMA_VERSION
            || self.allowed_operations != [SourceQueryOperation::ReadSql]
        {
            return Err(SourceQueryError::InvalidQuery);
        }
        for field in [
            &self.approval_id,
            &self.query_id,
            &self.tenant_ref,
            &self.owner_ref,
            &self.actor_ref,
            &self.authority_digest,
            &self.policy_digest,
            &self.identity_digest,
            &self.operation_digest,
            &self.parameter_digest,
            &self.query_digest,
        ] {
            require(field)?;
        }
        if !(self.issued_at_ms < self.expires_at_ms
            && self.expires_at_ms <= self.hard_expires_at_ms)
        {
            return Err(SourceQueryError::InvalidQuery);
        }
        self.request.validate()
    }
}

fn digest_of(domain: &[u8], fields: &[&[u8]]) -> Result<String, SourceQueryError> {
    medulla_types::contract::Digest256::framed(domain, fields)
        .map(|digest| digest.to_hex())
        .map_err(|_| SourceQueryError::InvalidQuery)
}

impl SourceQueryDialect {
    /// The serde snake_case name, used in digests.
    pub fn name(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::Mysql => "mysql",
            Self::Sqlite => "sqlite",
            Self::Mssql => "mssql",
            Self::Clickhouse => "clickhouse",
        }
    }
}

impl SourceQueryOperation {
    /// The serde snake_case name, used in digests.
    pub fn name(self) -> &'static str {
        match self {
            Self::ReadSql => "read_sql",
        }
    }
}

impl SourceQueryRequest {
    /// MEDULLA-FEDERATED-QUERY-R085.2: the framed digest of the bound parameters, three
    /// fields per parameter (position, kind name, value bytes).
    pub fn parameter_digest(&self) -> Result<String, SourceQueryError> {
        let mut owned: Vec<Vec<u8>> = Vec::with_capacity(self.parameters.len() * 3);
        for parameter in &self.parameters {
            owned.push(parameter.position.to_be_bytes().to_vec());
            let (kind, value): (&str, Vec<u8>) = match &parameter.value {
                SourceQueryValue::Null => ("null", Vec::new()),
                SourceQueryValue::Bool(v) => ("bool", vec![u8::from(*v)]),
                SourceQueryValue::Int64(v) => ("int64", v.to_be_bytes().to_vec()),
                SourceQueryValue::Float64(v) => ("float64", v.to_bits().to_be_bytes().to_vec()),
                SourceQueryValue::Text(v) => ("text", v.as_bytes().to_vec()),
                SourceQueryValue::Bytes(v) => ("bytes", v.clone()),
            };
            owned.push(kind.as_bytes().to_vec());
            owned.push(value);
        }
        let fields: Vec<&[u8]> = owned.iter().map(Vec::as_slice).collect();
        digest_of(b"eg/source-query-parameters/v1", &fields)
    }

    /// MEDULLA-FEDERATED-QUERY-R085.2: the framed digest of every field but the idempotency
    /// key, so one operation has one digest.
    pub fn operation_digest(&self) -> Result<String, SourceQueryError> {
        let parameter_digest = self.parameter_digest()?;
        let version = self.schema_version.to_be_bytes();
        let limits = [
            self.limits.max_rows,
            self.limits.max_bytes,
            self.limits.chunk_rows,
            self.limits.chunk_bytes,
            self.limits.queue_depth,
            self.limits.queue_bytes,
            self.limits.deadline_ms,
            self.limits.max_resident_bytes,
            self.requested_expires_at_ms,
        ]
        .map(u64::to_be_bytes);
        let mut fields: Vec<&[u8]> = vec![
            &version[..],
            self.source_ref.as_bytes(),
            self.source_revision.as_bytes(),
            self.schema_digest.as_bytes(),
            self.snapshot_ref.as_bytes(),
            self.dialect.name().as_bytes(),
            self.sql.as_bytes(),
            parameter_digest.as_bytes(),
        ];
        fields.extend(limits.iter().map(|bytes| bytes.as_slice()));
        digest_of(b"eg/source-query-operation/v1", &fields)
    }
}

impl ApprovedSourceQuery {
    /// MEDULLA-FEDERATED-QUERY-R085.2: the framed digest that binds every field but
    /// `query_digest`; the request is bound through `operation_digest`.
    pub fn compute_query_digest(&self) -> Result<String, SourceQueryError> {
        let version = self.schema_version.to_be_bytes();
        let times = [self.issued_at_ms, self.expires_at_ms, self.hard_expires_at_ms]
            .map(u64::to_be_bytes);
        let mut fields: Vec<&[u8]> = vec![
            &version[..],
            self.approval_id.as_bytes(),
            self.query_id.as_bytes(),
            self.operation_digest.as_bytes(),
            self.parameter_digest.as_bytes(),
            self.tenant_ref.as_bytes(),
            self.owner_ref.as_bytes(),
            self.actor_ref.as_bytes(),
            self.authority_digest.as_bytes(),
            self.policy_digest.as_bytes(),
            self.identity_digest.as_bytes(),
        ];
        fields.extend(self.allowed_operations.iter().map(|op| op.name().as_bytes()));
        fields.extend(times.iter().map(|bytes| bytes.as_slice()));
        digest_of(b"eg/source-query-approval/v1", &fields)
    }
}

/// The execution boundary a verification happens at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SourceQueryBoundary {
    Start,
    Chunk,
    Complete,
}

/// A request to verify an approval against observed source identities.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SourceQueryApprovalCheck {
    pub approved_query: ApprovedSourceQuery,
    pub observed_source_revision: String,
    pub observed_schema_digest: String,
    pub observed_snapshot_ref: String,
    /// Served as `phase`: the text form of a check is unchanged.
    #[serde(rename = "phase")]
    pub boundary: SourceQueryBoundary,
}

/// A bounded validation result; it authorizes nothing past `valid_until_ms`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SourceQueryApprovalValidity {
    pub approval_id: String,
    pub query_id: String,
    pub query_digest: String,
    pub lease_revision: u64,
    pub checked_at_ms: u64,
    pub valid_until_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-FEDERATED-QUERY-R738.14
    #[test]
    fn source_query_boundary_is_a_position_not_a_lifecycle() {
        // A check names the boundary it verifies at; the check is one-shot and nothing moves
        // the boundary afterwards. The served text form of the field is unchanged.
        for (boundary, text) in [
            (SourceQueryBoundary::Start, "start"),
            (SourceQueryBoundary::Chunk, "chunk"),
            (SourceQueryBoundary::Complete, "complete"),
        ] {
            assert_eq!(serde_json::to_value(boundary).unwrap(), text);
        }
        let source = include_str!("source_query.rs");
        let production = source.split("#[cfg(test)]").next().unwrap();
        assert!(!production.contains(concat!("SourceQuery", "Phase")));
        assert!(production
            .contains("#[serde(rename = \"phase\")]\n    pub boundary: SourceQueryBoundary,"));
    }

    fn limits() -> SourceQueryLimits {
        SourceQueryLimits {
            max_rows: 10_000,
            max_bytes: 1 << 24,
            chunk_rows: 500,
            chunk_bytes: 1 << 20,
            queue_depth: 4,
            queue_bytes: 1 << 22,
            deadline_ms: 30_000,
            max_resident_bytes: 1 << 26,
        }
    }

    fn request() -> SourceQueryRequest {
        SourceQueryRequest {
            schema_version: 1,
            source_ref: "src:orders".into(),
            source_revision: "rev-7".into(),
            schema_digest: "sha256:abc".into(),
            snapshot_ref: "snap-42".into(),
            dialect: SourceQueryDialect::Postgres,
            sql: "SELECT id FROM orders WHERE region = $1 AND total > $2 AND note <> '$9'".into(),
            parameters: vec![
                SourceQueryParameter {
                    position: 1,
                    value: SourceQueryValue::Text("eu".into()),
                },
                SourceQueryParameter {
                    position: 2,
                    value: SourceQueryValue::Float64(10.5),
                },
            ],
            limits: limits(),
            requested_expires_at_ms: 1_000,
            idempotency_key: "idem-1".into(),
        }
    }

    fn approved() -> ApprovedSourceQuery {
        ApprovedSourceQuery {
            schema_version: 1,
            approval_id: "appr-1".into(),
            query_id: "q-1".into(),
            request: request(),
            tenant_ref: "t1".into(),
            owner_ref: "o1".into(),
            actor_ref: "a1".into(),
            authority_digest: "d-auth".into(),
            policy_digest: "d-pol".into(),
            identity_digest: "d-id".into(),
            allowed_operations: vec![SourceQueryOperation::ReadSql],
            issued_at_ms: 100,
            expires_at_ms: 900,
            hard_expires_at_ms: 1_000,
            operation_digest: "d-op".into(),
            parameter_digest: "d-par".into(),
            query_digest: "d-q".into(),
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R085.1
    #[test]
    fn approval_records_round_trip() {
        let approval = approved();
        approval.validate().unwrap();
        let json = serde_json::to_string(&approval).unwrap();
        assert_eq!(
            serde_json::from_str::<ApprovedSourceQuery>(&json).unwrap(),
            approval
        );
        let check = SourceQueryApprovalCheck {
            approved_query: approval.clone(),
            observed_source_revision: "rev-7".into(),
            observed_schema_digest: "sha256:abc".into(),
            observed_snapshot_ref: "snap-42".into(),
            boundary: SourceQueryBoundary::Chunk,
        };
        let json = serde_json::to_string(&check).unwrap();
        assert_eq!(
            serde_json::from_str::<SourceQueryApprovalCheck>(&json).unwrap(),
            check
        );
        let validity = SourceQueryApprovalValidity {
            approval_id: "appr-1".into(),
            query_id: "q-1".into(),
            query_digest: "d-q".into(),
            lease_revision: 3,
            checked_at_ms: 200,
            valid_until_ms: 300,
        };
        let json = serde_json::to_string(&validity).unwrap();
        assert_eq!(
            serde_json::from_str::<SourceQueryApprovalValidity>(&json).unwrap(),
            validity
        );
        for value in [
            SourceQueryValue::Null,
            SourceQueryValue::Bool(true),
            SourceQueryValue::Int64(i64::MIN),
            SourceQueryValue::Bytes(vec![0, 255]),
        ] {
            let p = SourceQueryParameter { position: 1, value };
            let json = serde_json::to_string(&p).unwrap();
            assert_eq!(
                serde_json::from_str::<SourceQueryParameter>(&json).unwrap(),
                p
            );
        }

        // Unknown fields, unrepresentable values and unknown kinds refuse to decode.
        let mut extra = serde_json::to_value(request()).unwrap();
        extra["credentials"] = "secret".into();
        assert!(serde_json::from_value::<SourceQueryRequest>(extra).is_err());
        assert!(serde_json::from_str::<SourceQueryParameter>(
            r#"{"position":1,"value":{"kind":"int64","value":9223372036854775808}}"#
        )
        .is_err());
        assert!(serde_json::from_str::<SourceQueryParameter>(
            r#"{"position":1,"value":{"kind":"decimal","value":"1.0"}}"#
        )
        .is_err());

        // Invalid references, parameters, limits and approvals refuse.
        let mut no_snapshot = request();
        no_snapshot.snapshot_ref = " ".into();
        assert_eq!(no_snapshot.validate(), Err(SourceQueryError::InvalidQuery));
        let mut nan = request();
        nan.parameters[1].value = SourceQueryValue::Float64(f64::NAN);
        assert_eq!(nan.validate(), Err(SourceQueryError::InvalidQuery));
        let mut missing = request();
        missing.parameters.pop();
        assert_eq!(missing.validate(), Err(SourceQueryError::InvalidQuery));
        let mut misplaced = request();
        misplaced.parameters[1].position = 3;
        assert_eq!(misplaced.validate(), Err(SourceQueryError::InvalidQuery));
        let mut zero = request();
        zero.limits.queue_depth = 0;
        assert_eq!(zero.validate(), Err(SourceQueryError::LimitExceeded));
        let mut unbounded = request();
        unbounded.limits.max_rows = u64::MAX;
        assert_eq!(unbounded.validate(), Err(SourceQueryError::LimitExceeded));
        let mut write = approved();
        write.allowed_operations.push(SourceQueryOperation::ReadSql);
        assert_eq!(write.validate(), Err(SourceQueryError::InvalidQuery));
        let mut expired = approved();
        expired.hard_expires_at_ms = 800;
        assert_eq!(expired.validate(), Err(SourceQueryError::InvalidQuery));
        let mut mysql = request();
        mysql.dialect = SourceQueryDialect::Mysql;
        mysql.sql = "SELECT id FROM orders WHERE region = ? AND total > ?".into();
        mysql.validate().unwrap();
    }

    // spec: MEDULLA-FEDERATED-QUERY-R085.7
    #[test]
    fn errors_redact_literals_and_round_trip() {
        const KINDS: [(SourceQueryError, &str); 10] = [
            (SourceQueryError::UnsupportedDialect, "unsupported_dialect"),
            (SourceQueryError::InvalidQuery, "invalid_query"),
            (SourceQueryError::Unauthorized, "unauthorized"),
            (SourceQueryError::ApprovalExpired, "approval_expired"),
            (SourceQueryError::ApprovalRevoked, "approval_revoked"),
            (SourceQueryError::StaleSource, "stale_source"),
            (SourceQueryError::QueryFailed, "query_failed"),
            (SourceQueryError::LimitExceeded, "limit_exceeded"),
            (SourceQueryError::Cancelled, "cancelled"),
            (
                SourceQueryError::VerificationUnavailable,
                "verification_unavailable",
            ),
        ];
        let identity = "ab".repeat(32);
        for (kind, token) in KINDS {
            // The closed token is the wire form and the message.
            assert_eq!(
                serde_json::to_string(&kind).expect("encodes"),
                format!("\"{token}\"")
            );
            assert_eq!(kind.to_string(), token);
            let failure =
                SourceQueryFailure::new(kind, &identity, SourceQueryStep::Execution, Some(3))
                    .expect("a digest identity is accepted");
            let bytes = rmp_serde::to_vec_named(&failure).expect("encodes");
            let decoded: SourceQueryFailure = rmp_serde::from_slice(&bytes).expect("decodes");
            assert_eq!(decoded, failure);
            assert!(decoded.validate().is_ok());
            // No formatted form carries anything but the closed tokens, the digest and numbers.
            let shown = failure.to_string();
            assert!(shown.contains(token) && shown.contains(&identity));
        }
        // A failure before any chunk has no last chunk.
        let early = SourceQueryFailure::new(
            SourceQueryError::Cancelled,
            &identity,
            SourceQueryStep::Delivery,
            None,
        )
        .expect("identity ok");
        assert_eq!(early.last_chunk, None);

        // A literal, a password or SQL text cannot ride in the identity.
        for literal in [
            "SELECT * FROM t WHERE ssn = '123-45-6789'",
            "password=hunter2",
            "",
            &"AB".repeat(32),
            &"ab".repeat(31),
        ] {
            assert_eq!(
                SourceQueryFailure::new(
                    SourceQueryError::QueryFailed,
                    literal,
                    SourceQueryStep::Execution,
                    None
                ),
                Err(SourceQueryError::InvalidQuery),
                "{literal}"
            );
        }
        // Unknown kinds and unknown fields refuse.
        assert!(serde_json::from_str::<SourceQueryError>("\"boom\"").is_err());
        let extra = serde_json::json!({
            "kind": "query_failed", "query_identity": identity, "failed_step": "execution",
            "last_chunk": null, "sql": "SELECT 1"
        });
        assert!(serde_json::from_value::<SourceQueryFailure>(extra).is_err());
    }

    // spec: MEDULLA-FEDERATED-QUERY-R739.2
    #[test]
    fn source_query_failure_step_is_not_a_lifecycle_field() {
        let identity = "cd".repeat(32);
        let failure = SourceQueryFailure::new(
            SourceQueryError::StaleSource,
            &identity,
            SourceQueryStep::Approval,
            Some(1),
        )
        .expect("a digest identity is accepted");
        // The position is the `failed_step` key; the step words are unchanged.
        let wire = serde_json::to_value(&failure).expect("encodes");
        assert_eq!(wire["failed_step"], "approval");
        assert!(wire.get("stage").is_none());
        for (step, word) in [
            (SourceQueryStep::Validation, "validation"),
            (SourceQueryStep::Approval, "approval"),
            (SourceQueryStep::Verification, "verification"),
            (SourceQueryStep::Execution, "execution"),
            (SourceQueryStep::Delivery, "delivery"),
        ] {
            assert_eq!(serde_json::to_value(step).expect("encodes"), word);
        }
        // The old key is refused: there is no alias.
        let old = serde_json::json!({
            "kind": "stale_source", "query_identity": identity, "stage": "approval",
            "last_chunk": 1
        });
        assert!(serde_json::from_value::<SourceQueryFailure>(old).is_err());
        assert!(failure.to_string().contains("failed step Approval"));
    }
}
