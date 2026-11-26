//! Medulla's side of the served supply-chain impact method (MEDULLA-CQ-XSRC-R006.5).
//!
//! Per the registry rule, Arbora exposes the intent tool over its registry;
//! this module is the method body Medulla serves: it plans the question's
//! ontology classes through [`source_selection::select_sources_with`] (sources
//! come from the ontology's `vg:SourceAuthority` rows, never from the query
//! text) and assembles the typed [`SupplyChainImpactReport`] from the planned
//! rows, authority premises and abstentions. The served surface itself is
//! cross-repo (owner: Arbora) and is not claimed here; this slice fixes the
//! method body and proves its output matches the typed model.

use std::collections::{BTreeMap, BTreeSet};

use medulla_source_types::{
    supply_chain_report::assemble_supply_chain_report, supply_chain_report::ContractCoverage,
    supply_chain_report::ImpactRow,
    supply_chain_report::ReportAbstention, supply_chain_report::ReportError,
    supply_chain_report::sources_used_from_audit, supply_chain_report::SupplyChainImpactReport,
};

use crate::source_selection::{
    audit_from, select_sources_with, AbstainReason, CoverageFixture, CoverageResult,
    SelectionQuery,
};

/// Plan `roots` (the question's ontology classes) against the authority
/// fixture and assemble the supply-chain impact report from the recorded
/// rows. A class whose rank-1 source is not connected becomes a typed
/// abstention, never a silent fallback (MEDULLA-CQ-XSRC-R005.4). `contracts` are the asked-about
/// customer contract ids and `covers` the `(contract, service)` pairs of their `coversService`
/// edges; both empty gives the base report (MEDULLA-SUPPLY-VOCAB-R005.9).
pub fn supply_chain_impact(
    fixture: &CoverageFixture,
    roots: &[String],
    hop_budget: usize,
    rows: &[ImpactRow],
    contracts: &[String],
    covers: &[(String, String)],
) -> Result<SupplyChainImpactReport, ReportError> {
    // An impact report answers a question about the present, so a source whose role is
    // inspiration cannot serve it (MEDULLA-SRC-AUTH-R010.4).
    let query = SelectionQuery {
        needs_current_state: true,
        ..SelectionQuery::default()
    };
    let results = select_sources_with(fixture, roots, hop_budget, &query);
    // `sources_used` is the authority audit of the same verdicts, entry for entry
    // (MEDULLA-SRC-AUTH-R007.4): there is no second builder.
    let audit = audit_from(&results);
    let mut abstentions = abstentions_from(&results);
    // A source the report names `RoleInsufficient` contributes no fact: its rows are excluded
    // and the abstention is the only trace of it (MEDULLA-CQ-XSRC-R012.2).
    let excluded: BTreeSet<&str> = abstentions
        .iter()
        .filter_map(|abstention| match abstention {
            ReportAbstention::RoleInsufficient { source, .. } => Some(source.as_str()),
            _ => None,
        })
        .collect();
    let rows: Vec<ImpactRow> = rows
        .iter()
        .filter(|row| !excluded.contains(row.source.as_str()))
        .cloned()
        .collect();
    let rows = rows.as_slice();
    let (coverage, contract_abstentions) = contract_coverage(contracts, covers);
    abstentions.extend(contract_abstentions);
    let mut report = assemble_supply_chain_report(rows, &[], &abstentions)?;
    report.sources_used = sources_used_from_audit(&audit.used)?;
    report.contract_coverage = coverage;
    Ok(report)
}

