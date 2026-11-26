//! The served catalog reads: browse, search and asset detail (MEDULLA-TYPED-PACKS-R108.17).
//!
//! `Method::CatalogRead` carries one [`CatalogReadOp`]. The handler validates every clause of the
//! request, then runs the registered stored query of the op as its plan and shapes the rows into
//! one [`CatalogReadAnswer`]. The caller sends an op and its params and never query text.

use serde::{Deserialize, Serialize};

use crate::stewardship::StewardshipTarget;

/// The registered stored query behind `Browse`.
pub const CATALOG_ASSETS_BY_KIND: &str = "catalog.assets_by_kind";
/// The registered stored query behind `Search`.
pub const CATALOG_ASSETS_SEARCH: &str = "catalog.assets_search";
/// The registered stored query behind `Detail`.
pub const CATALOG_ASSET_DETAIL: &str = "catalog.asset_detail";

/// Refusal code: the asset is unknown or the caller may not read it (one body for both).
pub const CATALOG_ASSET_NOT_FOUND: &str = "ASSET_NOT_FOUND";
/// Refusal code: an unknown kind, a bad cursor, a page size out of bounds or a blank id.
pub const CATALOG_INVALID_ARGUMENT: &str = "INVALID_ARGUMENT";

/// The kinds of catalog asset a browse or a search may name: the local names of the classes
/// below `cat:Asset` in the catalog ontology.
pub const CATALOG_ASSET_KINDS: [&str; 26] = [
    "Catalog",
    "DataResource",
    "Field",
    "ToolAsset",
    "PromptAsset",
    "SkillAsset",
    "AgentAsset",
    "AgentTemplateAsset",
    "A2AAgentAsset",
    "AgentVersion",
    "OrgUnit",
    "TeamAsset",
    "PersonAsset",
    "RoleAsset",
    "BusinessDomain",
    "Product",
    "ServiceAsset",
    "DashboardAsset",
    "ReportAsset",
    "Article",
    "WorkflowDefinitionAsset",
    "WorkflowRun",
    "WorkflowStepAsset",
    "KnowledgeGraph",
    "SpecialistPackage",
    "ProviderAsset",
];

/// The kind a glossary term answers a search under (MEDULLA-TYPED-PACKS-R801.6): the local name
/// of `cat:CatalogGlossaryTerm`. A search may name it beside the asset kinds; a browse may not.
pub const CATALOG_GLOSSARY_KIND: &str = "CatalogGlossaryTerm";

/// One catalog read (`Method::CatalogRead`, MEDULLA-TYPED-PACKS-R108.17).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum CatalogReadOp {
    /// Assets by kind and owner: the kind counts and one page of asset rows.
    Browse {
        /// Restrict the rows to one asset kind.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
        /// Restrict the rows to the assets of one owner.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner: Option<String>,
        /// Page size; at most `catalog.search_page_max`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<u32>,
        /// The `next_cursor` of the previous page.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cursor: Option<String>,
    },
    /// Ranked text search over every kind, or over the listed kinds.
    Search {
        /// The search text.
        text: String,
        /// Restrict the hits to these asset kinds; empty means every kind.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        kinds: Vec<String>,
        /// Page size; at most `catalog.search_page_max`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<u32>,
        /// The `next_cursor` of the previous page.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cursor: Option<String>,
    },
    /// One asset: owners, glossary terms, direct lineage neighbours, ratings and comments.
    Detail {
        /// The asset's IRI.
        asset_id: String,
    },
    /// The owners and stewards that govern one target (MEDULLA-TYPED-PACKS-R108.18): the most
    /// specific stored assignment level wins, exactly as a change request resolves its
    /// reviewers. A caller reads this to decide whom to admit before it writes.
    Governance {
        /// The asset, concept scheme and domain the target belongs to.
        target: StewardshipTarget,
    },
    /// One page of curation work in the caller's tenant (MEDULLA-TYPED-PACKS-R108.18): the
    /// glossary description proposals and the stewardship change requests, ordered by id.
    Proposals {
        /// Restrict the rows to one state: `proposed`, `approved`, `rejected`, `open` or
        /// `applied`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state: Option<String>,
        /// Restrict the rows to one target (an asset or a glossary term IRI).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
        /// Page size; at most `catalog.search_page_max`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<u32>,
        /// The `next_cursor` of the previous page.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cursor: Option<String>,
    },
}

/// The owners and stewards governing one target (answer of `Governance`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CatalogGovernance {
    /// The principals holding the owner role at the governing level, sorted.
    pub owners: Vec<String>,
    /// The principals holding the steward role at the governing level, sorted.
    pub stewards: Vec<String>,
}

/// What a curation row is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum CatalogProposalKind {
    /// A `cat:DescriptionProposal` of a glossary term or asset description.
    Description,
    /// A `cat:ChangeRequest` of the stewardship flow.
    ChangeRequest,
}

/// One curation row of a `Proposals` page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CatalogProposalRow {
    /// The proposal or change request node id.
    pub id: String,
    /// Which of the two it is.
    pub kind: CatalogProposalKind,
    /// The term or asset it addresses.
    pub target: String,
    /// Its state name as stored.
    pub state: String,
    /// The principal that proposed or opened it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opened_by: Option<String>,
    /// The principals who may review it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reviewers: Vec<String>,
    /// The proposed description text (descriptions only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// The change request that governs a description proposal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_request: Option<String>,
}

