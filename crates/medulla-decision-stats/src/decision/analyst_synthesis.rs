//! The pure synthesis of analyst findings (MEDULLA-DECISION-ENGINE-R134.13).
//!
//! Accounts for every expected analyst exactly once, refuses conflicting
//! claims, and resolves each citation through a caller-supplied lookup that
//! sees only the records the request's graph view makes visible.

use std::collections::{BTreeMap, BTreeSet};

use medulla_decision_types::decision::analyst_synthesis::{
    AnalystAccount, SupportedConclusion, SynthesisRefusal, SynthesisRequest, SynthesisResult,
    ANALYST_DUPLICATE, ANALYST_MISSING, DECISION_ROLE_ABSENT, INVALID_CITATION, MISSING_CITATION,
    SYNTHESIS_PLAN_MISMATCH, UNRESOLVED_CONFLICT,
};

fn refuse(code: &'static str, detail: impl std::fmt::Display) -> SynthesisRefusal {
    SynthesisRefusal::new(code, detail)
}

/// Synthesise `request`. `plan_digest` is the plan the analysts ran;
/// `min_citations` is `synthesis.min_citations`; `record_digest` returns the
/// content digest of a record visible in the graph candidate view, if any.
/// The refusal is typed; the caller abstains on it with no recommendation.
pub fn synthesize(
    request: &SynthesisRequest,
    plan_digest: &str,
    min_citations: usize,
    record_digest: impl Fn(&str) -> Option<String>,
) -> Result<SynthesisResult, SynthesisRefusal> {
    if request.plan_digest != plan_digest {
        return Err(refuse(
            SYNTHESIS_PLAN_MISMATCH,
            "the findings answer another plan",
        ));
    }
    if !request
        .expected_analyst_ids
        .contains(&request.decision_role_id)
    {
        return Err(refuse(
            DECISION_ROLE_ABSENT,
            format!("`{}` is not an expected analyst", request.decision_role_id),
        ));
    }
    let mut accounting = Vec::new();
    let expected: BTreeSet<&String> = request.expected_analyst_ids.iter().collect();
    for analyst in &request.expected_analyst_ids {
        let findings: Vec<&str> = request
            .findings
            .iter()
            .filter(|finding| &finding.analyst_id == analyst)
            .map(|finding| finding.finding_id.as_str())
            .collect();
        let gaps: Vec<&_> = request
            .gaps
            .iter()
            .filter(|gap| &gap.analyst_id == analyst)
            .collect();
        match (findings.len(), gaps.len()) {
            (0, 0) => {
                return Err(refuse(
                    ANALYST_MISSING,
                    format!("analyst `{analyst}` has neither a finding nor a gap"),
                ))
            }
            (n, 0) if n >= 1 => accounting.push(AnalystAccount::Findings {
                analyst_id: analyst.clone(),
                finding_ids: findings.iter().map(|id| (*id).to_string()).collect(),
            }),
            (0, 1) => accounting.push(AnalystAccount::Gap {
                analyst_id: analyst.clone(),
                reason: gaps[0].reason.clone(),
            }),
            _ => {
                return Err(refuse(
                    ANALYST_DUPLICATE,
                    format!("analyst `{analyst}` appears more than once"),
                ))
            }
        }
    }
    if expected.len() != request.expected_analyst_ids.len() {
        return Err(refuse(
            ANALYST_DUPLICATE,
            "an expected analyst is listed twice",
        ));
    }
    let mut claims: BTreeMap<&str, Vec<&_>> = BTreeMap::new();
    for finding in &request.findings {
        if finding.citations.len() < min_citations {
            return Err(refuse(
                MISSING_CITATION,
                format!(
                    "finding `{}` cites {} of at least {min_citations} records",
                    finding.finding_id,
                    finding.citations.len()
                ),
            ));
        }
        for citation in &finding.citations {
            if record_digest(&citation.evidence_id).as_deref() != Some(citation.digest.as_str()) {
                return Err(refuse(
                    INVALID_CITATION,
                    format!(
                        "`{}` is not a visible record with the cited digest",
                        citation.evidence_id
                    ),
                ));
            }
        }
        claims.entry(&finding.claim_key).or_default().push(finding);
    }
    let mut conclusions = Vec::new();
    for (claim_key, findings) in claims {
        let digest = &findings[0].claim_digest;
        if findings.iter().any(|f| &f.claim_digest != digest) {
            return Err(refuse(
                UNRESOLVED_CONFLICT,
                format!("claim `{claim_key}` has findings with different claim digests"),
            ));
        }
        conclusions.push(SupportedConclusion {
            claim_key: claim_key.to_string(),
            claim_digest: digest.clone(),
            finding_ids: findings.iter().map(|f| f.finding_id.clone()).collect(),
        });
    }
    Ok(SynthesisResult {
        decision_role_id: request.decision_role_id.clone(),
        conclusions,
        accounting,
    })
}
