//! The `changeset_review` question family (MEDULLA-DECISION-ENGINE-R138.101.2).
//!
//! A semantic review of a graph changeset reports findings in five flaw
//! classes, each bound to the lines it cites. Each finding is a `Declared`
//! option: `classification` holds its class and severity, `numbers` its lines
//! and confidence, `texts` its path, evidence and false-positive evidence.
//! The family admits the review (revisions named, classes supported and
//! reviewed, every finding proven and inside the changeset, the changed-files
//! digest honest), filters the low-impact findings the reviewer itself doubts
//! (`filter_findings`), and answers `block` when a kept finding reaches
//! `review.block_severity`, else `pass`. `Decide` is evaluate-only and writes
//! nothing; the findings are the caller's claims.

use std::collections::BTreeSet;

use medulla_decision_stats::decision::changeset_review::{block_severity, filter_findings};
use medulla_decision_types::decision::changeset_review::{
    changed_files_digest, FindingDisposition, ReviewClass, ReviewFinding, Severity, CLASS_PREFIX,
    SEVERITY_PREFIX,
};
use medulla_decision_types::decision::statistical::declared::{DeclaredOption, DeclaredText};
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, StatisticalErrorCode, TypedValue,
};
use medulla_decision_types::decision::{DecisionPolicy, PremiseClass, PremiseRef};
use medulla_graph_core::graph::GraphView;

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};
use crate::decide::stat_support::refusal;

use super::support::{advisory, invalid, param, text_param};
use super::DecisionFamily;

/// The family of the `changeset_review` question kind.
pub struct ChangesetReviewFamily;

/// What the review states about the changeset it covers.
struct Asked<'a> {
    changed: BTreeSet<&'a str>,
    reviewed: BTreeSet<ReviewClass>,
}

fn iri_list<'a>(request: &'a DecideRequest, name: &str) -> Result<Vec<&'a str>, String> {
    match param(request, name)? {
        Some(TypedValue::IriList(list)) => Ok(list.iter().map(String::as_str).collect()),
        Some(_) => Err(invalid(format!("parameter {name} is not an IRI list"))),
        None => Err(invalid(format!("parameter {name} is required"))),
    }
}

fn revision<'a>(request: &'a DecideRequest, name: &str) -> Result<&'a str, String> {
    text_param(request, name)?
        .map(str::trim)
        .filter(|rev| !rev.is_empty())
        .ok_or_else(|| {
            refusal(
                StatisticalErrorCode::ReviewRevisionMissing,
                format!("parameter {name} names no revision"),
            )
        })
}

fn text<'a>(texts: &'a [DeclaredText], key: &str) -> Option<&'a str> {
    texts.iter().find(|t| t.key == key).map(|t| t.text.as_str())
}

fn number(option: &DeclaredOption, key: &str) -> Option<u32> {
    option
        .numbers
        .iter()
        .find(|n| n.key == key)
        .and_then(|n| u32::try_from(n.q32 >> 32).ok())
}

fn absent(option: &DeclaredOption, what: &str) -> String {
    refusal(
        StatisticalErrorCode::ReviewProofAbsent,
        format!("finding {} carries no {what}", option.option_id),
    )
}

/// The finding a declared option carries, admitted against the review.
fn finding(option: &DeclaredOption, asked: &Asked) -> Result<ReviewFinding, String> {
    let named = |prefix: &str| {
        option
            .classification
            .iter()
            .find_map(|iri| iri.strip_prefix(prefix))
    };
    let path = text(option.texts.as_slice(), "path")
        .filter(|p| !p.trim().is_empty())
        .ok_or_else(|| absent(option, "path"))?;
    let evidence = text(option.texts.as_slice(), "evidence")
        .filter(|e| !e.trim().is_empty())
        .ok_or_else(|| absent(option, "evidence"))?;
    let (Some(line_start), Some(line_end), Some(confidence)) = (
        number(option, "line_start"),
        number(option, "line_end"),
        number(option, "confidence_permille"),
    ) else {
        return Err(absent(option, "cited lines and confidence"));
    };
    let class_name = named(CLASS_PREFIX).ok_or_else(|| absent(option, "flaw class"))?;
    let class = ReviewClass::parse(class_name).ok_or_else(|| {
        refusal(
            StatisticalErrorCode::ReviewClassUnsupported,
            format!(
                "finding {} cites unsupported class {class_name}",
                option.option_id
            ),
        )
    })?;
    let severity_name = named(SEVERITY_PREFIX).ok_or_else(|| absent(option, "severity"))?;
    let severity = Severity::parse(severity_name).ok_or_else(|| {
        invalid(format!(
            "finding {} names unknown severity {severity_name}",
            option.option_id
        ))
    })?;
    let finding = ReviewFinding {
        class,
        path: path.to_string(),
        line_start,
        line_end,
        evidence: evidence.to_string(),
        severity,
        confidence_permille: u16::try_from(confidence).unwrap_or(u16::MAX),
        false_positive_evidence: text(option.texts.as_slice(), "false_positive_evidence")
            .map(str::to_string),
    };
    finding
        .validate()
        .map_err(|detail| refusal(StatisticalErrorCode::ReviewProofAbsent, detail))?;
    if !asked.reviewed.contains(&class) {
        return Err(refusal(
            StatisticalErrorCode::ReviewClassUnreviewed,
            format!(
                "finding {} cites class {class_name}, which the review does not cover",
                option.option_id
            ),
        ));
    }
    if !asked.changed.contains(path) {
        return Err(refusal(
            StatisticalErrorCode::ReviewFindingOutsideChangeset,
            format!(
                "finding {} cites {path}, which is not changed",
                option.option_id
            ),
        ));
    }
    Ok(finding)
}

