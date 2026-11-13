//! The typed report of the cross-source competency questions
//! (MEDULLA-CQ-XSRC-R008.4): which services a vulnerable image exposes, which
//! capabilities a host limit puts at risk, and which open incidents touch a
//! contract. One report shape serves the three questions. It reuses
//! [`SourceUsed`] and [`ReportAbstention`] of the supply-chain report, so there
//! is no second list of sources or abstention reasons.

use serde::{Deserialize, Serialize};

use crate::supply_chain_report::{ReportAbstention, SourceUsed};

/// The closed set of served cross-source questions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrossSourceQuestionId {
    /// Which services are exposed by a vulnerability in an image we deploy.
    ExposedServices,
    /// Which capabilities are at risk because a host is over a limit.
    HostLimitCapabilities,
    /// Which open incidents or changes touch a service under contract.
    ContractIncidents,
}

/// Why a cross-source question or report was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CrossSourceError {
    /// The question name is not one of the closed set.
    UnknownQuestion { name: String },
    /// An item named no source.
    ItemWithoutSource,
}

impl CrossSourceError {
    /// The stable wire code of the refusal.
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownQuestion { .. } => "CQ_XSRC_UNKNOWN_QUESTION",
            Self::ItemWithoutSource => "CQ_XSRC_ITEM_WITHOUT_SOURCE",
        }
    }
}

impl std::fmt::Display for CrossSourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownQuestion { name } => write!(
                f,
                "{}: `{name}` is not a served cross-source question",
                self.code()
            ),
            Self::ItemWithoutSource => write!(
                f,
                "{}: an item named no source; items are refused, not defaulted",
                self.code()
            ),
        }
    }
}

impl std::error::Error for CrossSourceError {}

impl CrossSourceQuestionId {
    /// Every served question, in wire order.
    pub const ALL: [Self; 3] = [
        Self::ExposedServices,
        Self::HostLimitCapabilities,
        Self::ContractIncidents,
    ];

    /// The wire name of the question.
    pub fn name(self) -> &'static str {
        match self {
            Self::ExposedServices => "exposed_services",
            Self::HostLimitCapabilities => "host_limit_capabilities",
            Self::ContractIncidents => "contract_incidents",
        }
    }

    /// Parse a wire name; an unknown name is refused with its typed code.
    pub fn parse(name: &str) -> Result<Self, CrossSourceError> {
        Self::ALL
            .into_iter()
            .find(|question| question.name() == name)
            .ok_or_else(|| CrossSourceError::UnknownQuestion {
                name: name.to_string(),
            })
    }

    /// The ontology classes the question plans over. Sources come only from
    /// the authority rows of these classes, never from the query text.
    pub fn root_classes(self) -> &'static [&'static str] {
        match self {
            Self::ExposedServices => &["ContainerImage", "Vulnerability", "Service"],
            Self::HostLimitCapabilities => &["BusinessCapability", "Service", "Host"],
            Self::ContractIncidents => &["Incident", "Change", "Service", "CustomerContract"],
        }
    }
}

/// One related fact of an item: the relation and the value it points at.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RelatedValue {
    /// The relation name (for example `image`, `vulnerability`, `host`).
    pub relation: String,
    /// The related value (an IRI or a literal).
    pub value: String,
}

/// One answer item: a subject, the facts related to it, and the source that
/// served it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossSourceItem {
    /// The subject the item is about.
    pub subject_iri: String,
    /// The facts related to the subject, sorted.
    pub related: Vec<RelatedValue>,
    /// The source that served the item.
    pub source: String,
}

impl CrossSourceItem {
    /// Build one item, refusing an item with no source.
    pub fn try_new(
        subject_iri: &str,
        mut related: Vec<RelatedValue>,
        source: &str,
    ) -> Result<Self, CrossSourceError> {
        if source.trim().is_empty() {
            return Err(CrossSourceError::ItemWithoutSource);
        }
        related.sort();
        Ok(Self {
            subject_iri: subject_iri.to_string(),
            related,
            source: source.to_string(),
        })
    }
}

