//! Importance resistance in the deferred re-embed worker.
//!
//! `MEDULLA-REPO-INGEST-R006.2` — the re-embed candidate is blended toward
//! the active embedding, and the coordinates the consolidation job scored as
//! important move less than the rest.  For every coordinate
//! `z_i = (p_i + lambda * w_i * o_i) / (1 + lambda * w_i)`, where `p` is the
//! proposed embedding, `o` the active anchor and `w` the importance vector
//! the consolidation job stores with each embedding version.  `lambda` is the
//! `embedding.importance_lambda` knob (default 1.0, range `(0, 100]`, unit
//! none).  A high importance weight pulls the coordinate back to the anchor;
//! a zero weight adopts the proposed value outright.
//!
//! All three vectors share one `EmbeddingVersionStore` model id and
//! dimension; the re-embed worker guarantees that, and this function
//! re-checks the dimensions it can see.  A refusal keeps the active version,
//! and the candidate stays shadow data until `evaluate_promotion` of
//! `MEDULLA-UNIFIED-DATA-PLANE-R040.2` passes.  Query routing never calls
//! this path.

/// Stable refusal codes for the re-embed importance path.
///
/// A refusal keeps the active embedding version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegularizeError {
    /// The consolidation job produced no importance vector for this embedding
    /// version.
    ImportanceEvidenceMissing,
    /// The proposed, anchor and importance vectors do not share one dimension
    /// (one `EmbeddingVersionStore` model id).
    ImportanceSpaceMismatch,
    /// A coordinate of any vector is not finite, a weight is negative, or
    /// `lambda` is not a finite positive value.
    EmbeddingNonFinite,
    /// The anchor embedding was already superseded by a newer re-embed.  Only
    /// the re-embed worker can observe this; the pure function never yields
    /// it.
    ReembedSuperseded,
}

impl RegularizeError {
    /// Stable refusal code as carried on the wire.
    pub const fn code(self) -> &'static str {
        match self {
            Self::ImportanceEvidenceMissing => "IMPORTANCE_EVIDENCE_MISSING",
            Self::ImportanceSpaceMismatch => "IMPORTANCE_SPACE_MISMATCH",
            Self::EmbeddingNonFinite => "EMBEDDING_NONFINITE",
            Self::ReembedSuperseded => "REEMBED_SUPERSEDED",
        }
    }
}

impl core::fmt::Display for RegularizeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for RegularizeError {}

