//! MEDULLA-DURABLE-KERNEL-R024.8: the Spark job backend's `ForeignSourceSpec`
//! source.
//!
//! [`medulla_types::wire::ForeignSourceSpec::SparkBatch`] already names a sealed,
//! completed batch artifact (interactive Spark SQL pushdown is deliberately
//! absent; a query may read only a registered artifact, never launch a job
//! or trust a caller-provided path). `federation::source_for`'s generic
//! dispatch already refuses it through `Oq2Unbound`; this module is the
//! EXPLICIT per-registration half -- the same shape `federation_age` and
//! `federation_trino` give their own backend -- validating the artifact
//! reference's shape before a registration is attempted, then refusing with
//! a named, typed reason because no bound Spark artifact-store driver exists
//! yet in this workspace. Never silently accepted as if a driver existed.

use medulla_types::wire::ForeignSourceSpec;

use crate::federation::ForeignSourceRegistry;

const MAX_FIELD_BYTES: usize = 1024;
/// `SparkBatch::artifact_ref` must reference a REGISTERED artifact, never a
/// caller-provided path -- enforced here by requiring the `artifact://`
/// scheme the registration surface issues, matching the wire doc's "a
/// completed, registered artifact, never ... a caller-provided path".
const ARTIFACT_REF_SCHEME: &str = "artifact://";

/// Validate a `SparkBatch` spec's shape, then refuse: no bound Spark
/// artifact-store driver exists yet. Never registers anything.
pub fn register_spark(
    _registry: &mut ForeignSourceRegistry,
    _name: impl Into<String>,
    spec: &ForeignSourceSpec,
) -> Result<(), String> {
    validate_spark_batch_shape(spec)?;
    Err(
        "federation: Spark job backend has no bound artifact-store driver yet \
         (MEDULLA-DURABLE-KERNEL-R024.8); SparkBatch reads only a registered artifact"
            .into(),
    )
}

fn validate_spark_batch_shape(spec: &ForeignSourceSpec) -> Result<(), String> {
    let ForeignSourceSpec::SparkBatch {
        artifact_ref,
        id_field,
        score_field,
    } = spec
    else {
        return Err("federation: expected a Spark batch source".into());
    };
    if artifact_ref.len() > MAX_FIELD_BYTES || !artifact_ref.starts_with(ARTIFACT_REF_SCHEME) {
        return Err(format!(
            "federation: invalid Spark artifact reference (must start with {ARTIFACT_REF_SCHEME:?})"
        ));
    }
    if artifact_ref.len() == ARTIFACT_REF_SCHEME.len() {
        return Err("federation: invalid Spark artifact reference (empty artifact id)".into());
    }
    identifier(id_field, "id field")?;
    if let Some(score) = score_field {
        identifier(score, "score field")?;
        if score == id_field {
            return Err("federation: Spark score and id fields must differ".into());
        }
    }
    Ok(())
}

fn identifier(value: &str, field: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > MAX_FIELD_BYTES
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err(format!("federation: invalid Spark {field}"));
    }
    Ok(())
}

/// MEDULLA-FEDERATED-QUERY-R053.700: the idempotent Spark submission id --
/// the SHA-256 digest (64 lowercase hex characters) of the preset id, the input
/// digests and the parameters. Identical inputs always derive the same id, so a
/// resubmission names the run it already started. This is the only id function.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SubmissionId(String);

impl SubmissionId {
    /// Derive the id. Every part is length-prefixed so no two distinct inputs
    /// share an encoding; parameters hash in key order.
    pub fn derive(
        preset_id: &str,
        input_digests: &[&str],
        parameters: &std::collections::BTreeMap<String, String>,
    ) -> Self {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        let mut part = |bytes: &[u8]| {
            hasher.update((bytes.len() as u64).to_be_bytes());
            hasher.update(bytes);
        };
        part(b"medulla.spark.submission.v1");
        part(preset_id.as_bytes());
        part(&(input_digests.len() as u64).to_be_bytes());
        for digest in input_digests {
            part(digest.as_bytes());
        }
        part(&(parameters.len() as u64).to_be_bytes());
        for (key, value) in parameters {
            part(key.as_bytes());
            part(value.as_bytes());
        }
        Self(hex::encode(hasher.finalize()))
    }

    /// The 64 lowercase hex characters.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The change-envelope fence fields a Spark or Trino result carries.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SubmissionFence {
    pub source_id: Option<String>,
    pub snapshot_id: Option<String>,
    pub submission_id: Option<String>,
    pub generation: Option<u64>,
    pub producer_revision: Option<String>,
}

/// A typed refusal of the Spark submission path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubmissionRefusal {
    /// A fence field is absent or empty; carries the field name.
    FenceFieldMissing(&'static str),
    /// The preset is not one the source offers.
    PresetUnavailable,
}

impl SubmissionRefusal {
    /// The registered `ServerErrorCode` wire token.
    pub const fn code(&self) -> medulla_types::contract::ServerErrorCode {
        use medulla_types::contract::ServerErrorCode;
        match self {
            Self::FenceFieldMissing(_) => ServerErrorCode::FenceFieldMissing,
            Self::PresetUnavailable => ServerErrorCode::PresetUnavailable,
        }
    }
}