/// The answer to one cross-source question.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossSourceReport {
    /// The question answered.
    pub question: CrossSourceQuestionId,
    /// The answer items, sorted by subject then source.
    pub items: Vec<CrossSourceItem>,
    /// The sources the plan used, with the authority premise of each.
    pub sources_used: Vec<SourceUsed>,
    /// Why a class or source is absent from the answer.
    pub abstentions: Vec<ReportAbstention>,
}

/// Pure, deterministic assembly: equal inputs give byte-identical encodings.
pub fn assemble_cross_source_report(
    question: CrossSourceQuestionId,
    mut items: Vec<CrossSourceItem>,
    mut sources_used: Vec<SourceUsed>,
    abstentions: Vec<ReportAbstention>,
) -> CrossSourceReport {
    items.sort_by(|left, right| {
        (&left.subject_iri, &left.source).cmp(&(&right.subject_iri, &right.source))
    });
    sources_used.sort_by(|left, right| {
        (&left.source, &left.class_served, &left.authority_premise).cmp(&(
            &right.source,
            &right.class_served,
            &right.authority_premise,
        ))
    });
    sources_used.dedup();
    let mut abstentions = abstentions;
    abstentions.sort_by_key(|abstention| serde_json::to_string(abstention).unwrap_or_default());
    CrossSourceReport {
        question,
        items,
        sources_used,
        abstentions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_identity_types::authority_audit::AuditSourceMode;

    fn related(relation: &str, value: &str) -> RelatedValue {
        RelatedValue {
            relation: relation.to_string(),
            value: value.to_string(),
        }
    }

    // spec: MEDULLA-CQ-XSRC-R008.4
    #[test]
    fn report_round_trips() {
        let item = CrossSourceItem::try_new(
            "https://knuckles-team.github.io/ontology/source-record#checkout",
            vec![
                related("vulnerability", "CVE-2021-23017"),
                related("image", "nginx:1.18"),
            ],
            "gitlab-mcp",
        )
        .expect("an item with a source builds");
        let used = SourceUsed::try_new(
            "gitlab-mcp",
            "Vulnerability",
            "row-1 (rank 1)",
            AuditSourceMode::Live,
            None,
            None,
        )
        .expect("a live source builds");
        let abstention = ReportAbstention::Unconnected {
            source: "dockerhub-mcp".to_string(),
            class: "ContainerImage".to_string(),
        };
        let first = assemble_cross_source_report(
            CrossSourceQuestionId::ExposedServices,
            vec![item.clone()],
            vec![used.clone(), used.clone()],
            vec![abstention.clone()],
        );
        let again = assemble_cross_source_report(
            CrossSourceQuestionId::ExposedServices,
            vec![item],
            vec![used],
            vec![abstention],
        );
        assert_eq!(first, again, "assembly is deterministic");
        assert_eq!(first.sources_used.len(), 1, "a source is listed once");
        assert_eq!(
            first.items[0].related[0].relation, "image",
            "related is sorted"
        );
        let encoded = serde_json::to_string(&first).expect("the report encodes");
        let decoded: CrossSourceReport =
            serde_json::from_str(&encoded).expect("the encoding is the typed model");
        assert_eq!(decoded, first);
    }

    // spec: MEDULLA-CQ-XSRC-R008.4
    #[test]
    fn an_item_without_a_source_and_an_unknown_question_are_refused() {
        let refused = CrossSourceItem::try_new("iri", Vec::new(), "  ").unwrap_err();
        assert_eq!(refused.code(), "CQ_XSRC_ITEM_WITHOUT_SOURCE");
        let unknown = CrossSourceQuestionId::parse("everything").unwrap_err();
        assert_eq!(unknown.code(), "CQ_XSRC_UNKNOWN_QUESTION");
        for question in CrossSourceQuestionId::ALL {
            assert_eq!(CrossSourceQuestionId::parse(question.name()), Ok(question));
        }
    }
}