/// The number of assets of one kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CatalogKindCount {
    /// The asset kind.
    pub kind: String,
    /// How many assets of the kind the caller may read.
    pub count: u64,
}

/// One asset row of a browse or a search page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CatalogAssetRow {
    /// The asset's IRI.
    pub asset: String,
    /// The asset kind.
    pub kind: String,
    /// The asset's display name, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The asset's owner, when it has one (browse only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// The relevance score (search only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
}

/// One comment on an asset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CatalogComment {
    /// The annotation node.
    pub id: String,
    /// The comment text.
    pub text: String,
    /// The verified agent id that wrote it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// When it was written, in milliseconds since the epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_ms: Option<u64>,
}

/// The detail of one asset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CatalogAssetDetail {
    /// The asset's IRI.
    pub asset: String,
    /// The asset kind.
    pub kind: String,
    /// The asset's display name, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The asset's owners.
    pub owners: Vec<String>,
    /// The labels of the glossary terms that define the asset.
    pub terms: Vec<String>,
    /// The direct upstream lineage neighbours.
    pub upstream: Vec<String>,
    /// The direct downstream lineage neighbours.
    pub downstream: Vec<String>,
    /// How many ratings the asset has.
    pub rating_count: u64,
    /// The mean rating; absent with no rating.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rating_mean: Option<f64>,
    /// The comments on the asset.
    pub comments: Vec<CatalogComment>,
}

/// What every catalog read answers. A browse fills `kind_counts` and `rows`, a search fills
/// `rows`, a detail fills `detail`, a governance read fills `governance` and a proposals read
/// fills `proposals`; a page with more rows after it carries `next_cursor`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CatalogReadAnswer {
    /// The asset count of each kind the caller may read (browse).
    pub kind_counts: Vec<CatalogKindCount>,
    /// One page of asset rows (browse and search).
    pub rows: Vec<CatalogAssetRow>,
    /// The cursor of the next page, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    /// The asset detail (detail).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<CatalogAssetDetail>,
    /// The owners and stewards of the target (governance).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub governance: Option<CatalogGovernance>,
    /// One page of curation rows (proposals); the page's cursor is `next_cursor`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proposals: Vec<CatalogProposalRow>,
}

#[cfg(test)]
mod governance_tests {
    use super::*;
    use crate::stewardship::{StewardRole, StewardshipAssignment, StewardshipScope};

    // spec: MEDULLA-TYPED-PACKS-R108.18
    #[test]
    fn governance_and_proposals_ops_and_answers_round_trip() {
        let ops = [
            serde_json::json!({"op": "governance", "target": {"asset": "table:orders"}}),
            serde_json::json!({"op": "proposals", "state": "open", "limit": 5}),
        ];
        for op in ops {
            let decoded: CatalogReadOp = serde_json::from_value(op.clone()).unwrap();
            assert_eq!(serde_json::to_value(&decoded).unwrap(), op);
        }
        assert!(serde_json::from_value::<CatalogReadOp>(
            serde_json::json!({"op": "governance", "target": {}, "extra": 1})
        )
        .is_err());

        let answer = CatalogReadAnswer {
            kind_counts: Vec::new(),
            rows: Vec::new(),
            next_cursor: Some("1".into()),
            detail: None,
            governance: Some(CatalogGovernance {
                owners: vec!["o".into()],
                stewards: vec!["s".into()],
            }),
            proposals: vec![CatalogProposalRow {
                id: "p1".into(),
                kind: CatalogProposalKind::ChangeRequest,
                target: "table:orders".into(),
                state: "open".into(),
                opened_by: Some("s".into()),
                reviewers: vec!["o".into()],
                text: None,
                change_request: None,
            }],
        };
        let bytes = serde_json::to_vec(&answer).unwrap();
        assert_eq!(serde_json::from_slice::<CatalogReadAnswer>(&bytes).unwrap(), answer);
        // A browse answer keeps its bytes: the new fields are absent when empty.
        let browse = serde_json::to_value(CatalogReadAnswer {
            governance: None,
            proposals: Vec::new(),
            ..answer
        })
        .unwrap();
        assert!(browse.get("governance").is_none() && browse.get("proposals").is_none());
    }

    // spec: MEDULLA-TYPED-PACKS-R108.18
    #[test]
    fn an_assignment_reads_back_from_its_typed_node_properties() {
        let properties = |kind: &str, role: &str| {
            serde_json::json!({
                "scope_kind": kind, "scope_id": "x", "role": role, "principal": "p"
            })
            .as_object()
            .cloned()
            .unwrap()
        };
        assert_eq!(
            StewardshipAssignment::from_properties(&properties("concept_scheme", "owner")),
            Some(StewardshipAssignment {
                scope: StewardshipScope::ConceptScheme("x".into()),
                role: StewardRole::Owner,
                principal: "p".into(),
            })
        );
        assert_eq!(StewardshipAssignment::from_properties(&properties("table", "owner")), None);
        assert_eq!(StewardshipAssignment::from_properties(&properties("asset", "boss")), None);
    }
}