impl SubmissionFence {
    /// Refuse the first absent or empty fence field, in envelope order.
    pub fn check(&self) -> Result<(), SubmissionRefusal> {
        let present = |value: &Option<String>| value.as_deref().is_some_and(|v| !v.is_empty());
        for (name, ok) in [
            ("source_id", present(&self.source_id)),
            ("snapshot_id", present(&self.snapshot_id)),
            ("submission_id", present(&self.submission_id)),
            ("generation", self.generation.is_some()),
            ("producer_revision", present(&self.producer_revision)),
        ] {
            if !ok {
                return Err(SubmissionRefusal::FenceFieldMissing(name));
            }
        }
        Ok(())
    }
}

/// Resolve a preset among those the source offers, then derive its submission id.
pub fn derive_submission(
    available_presets: &[&str],
    preset_id: &str,
    input_digests: &[&str],
    parameters: &std::collections::BTreeMap<String, String>,
) -> Result<SubmissionId, SubmissionRefusal> {
    if !available_presets.contains(&preset_id) {
        return Err(SubmissionRefusal::PresetUnavailable);
    }
    Ok(SubmissionId::derive(preset_id, input_digests, parameters))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_spec() -> ForeignSourceSpec {
        ForeignSourceSpec::SparkBatch {
            artifact_ref: "artifact://batches/2026-10-09/ingest-run".into(),
            id_field: "id".into(),
            score_field: None,
        }
    }

    // spec: MEDULLA-DURABLE-KERNEL-R024.8
    #[test]
    fn well_formed_spec_is_refused_not_silently_accepted() {
        let mut registry = ForeignSourceRegistry::default();
        let err = register_spark(&mut registry, "spark", &valid_spec()).unwrap_err();
        assert!(err.contains("no bound artifact-store driver"), "{err}");
    }

    // spec: MEDULLA-DURABLE-KERNEL-R024.8
    #[test]
    fn caller_provided_path_without_the_artifact_scheme_is_rejected() {
        let mut registry = ForeignSourceRegistry::default();
        let mut spec = valid_spec();
        if let ForeignSourceSpec::SparkBatch { artifact_ref, .. } = &mut spec {
            *artifact_ref = "/var/spark/output/part-00000".into();
        }
        let err = register_spark(&mut registry, "spark", &spec).unwrap_err();
        assert!(err.contains("must start with"), "{err}");
    }

    // spec: MEDULLA-DURABLE-KERNEL-R024.8
    #[test]
    fn empty_artifact_id_after_the_scheme_is_rejected() {
        let mut registry = ForeignSourceRegistry::default();
        let mut spec = valid_spec();
        if let ForeignSourceSpec::SparkBatch { artifact_ref, .. } = &mut spec {
            *artifact_ref = "artifact://".into();
        }
        let err = register_spark(&mut registry, "spark", &spec).unwrap_err();
        assert!(err.contains("empty artifact id"), "{err}");
    }

    // spec: MEDULLA-DURABLE-KERNEL-R024.8
    #[test]
    fn score_field_matching_id_field_is_rejected() {
        let mut registry = ForeignSourceRegistry::default();
        let mut spec = valid_spec();
        if let ForeignSourceSpec::SparkBatch {
            id_field,
            score_field,
            ..
        } = &mut spec
        {
            *score_field = Some(id_field.clone());
        }
        let err = register_spark(&mut registry, "spark", &spec).unwrap_err();
        assert!(err.contains("must differ"), "{err}");
    }

    // spec: MEDULLA-DURABLE-KERNEL-R024.8
    #[test]
    fn wrong_spec_variant_is_rejected() {
        let mut registry = ForeignSourceRegistry::default();
        let spec = ForeignSourceSpec::Named {
            name: "other".into(),
        };
        let err = register_spark(&mut registry, "spark", &spec).unwrap_err();
        assert!(err.contains("expected a Spark batch source"), "{err}");
    }

    /// Verifies MEDULLA-FEDERATED-QUERY-R053.700: `Trino` and `Spark` sources with idempotent submission: model and behavior
    // spec: MEDULLA-FEDERATED-QUERY-R053.700
    #[test]
    fn submission_id_idempotent_and_fenced() {
        let mut params = std::collections::BTreeMap::new();
        params.insert("window".to_string(), "7d".to_string());
        let first = derive_submission(&["daily"], "daily", &["sha256:aa"], &params).unwrap();
        let again = derive_submission(&["daily"], "daily", &["sha256:aa"], &params).unwrap();
        assert_eq!(first, again, "resubmission names the same run");
        assert_eq!(first.as_str().len(), 64);
        assert!(first
            .as_str()
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        let other = SubmissionId::derive("daily", &["sha256:ab"], &params);
        assert_ne!(first, other);
        assert_ne!(
            SubmissionId::derive("ab", &["c"], &Default::default()),
            SubmissionId::derive("a", &["bc"], &Default::default())
        );

        let refusal = derive_submission(&["daily"], "hourly", &[], &params).unwrap_err();
        assert_eq!(refusal.code().as_str(), "PRESET_UNAVAILABLE");

        let mut fence = SubmissionFence {
            source_id: Some("spark".into()),
            snapshot_id: Some("s1".into()),
            submission_id: Some(first.as_str().to_string()),
            generation: Some(1),
            producer_revision: Some("r1".into()),
        };
        assert_eq!(fence.check(), Ok(()));
        fence.snapshot_id = Some(String::new());
        let refusal = fence.check().unwrap_err();
        assert_eq!(refusal, SubmissionRefusal::FenceFieldMissing("snapshot_id"));
        assert_eq!(refusal.code().as_str(), "FENCE_FIELD_MISSING");
    }
}
