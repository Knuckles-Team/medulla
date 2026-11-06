//! Medulla's side of the served supply-chain impact method (MEDULLA-CQ-XSRC-R006.5).
//!
//! Per the registry rule, Arbora exposes the intent tool over its registry;
//! this module is the method body Medulla serves: it plans the question's
//! ontology classes through [`source_selection::select_sources`] (sources
//! come from the ontology's `vg:SourceAuthority` rows, never from the query
//! text) and assembles the typed [`SupplyChainImpactReport`] from the planned
//! rows, authority premises and abstentions. The served surface itself is
//! cross-repo (owner: Arbora) and is not claimed here; this slice fixes the
//! method body and proves its output matches the typed model.

use medulla_identity_types::authority_audit::AuditSourceMode;
use medulla_source_types::{
    supply_chain_report::assemble_supply_chain_report, supply_chain_report::ImpactRow,
    supply_chain_report::ReportAbstention, supply_chain_report::ReportError,
    supply_chain_report::SourcePremise, supply_chain_report::SupplyChainImpactReport,
};

use crate::source_selection::{
    select_sources, AbstainReason, CoverageFixture, CoverageResult, Premise,
};

/// Plan `roots` (the question's ontology classes) against the authority
/// fixture and assemble the supply-chain impact report from the recorded
/// rows. A class whose rank-1 source is not connected becomes a typed
/// abstention, never a silent fallback (MEDULLA-CQ-XSRC-R005.4).
pub fn supply_chain_impact(
    fixture: &CoverageFixture,
    roots: &[String],
    hop_budget: usize,
    rows: &[ImpactRow],
) -> Result<SupplyChainImpactReport, ReportError> {
    let results = select_sources(fixture, roots, hop_budget);
    let mut premises: Vec<SourcePremise> = Vec::new();
    for result in &results {
        if let CoverageResult::Covered {
            premises: covered_premises,
            ..
        } = result
        {
            for premise in covered_premises {
                if let Premise::Authority {
                    row,
                    source,
                    class,
                    rank,
                    ..
                } = premise
                {
                    // The planner serves live when no copy has a watermark
                    // (MEDULLA-UNIFIED-DATA-PLANE-R038.1).
                    premises.push(SourcePremise {
                        source: source.clone(),
                        class_served: class.clone(),
                        row: row.clone(),
                        rank: *rank,
                        mode: AuditSourceMode::Live,
                        watermark: None,
                        age_seconds: None,
                    });
                }
            }
        }
    }
    let abstentions = abstentions_from(&results);
    assemble_supply_chain_report(rows, &premises, &abstentions)
}