/// Regularize a re-embed candidate against the active anchor.
///
/// `proposed` is the candidate embedding, `old` the currently active
/// embedding (the anchor; `None` when no version is active yet) and `weights`
/// the importance vector stored with the embedding version by the
/// consolidation job (`None` refuses with
/// [`RegularizeError::ImportanceEvidenceMissing`]).
///
/// For every coordinate
/// `z_i = (p_i + lambda * w_i * o_i) / (1 + lambda * w_i)`.  When `old` is
/// `None` the result is the validated `proposed` vector itself.  Weights are
/// finite and non-negative; `lambda` is finite and positive — the knob
/// `embedding.importance_lambda` bounds it to `(0, 100]` at configuration
/// time.
pub fn regularize(
    proposed: &[f32],
    old: Option<&[f32]>,
    weights: Option<&[f32]>,
    lambda: f32,
) -> Result<Vec<f32>, RegularizeError> {
    let weights = weights.ok_or(RegularizeError::ImportanceEvidenceMissing)?;
    if !lambda.is_finite() || lambda <= 0.0 {
        return Err(RegularizeError::EmbeddingNonFinite);
    }
    if proposed.iter().any(|p| !p.is_finite()) || weights.iter().any(|w| !w.is_finite() || *w < 0.0)
    {
        return Err(RegularizeError::EmbeddingNonFinite);
    }
    let Some(old) = old else {
        if weights.len() != proposed.len() {
            return Err(RegularizeError::ImportanceSpaceMismatch);
        }
        return Ok(proposed.to_vec());
    };
    if old.iter().any(|o| !o.is_finite()) {
        return Err(RegularizeError::EmbeddingNonFinite);
    }
    if old.len() != proposed.len() || weights.len() != proposed.len() {
        return Err(RegularizeError::ImportanceSpaceMismatch);
    }
    let mut out = Vec::with_capacity(proposed.len());
    for i in 0..proposed.len() {
        let p = proposed[i];
        let o = old[i];
        let w = weights[i];
        let denominator = 1.0 + lambda * w;
        out.push((p + lambda * w * o) / denominator);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-REPO-INGEST-R006.2
    #[test]
    fn squared_l2_anchor_matches_closed_form() {
        let proposed = [0.0f32, 1.0, -2.0, 0.5, 3.0, -0.25, 1.5, 0.125];
        let old = [0.0f32, -1.0, 2.0, 0.25, -3.0, 0.75, -1.5, 0.0625];
        let weights = [0.0f32, 0.1, 1.0, 4.0, 0.01, 10.0, 0.5, 2.0];
        let lambda = 2.5;

        let z = regularize(&proposed, Some(&old), Some(&weights), lambda).unwrap();

        // Closed form: z_i - o_i = (p_i - o_i) / (1 + lambda * w_i).
        let mut closed = 0.0f32;
        for i in 0..proposed.len() {
            let step = (proposed[i] - old[i]) / (1.0 + lambda * weights[i]);
            closed += step * step;
        }
        let mut actual = 0.0f32;
        for i in 0..z.len() {
            let d = z[i] - old[i];
            actual += d * d;
        }
        assert!(
            (actual - closed).abs() <= 1e-4 * closed.max(actual).max(1.0),
            "squared L2 from the anchor {actual} diverges from the closed form {closed}"
        );
    }

    #[test]
    fn per_coordinate_blend_matches_the_specified_formula() {
        let proposed = [0.0f32, 1.0];
        let old = [2.0f32, -1.0];
        let weights = [0.5f32, 2.0];
        let lambda = 0.5;

        let z = regularize(&proposed, Some(&old), Some(&weights), lambda).unwrap();

        let expected: Vec<f32> = proposed
            .iter()
            .zip(old.iter())
            .zip(weights.iter())
            .map(|((&p, &o), &w)| (p + lambda * w * o) / (1.0 + lambda * w))
            .collect();
        for (z, e) in z.iter().zip(expected.iter()) {
            assert!((z - e).abs() <= 1e-6, "coordinate {z} diverges from {e}");
        }
        // z_1 = (1 - 1) / 2 = 0 exactly; z_0 = 0.5 / 1.25 = 0.4.
        assert!((z[1] - 0.0).abs() <= 1e-7);
        assert!((z[0] - 0.4).abs() <= 1e-6);
    }

    #[test]
    fn high_importance_coordinates_move_less() {
        let proposed = [1.0f32; 3];
        let old = [0.0f32; 3];
        let weights = [0.0f32, 0.5, 5.0];

        let z = regularize(&proposed, Some(&old), Some(&weights), 1.0).unwrap();

        // |z_i - o_i| = 1 / (1 + w_i): strictly decreasing in the weight.
        assert_eq!(z[0], 1.0, "zero weight adopts the proposed value");
        assert!(
            z[2] < z[1] && z[1] < z[0],
            "displacement must shrink with importance: {z:?}"
        );
    }

    #[test]
    fn unit_weight_with_unit_lambda_is_the_midpoint() {
        let z = regularize(&[1.0f32, -2.0], Some(&[3.0, 0.0]), Some(&[1.0, 1.0]), 1.0).unwrap();
        assert!((z[0] - 2.0).abs() <= 1e-7);
        assert!((z[1] - -1.0).abs() <= 1e-7);
    }

    #[test]
    fn missing_importance_evidence_is_refused_and_keeps_no_candidate() {
        let proposed = [0.0f32, 1.0];
        let err = regularize(&proposed, Some(&[1.0, 2.0]), None, 1.0).unwrap_err();
        assert_eq!(err, RegularizeError::ImportanceEvidenceMissing);
        assert_eq!(err.code(), "IMPORTANCE_EVIDENCE_MISSING");
    }

    #[test]
    fn dimension_mismatch_is_refused_with_the_stable_code() {
        let err = regularize(&[0.0, 1.0], Some(&[1.0]), Some(&[0.5, 0.5]), 1.0).unwrap_err();
        assert_eq!(err, RegularizeError::ImportanceSpaceMismatch);
        assert_eq!(err.code(), "IMPORTANCE_SPACE_MISMATCH");

        let err = regularize(&[0.0, 1.0], Some(&[1.0, 2.0]), Some(&[0.5]), 1.0).unwrap_err();
        assert_eq!(err, RegularizeError::ImportanceSpaceMismatch);

        let err = regularize(&[0.0, 1.0], None, Some(&[0.5]), 1.0).unwrap_err();
        assert_eq!(err, RegularizeError::ImportanceSpaceMismatch);
    }

    #[test]
    fn non_finite_or_out_of_domain_input_is_refused() {
        let ok = [0.0f32, 1.0];
        for input in [
            regularize(&[f32::NAN, 1.0], Some(&ok), Some(&[0.5, 0.5]), 1.0),
            regularize(&ok, Some(&[1.0, f32::INFINITY]), Some(&[0.5, 0.5]), 1.0),
            regularize(&ok, Some(&ok), Some(&[0.5, -0.5]), 1.0),
            regularize(&ok, Some(&ok), Some(&[0.5, f32::NAN]), 1.0),
            regularize(&ok, Some(&ok), Some(&[0.5, 0.5]), 0.0),
            regularize(&ok, Some(&ok), Some(&[0.5, 0.5]), f32::NEG_INFINITY),
        ] {
            let err = input.unwrap_err();
            assert_eq!(err, RegularizeError::EmbeddingNonFinite);
            assert_eq!(err.code(), "EMBEDDING_NONFINITE");
        }
    }

    #[test]
    fn absent_anchor_returns_the_proposed_vector() {
        let z = regularize(&[1.0f32, -0.5], None, Some(&[0.5, 2.0]), 1.0).unwrap();
        assert_eq!(z, vec![1.0f32, -0.5]);
    }
}
