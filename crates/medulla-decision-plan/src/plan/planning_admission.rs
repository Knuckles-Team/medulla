//! Planning admission: a plan is admitted only when the architecture view
//! already wires every feature it names (MEDULLA-DECISION-ENGINE-R171.2).
//!
//! The comparison is a set difference. The plan names, per feature, the
//! container it lives in and the file it attaches to; the view lists the
//! containers and the source paths that support each. The view is derived by
//! `medulla_core::architecture_view::derive_architecture_view`; this module
//! derives nothing and writes nothing.

use medulla_catalog_types::architecture_view::ArchitectureView;
use medulla_decision_types::decision::planning_admission::{
    DesignDifference, MissingKind, PlanningAdmissionRequest,
};

/// Compare the plan's features with the wired call sites of the view.
///
/// A feature is wired when its `container` exists in the view and its attach
/// point `file` is one of that container's `support` paths. The differences come
/// back in feature order: a feature whose container is absent is
/// `MissingKind::Edge` (nothing in the view reaches it), a feature whose
/// container exists but whose file is not a support path is
/// `MissingKind::AttachPoint`.
pub fn compare_features(
    view: &ArchitectureView,
    request: &PlanningAdmissionRequest,
) -> Vec<DesignDifference> {
    request
        .features
        .iter()
        .filter_map(|feature| {
            let missing = match view
                .containers
                .iter()
                .find(|container| container.name == feature.container)
            {
                None => MissingKind::Edge,
                Some(container)
                    if container
                        .support
                        .iter()
                        .any(|path| *path == feature.attach_point.file) =>
                {
                    return None;
                }
                Some(_) => MissingKind::AttachPoint,
            };
            Some(DesignDifference {
                feature_id: feature.feature_id.clone(),
                container: feature.container.clone(),
                missing,
            })
        })
        .collect()
}

/// Whether the plan is admitted: it is when the difference list is empty.
pub fn admits(differences: &[DesignDifference]) -> bool {
    differences.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_catalog_types::architecture_view::ViewContainer;
    use medulla_decision_types::decision::planning_admission::{AttachPoint, PlanFeature};
    use medulla_types::contract::{BoundedVec, Digest256};

    fn view() -> ArchitectureView {
        ArchitectureView {
            containers: vec![
                ViewContainer {
                    name: "crate:a".into(),
                    technology: "rust".into(),
                    support: vec!["src/lib.rs".into(), "src/run.rs".into()],
                },
                ViewContainer {
                    name: "crate:b".into(),
                    technology: "rust".into(),
                    support: vec!["src/main.rs".into()],
                },
            ],
            edges: Vec::new(),
        }
    }

    fn feature(id: &str, container: &str, file: &str) -> PlanFeature {
        PlanFeature {
            feature_id: id.into(),
            description_digest: Digest256::sha256(id.as_bytes()),
            container: container.into(),
            attach_point: AttachPoint {
                file: file.into(),
                symbol: "run".into(),
                operation: "call".into(),
            },
            gap_digests: BoundedVec::new(vec![Digest256::sha256(b"gap")]).unwrap(),
        }
    }

    fn request(features: Vec<PlanFeature>) -> PlanningAdmissionRequest {
        PlanningAdmissionRequest {
            repository_id: "repo:1".into(),
            repository_revision: "abc123".into(),
            architecture_view_digest: Digest256::sha256(b"view"),
            gap_proposal_digests: BoundedVec::new(vec![Digest256::sha256(b"gap")]).unwrap(),
            features: BoundedVec::new(features).unwrap(),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R171.2
    #[test]
    fn set_difference_orders_differences() {
        let wired = request(vec![
            feature("f0", "crate:a", "src/run.rs"),
            feature("f1", "crate:b", "src/main.rs"),
        ]);
        let differences = compare_features(&view(), &wired);
        assert!(differences.is_empty());
        assert!(admits(&differences));

        let unwired = request(vec![
            feature("f0", "crate:a", "src/run.rs"),
            feature("f1", "crate:zz", "src/lib.rs"),
            feature("f2", "crate:b", "src/other.rs"),
            feature("f3", "crate:a", "src/lib.rs"),
            feature("f4", "crate:missing", "src/lib.rs"),
        ]);
        let differences = compare_features(&view(), &unwired);
        assert!(!admits(&differences));
        assert_eq!(
            differences,
            vec![
                DesignDifference {
                    feature_id: "f1".into(),
                    container: "crate:zz".into(),
                    missing: MissingKind::Edge,
                },
                DesignDifference {
                    feature_id: "f2".into(),
                    container: "crate:b".into(),
                    missing: MissingKind::AttachPoint,
                },
                DesignDifference {
                    feature_id: "f4".into(),
                    container: "crate:missing".into(),
                    missing: MissingKind::Edge,
                },
            ],
            "differences follow feature order"
        );
    }
}