/// Map the planner's coverage results onto the report's typed abstentions
/// (MEDULLA-CQ-XSRC-R006.3): one variant per source-authority reason, with no
/// second list. Covered results produce none.
pub fn abstentions_from(results: &[CoverageResult]) -> Vec<ReportAbstention> {
    let mut abstentions = Vec::new();
    for result in results {
        match result {
            CoverageResult::Covered { .. } => {}
            CoverageResult::Uncovered { class, .. } => {
                abstentions.push(ReportAbstention::Uncovered {
                    class: class.clone(),
                })
            }
            CoverageResult::Disconnected { class } => {
                abstentions.push(ReportAbstention::Disconnected {
                    class: class.clone(),
                });
            }
            CoverageResult::Abstained {
                class,
                reason,
                source,
                ..
            } => {
                let source = source.clone().unwrap_or_default();
                let class = class.clone();
                abstentions.push(match reason {
                    AbstainReason::Unconnected => ReportAbstention::Unconnected { source, class },
                    AbstainReason::UnapprovedMapping => ReportAbstention::UnapprovedMapping {
                        class,
                        mapping: source,
                    },
                    AbstainReason::Stale => ReportAbstention::Stale { source, class },
                    AbstainReason::ContractMismatch => {
                        ReportAbstention::ContractMismatch { source, class }
                    }
                    AbstainReason::Ambiguous => ReportAbstention::Ambiguous { source, class },
                    AbstainReason::Proposed => ReportAbstention::Proposed { source, class },
                    AbstainReason::RoleInsufficient => {
                        ReportAbstention::RoleInsufficient { source, class }
                    }
                });
            }
        }
    }
    abstentions
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-CQ-XSRC-R006.5
    #[test]
    fn the_method_body_returns_a_report_matching_the_typed_model() {
        let fixture = CoverageFixture::new()
            .with_relation("Service", "usesComponent", "ComponentVersion")
            .with_relation(
                "ComponentVersion",
                "supplyChainVersionOf",
                "SoftwareComponent",
            )
            .with_relation("SoftwareComponent", "publishedBy", "Supplier")
            .with_relation("ComponentVersion", "affectsComponent", "Vulnerability")
            .with_authority("Supplier", "erpnext-suppliers", "erpnext-mcp", 5, 1)
            .with_authority(
                "Vulnerability",
                "gitlab-vulnerabilities",
                "gitlab-mcp",
                3,
                1,
            )
            .with_authority("ContainerImage", "dockerhub-images", "dockerhub-mcp", 4, 1)
            .with_unconnected_source("dockerhub-mcp");
        let roots = vec![
            "Service".to_string(),
            "SoftwareComponent".to_string(),
            "Supplier".to_string(),
            "Vulnerability".to_string(),
            "ContainerImage".to_string(),
        ];
        let rows = vec![ImpactRow {
            service: "http://knuckles.team/xsrc#checkout".to_string(),
            supplier: "http://knuckles.team/xsrc#supplier-acme".to_string(),
            component_version: "http://knuckles.team/xsrc#version-1.18.0".to_string(),
            vulnerability: Some("CVE-2021-23017".to_string()),
            impact_kind: medulla_source_types::supply_chain_report::ImpactKind::RuntimeChain,
            source: "gitlab-mcp".to_string(),
        }];
        let report = supply_chain_impact(&fixture, &roots, 2, &rows).expect("the report assembles");
        let encoded = serde_json::to_string(&report).expect("the report encodes");
        let decoded: SupplyChainImpactReport =
            serde_json::from_str(&encoded).expect("the encoding is the typed model");
        assert_eq!(decoded, report);
        assert_eq!(report.service_impacts.len(), 1);
        assert!(!report.service_impacts[0].items.is_empty());
        assert!(report
            .sources_used
            .iter()
            .any(|source| source.source == "erpnext-mcp"));
        // The unconnected dockerhub source is a typed abstention, not a fallback.
        assert!(report.abstentions.iter().any(|abstention| matches!(
            abstention,
            ReportAbstention::Unconnected { source, class }
                if source == "dockerhub-mcp" && class == "ContainerImage"
        )));
    }

    // spec: MEDULLA-CQ-XSRC-R006.3
    #[test]
    fn one_report_per_abstention_kind() {
        use medulla_identity_types::authority_audit::NotUsedReason;
        use std::collections::BTreeSet;

        let reasons = [
            NotUsedReason::Unconnected,
            NotUsedReason::UnapprovedMapping,
            NotUsedReason::Stale,
            NotUsedReason::ContractMismatch,
            NotUsedReason::Ambiguous,
            NotUsedReason::Proposed,
            NotUsedReason::RoleInsufficient,
        ];
        let kind = |value: serde_json::Value, field: &str| -> String {
            value
                .get(field)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| value.as_str().unwrap().to_string())
        };
        let mut report_kinds = BTreeSet::new();
        for reason in reasons {
            let results = vec![CoverageResult::Abstained {
                class: "Vulnerability".to_string(),
                facet: None,
                reason,
                source: Some("gitlab-mcp".to_string()),
                premises: Vec::new(),
            }];
            // A report with an abstention is still returned, never an error.
            let report = assemble_supply_chain_report(&[], &[], &abstentions_from(&results))
                .expect("a report with an abstention is returned");
            assert_eq!(report.abstentions.len(), 1);
            let reported = kind(
                serde_json::to_value(&report.abstentions[0]).unwrap(),
                "kind",
            );
            assert_eq!(
                reported,
                kind(serde_json::to_value(reason).unwrap(), "kind")
            );
            report_kinds.insert(reported);
        }
        let engine_kinds: BTreeSet<String> = reasons
            .iter()
            .map(|reason| kind(serde_json::to_value(reason).unwrap(), "kind"))
            .collect();
        assert_eq!(report_kinds, engine_kinds);
        // Uncovered and Disconnected stay alongside the engine reasons.
        let extra = abstentions_from(&[CoverageResult::Disconnected {
            class: "Legacy".to_string(),
        }]);
        assert!(matches!(extra[0], ReportAbstention::Disconnected { .. }));
    }
}