/// The contract coverage read (MEDULLA-SUPPLY-VOCAB-R005.9): one [`ContractCoverage`] per asked
/// contract with at least one `(contract, service)` pair (contracts sorted, services sorted and
/// deduplicated), and one [`ReportAbstention::ContractUncovered`] per asked contract with none,
/// so a contract with no edge is named, never an empty success. Pairs for a contract that was
/// not asked about are ignored.
pub fn contract_coverage(
    contracts: &[String],
    covers: &[(String, String)],
) -> (Vec<ContractCoverage>, Vec<ReportAbstention>) {
    let asked: BTreeSet<&String> = contracts.iter().collect();
    let mut by_contract: BTreeMap<&String, BTreeSet<&String>> = BTreeMap::new();
    for (contract, service) in covers {
        if asked.contains(contract) {
            by_contract.entry(contract).or_default().insert(service);
        }
    }
    let mut coverage = Vec::new();
    let mut abstentions = Vec::new();
    for contract in asked {
        match by_contract.get(contract) {
            Some(services) => coverage.push(ContractCoverage {
                contract: contract.clone(),
                services: services.iter().map(|service| (*service).clone()).collect(),
            }),
            None => abstentions.push(ReportAbstention::ContractUncovered {
                contract: contract.clone(),
            }),
        }
    }
    (coverage, abstentions)
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
    use crate::source_selection::select_sources;
    use medulla_identity_types::authority_audit::SourceRole;

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
            service: "https://knuckles-team.github.io/ontology/source-record#checkout".to_string(),
            supplier: "https://knuckles-team.github.io/ontology/source-record#supplier-acme"
                .to_string(),
            component_version:
                "https://knuckles-team.github.io/ontology/source-record#version-1.18.0".to_string(),
            vulnerability: Some("CVE-2021-23017".to_string()),
            impact_kind: medulla_source_types::supply_chain_report::ImpactKind::RuntimeChain,
            source: "gitlab-mcp".to_string(),
            mapping: None,
            read_at_ms: None,
        }];
        let report = supply_chain_impact(&fixture, &roots, 2, &rows, &[], &[]).expect("the report assembles");
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

    // spec: MEDULLA-CQ-XSRC-R012.2
    #[test]
    fn a_role_insufficient_source_contributes_no_fact_and_is_named() {
        let fixture = CoverageFixture::new()
            .with_authority("SoftwareComponent", "gh-components", "github-mcp", 1, 1)
            .with_authority("Supplier", "erp-suppliers", "erpnext-mcp", 2, 1)
            .with_source_role("github-mcp", SourceRole::Inspiration);
        let roots = vec!["SoftwareComponent".to_string(), "Supplier".to_string()];
        let row = |version: &str, source: &str| ImpactRow {
            service: "svc".to_string(),
            supplier: "supplier-acme".to_string(),
            component_version: version.to_string(),
            vulnerability: None,
            impact_kind: medulla_source_types::supply_chain_report::ImpactKind::RuntimeChain,
            source: source.to_string(),
            mapping: None,
            read_at_ms: None,
        };
        let rows = vec![row("v-from-github", "github-mcp"), row("v-from-erp", "erpnext-mcp")];
        let report = supply_chain_impact(&fixture, &roots, 0, &rows, &[], &[])
            .expect("the answer is returned, not refused");
        let sources: Vec<&str> = report
            .service_impacts
            .iter()
            .flat_map(|impact| impact.items.iter().map(|item| item.source.as_str()))
            .collect();
        assert_eq!(sources, ["erpnext-mcp"], "no fact of the excluded source");
        let named: Vec<&ReportAbstention> = report
            .abstentions
            .iter()
            .filter(|a| matches!(a, ReportAbstention::RoleInsufficient { .. }))
            .collect();
        assert_eq!(named.len(), 1, "the excluded source is named exactly once");
        assert!(matches!(
            named[0],
            ReportAbstention::RoleInsufficient { source, class }
                if source == "github-mcp" && class == "SoftwareComponent"
        ));
    }

    // proves R005.9: Curated contract-to-service link, and the contract coverage read of the ...
    // spec: MEDULLA-SUPPLY-VOCAB-R005.9
    #[test]
    fn contract_without_edge_is_contract_uncovered() {
        let contracts = vec!["contract-2".to_string(), "contract-1".to_string()];
        let covers = vec![
            ("contract-1".to_string(), "svc-b".to_string()),
            ("contract-1".to_string(), "svc-a".to_string()),
            ("contract-1".to_string(), "svc-a".to_string()),
            ("contract-9".to_string(), "svc-z".to_string()),
        ];
        let (coverage, abstentions) = contract_coverage(&contracts, &covers);
        assert_eq!(
            coverage,
            vec![ContractCoverage {
                contract: "contract-1".to_string(),
                services: vec!["svc-a".to_string(), "svc-b".to_string()],
            }]
        );
        assert_eq!(
            abstentions,
            vec![ReportAbstention::ContractUncovered {
                contract: "contract-2".to_string()
            }]
        );
        // The served report carries both, and no contract asked means the base report.
        let report = supply_chain_impact(&CoverageFixture::new(), &[], 0, &[], &contracts, &covers)
            .expect("the report assembles");
        assert_eq!(report.contract_coverage, coverage);
        assert_eq!(report.abstentions, abstentions);
        let base = supply_chain_impact(&CoverageFixture::new(), &[], 0, &[], &[], &covers)
            .expect("the base report assembles");
        assert!(base.contract_coverage.is_empty() && base.abstentions.is_empty());
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

    // spec: MEDULLA-SRC-AUTH-R007.4
    #[test]
    fn sources_used_equals_audit() {
        let fixture = CoverageFixture::new()
            .with_authority("Supplier", "erpnext-suppliers", "erpnext-mcp", 5, 1)
            .with_authority("Vulnerability", "gitlab-vulnerabilities", "gitlab-mcp", 3, 1)
            .with_authority("ContainerImage", "dockerhub-images", "dockerhub-mcp", 4, 1)
            .with_unconnected_source("dockerhub-mcp");
        let roots = vec![
            "Supplier".to_string(),
            "Vulnerability".to_string(),
            "ContainerImage".to_string(),
        ];
        let report = supply_chain_impact(&fixture, &roots, 0, &[], &[], &[]).expect("the report assembles");
        let audit = audit_from(&select_sources(&fixture, &roots, 0));
        // One entry per used audit entry, in the audit's order, with its premises and mode.
        assert!(!audit.used.is_empty());
        assert_eq!(report.sources_used.len(), audit.used.len());
        for (reported, entry) in report.sources_used.iter().zip(&audit.used) {
            assert_eq!(reported.source, entry.source);
            assert_eq!(reported.class_served, entry.concept);
            assert_eq!(reported.authority_premise, entry.premises.join("; "));
            assert_eq!(Some(reported.mode), entry.mode);
        }
        // A not-used row (the unconnected source) never appears in `sources_used`.
        assert!(!audit.not_used.is_empty());
        for not_used in &audit.not_used {
            assert!(report
                .sources_used
                .iter()
                .all(|used| used.source != not_used.source));
        }
    }
}
