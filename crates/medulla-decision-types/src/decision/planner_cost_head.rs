//! MEDULLA-FEDERATED-QUERY-R732.5: the `PlannerCostHead` component body.
//!
//! One revision of the planner's learned cost head is a library component of kind
//! `AgentComponentKind::PlannerCostHead`, published through the same component-revision
//! store the policy-evolution gate reads. The component's `planner.cost_head` attribute
//! carries the revision as JSON and its `content_digest` is the revision's own digest, so a
//! pinned revision is exactly the body that was evaluated.

use std::collections::BTreeMap;

use medulla_types::contract::Digest256;
use serde::Deserialize;

use super::DecisionErrorCode;

/// The component attribute a published `PlannerCostHead` carries its revision in: the
/// canonical JSON of the cost head (digest, correction curves, planner knobs).
pub const PLANNER_COST_HEAD_ATTRIBUTE: &str = "planner.cost_head";

/// The component id every planner cost-head revision is published under, and the
/// `KnobSet` policy id the policy-evolution gate and the champion pointer name.
pub const PLANNER_POLICY_ID: &str = "planner";

/// Digest of a cost head: the correction curves `(op kind, coefficients)` in op-kind
/// order, then the planner knobs in name order (MEDULLA-FEDERATED-QUERY-R732.2).
pub fn cost_head_digest(curves: &[(&str, &[f64])], knobs: &[(&str, &str)]) -> String {
    let mut curves: Vec<(&str, &[f64])> = curves.to_vec();
    curves.sort_by(|a, b| a.0.cmp(b.0));
    let mut knobs: Vec<(&str, &str)> = knobs.to_vec();
    knobs.sort();
    // A tuple of strings and floats always encodes, and the framing refuses only an
    // empty domain or a field past 4 GiB.
    #[allow(clippy::disallowed_methods)]
    let bytes = rmp_serde::to_vec(&(curves, knobs)).expect("curves and knobs always encode");
    #[allow(clippy::disallowed_methods)]
    let digest = Digest256::framed(b"eg/cost-head/v1", &[&bytes])
        .expect("the cost-head domain is non-empty");
    digest.to_hex()
}

/// The fields of a cost head the verifier reads; the rest of the body is the planner's.
#[derive(Deserialize)]
struct CostHeadView {
    digest: String,
    corrections: Vec<CorrectionView>,
    knobs: Vec<(String, String)>,
}

#[derive(Deserialize)]
struct CorrectionView {
    op_kind: String,
    coeffs: Vec<f64>,
}

/// A `PlannerCostHead` component's body verifies when it decodes and its digest, recomputed
/// from the curves and knobs it carries, is both the digest it names and the component's
/// `content_digest`.
pub fn verify_cost_head(
    attributes: &BTreeMap<String, String>,
    content_digest: &str,
) -> Result<(), DecisionErrorCode> {
    let body = attributes
        .get(PLANNER_COST_HEAD_ATTRIBUTE)
        .ok_or(DecisionErrorCode::PolicyBodyUnavailable)?;
    let view: CostHeadView =
        serde_json::from_str(body).map_err(|_| DecisionErrorCode::PolicyBodyUnavailable)?;
    let curves: Vec<(&str, &[f64])> = view
        .corrections
        .iter()
        .map(|c| (c.op_kind.as_str(), c.coeffs.as_slice()))
        .collect();
    let knobs: Vec<(&str, &str)> = view
        .knobs
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let recomputed = cost_head_digest(&curves, &knobs);
    if recomputed != view.digest || recomputed != content_digest {
        return Err(DecisionErrorCode::PolicyBodyUnavailable);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attributes(digest: &str, coeff: f64) -> BTreeMap<String, String> {
        let body = serde_json::json!({
            "digest": digest,
            "corrections": [{ "op_kind": "Scan", "coeffs": [coeff, 0.0] }],
            "knobs": [["join_cost", "2"]],
        });
        BTreeMap::from([(PLANNER_COST_HEAD_ATTRIBUTE.to_string(), body.to_string())])
    }

    // spec: MEDULLA-FEDERATED-QUERY-R732.5
    #[test]
    fn a_cost_head_body_verifies_only_against_its_own_digest() {
        let digest = cost_head_digest(&[("Scan", &[0.5, 0.0])], &[("join_cost", "2")]);
        assert_eq!(verify_cost_head(&attributes(&digest, 0.5), &digest), Ok(()));
        // A body whose curves moved no longer matches the digest it names.
        assert!(verify_cost_head(&attributes(&digest, 0.6), &digest).is_err());
        // A digest that is not the component's content digest is refused.
        assert!(verify_cost_head(&attributes(&digest, 0.5), "other").is_err());
        assert!(verify_cost_head(&BTreeMap::new(), &digest).is_err());
    }
}