fn asked(request: &DecideRequest) -> Result<Asked<'_>, String> {
    revision(request, "base_rev")?;
    revision(request, "head_rev")?;
    let mut reviewed = BTreeSet::new();
    for iri in iri_list(request, "classes_reviewed")? {
        let class = iri
            .strip_prefix(CLASS_PREFIX)
            .and_then(ReviewClass::parse)
            .ok_or_else(|| {
                refusal(
                    StatisticalErrorCode::ReviewClassUnsupported,
                    format!("review class {iri} is not one of the five"),
                )
            })?;
        reviewed.insert(class);
    }
    let paths = iri_list(request, "changed_paths")?;
    let stated = text_param(request, "changed_files_digest")?.unwrap_or_default();
    let actual = changed_files_digest(paths.iter().copied());
    if stated != actual {
        return Err(refusal(
            StatisticalErrorCode::ReviewChangesetDigestMismatch,
            format!("changed_files_digest is {stated:?}, the changed paths digest to {actual}"),
        ));
    }
    Ok(Asked {
        changed: paths.into_iter().collect(),
        reviewed,
    })
}

/// Every finding of the question, admitted, with its option id.
fn findings(request: &DecideRequest) -> Result<Vec<(String, ReviewFinding)>, String> {
    let asked = asked(request)?;
    let CandidateSource::Declared { options } = &request.candidates else {
        return Err(invalid("a changeset_review question declares its findings"));
    };
    options
        .iter()
        .map(|option| Ok((option.option_id.clone(), finding(option, &asked)?)))
        .collect()
}

fn disposition_name(disposition: FindingDisposition) -> &'static str {
    match disposition {
        FindingDisposition::Kept => "kept",
        FindingDisposition::FilteredLowImpact => "filtered_low_impact",
    }
}

fn conclude_review(request: &DecideRequest, policy: &DecisionPolicy) -> Result<Executed, String> {
    let admitted = findings(request)?;
    let only: Vec<ReviewFinding> = admitted.iter().map(|(_, f)| f.clone()).collect();
    let dispositions = filter_findings(&only, policy);
    let threshold = block_severity(policy);
    let blocks = dispositions
        .iter()
        .any(|(f, d)| *d == FindingDisposition::Kept && f.severity >= threshold);
    let mut executed = advisory(vec![if blocks { "block" } else { "pass" }.to_string()])?;
    executed.family_premises = admitted
        .iter()
        .zip(&dispositions)
        .map(|((id, _), (_, disposition))| FamilyPremise {
            subject: "changeset_review",
            fact: format!("review.finding:{id}={}", disposition_name(*disposition)),
            class: PremiseClass::Claim,
            source: FamilyPremiseSource::Request("candidates.options"),
        })
        .collect();
    Ok(executed)
}

impl DecisionFamily for ChangesetReviewFamily {
    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        findings(request).map(|_| ())
    }

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(conclude_review(request, policy))
    }

    fn premises(
        &self,
        _request: &DecideRequest,
        executed: &Executed,
        policy_digest: &str,
    ) -> Vec<PremiseRef> {
        executed
            .family_premises
            .iter()
            .map(|premise| premise.seal(policy_digest))
            .collect()
    }
}
