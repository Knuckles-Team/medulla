//! Skill tiers and the review that qualifies a revision for one
//! (MEDULLA-DECISION-ENGINE-R714.1, R714.5, R715.1).
//!
//! A skill revision is `Domain` (team-reviewed), `Core` (platform-reviewed and tested) or
//! `AlwaysOn` (loaded for every job). A tier needs its review receipt before the revision becomes
//! champion. A persona names the skill set one role receives; the set is the revision list its
//! `Persona` node reaches through `includes` edges.

use serde::{Deserialize, Serialize};

use medulla_types::refusal_code::RefusalCode;

/// The graph class of a persona node (vocabulary R817).
pub const PERSONA_CLASS: &str = "Persona";
/// The link from a persona to each skill revision its role receives.
pub const PERSONA_INCLUDES: &str = "includes";
/// The team whose approver a `Core` or `AlwaysOn` tier needs.
pub const PLATFORM_TEAM: &str = "platform";
/// The link from a principal to a team it belongs to.
pub const MEMBER_OF: &str = "memberOf";
/// The store key prefix of the tier a skill champion holds.
pub const TIER_KEY_PREFIX: &str = "skill-tier:";
/// The premise a tiered promotion seals: the tier the revision is promoted to.
pub const PREMISE_TARGET: &str = "tier:target=";
/// The premise that names the always-on count against the bound.
pub const PREMISE_ALWAYS_ON: &str = "tier:always_on=";
/// The premise that names the persona a load set answered for.
pub const PREMISE_PERSONA: &str = "persona:";

/// The two capabilities every tool subset and load set carries (MEDULLA-DECISION-ENGINE-R715.1):
/// the flag tool of R708 and the friction tool of R710.
pub const ALWAYS_ON_CAPABILITIES: [&str; 2] = ["knowledge.flag", "skills.friction"];

/// `TIER_REVIEW_MISSING`: a tier needs a review receipt it does not hold.
pub const TIER_REVIEW_MISSING: RefusalCode = RefusalCode::new("TIER_REVIEW_MISSING");
/// `ALWAYS_ON_CAPACITY_REACHED`: the tenant already holds `skills.always_on_max` always-on skills.
pub const ALWAYS_ON_CAPACITY_REACHED: RefusalCode = RefusalCode::new("ALWAYS_ON_CAPACITY_REACHED");
/// `PERSONA_UNKNOWN`: the graph holds no persona of that id.
pub const PERSONA_UNKNOWN: RefusalCode = RefusalCode::new("PERSONA_UNKNOWN");
/// `TIER_DOWNGRADE_NEEDS_REVIEW`: a move to a lower tier has no approver.
pub const TIER_DOWNGRADE_NEEDS_REVIEW: RefusalCode = RefusalCode::new("TIER_DOWNGRADE_NEEDS_REVIEW");
/// `ALWAYS_ON_TOOL_REMOVED`: a tenant policy removes an always-on capability.
pub const ALWAYS_ON_TOOL_REMOVED: RefusalCode = RefusalCode::new("ALWAYS_ON_TOOL_REMOVED");

/// `TIER_SELF_REVIEW`: the reviewer is the author of the revision (the two-person rule,
/// MEDULLA-IDENTITY-R739).
pub const TIER_SELF_REVIEW: RefusalCode = RefusalCode::new("TIER_SELF_REVIEW");
/// `TIER_REVIEW_SCOPE_REQUIRED`: a tiered promotion needs the scope [`SCOPE_REVIEW`].
pub const TIER_REVIEW_SCOPE_REQUIRED: RefusalCode = RefusalCode::new("TIER_REVIEW_SCOPE_REQUIRED");
/// The one engine scope that reviews a skill promotion; platform or team review is told apart
/// by the reviewer's role in the graph, not by a second scope (MEDULLA-IDENTITY-R739).
pub const SCOPE_REVIEW: &str = "skills:review";
/// The request parameter the engine fills with the verified reviewer of a tiered promotion; a
/// caller-supplied value is dropped.
pub const PARAM_TIER_REVIEWER: &str = "tier_reviewer";

/// The tier refusal table (MEDULLA-DECISION-ENGINE-R714.5, R715.1).
pub const REFUSALS: &[RefusalCode] = &[
    TIER_SELF_REVIEW,
    TIER_REVIEW_SCOPE_REQUIRED,
    TIER_REVIEW_MISSING,
    ALWAYS_ON_CAPACITY_REACHED,
    PERSONA_UNKNOWN,
    TIER_DOWNGRADE_NEEDS_REVIEW,
    ALWAYS_ON_TOOL_REMOVED,
];

/// How far a skill revision has been reviewed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SkillTier {
    /// Reviewed by the team that owns the skill.
    Domain,
    /// Reviewed by the platform team and tested.
    Core,
    /// A core skill loaded for every job; bounded by `skills.always_on_max`.
    AlwaysOn,
}

impl SkillTier {
    /// The snake_case text used in a premise and in the store.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Domain => "domain",
            Self::Core => "core",
            Self::AlwaysOn => "always_on",
        }
    }

    /// The tier a premise or a stored row names.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "domain" => Some(Self::Domain),
            "core" => Some(Self::Core),
            "always_on" => Some(Self::AlwaysOn),
            _ => None,
        }
    }
}

/// The review receipt of one tier: who approved it for which team.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct TierReview {
    /// The tier the review qualifies the revision for.
    pub tier: SkillTier,
    /// The team that reviewed it.
    pub reviewer_team: String,
    /// The principal that approved it.
    pub approver_principal: String,
    /// The suite report the review read, when it read one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eval_report_ref: Option<String>,
}

/// The facts about the revision and its approver that the tier gate reads beside the review.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TierContext {
    /// The tier the revision holds now, when it holds one.
    pub current: Option<SkillTier>,
    /// Whether the graph shows the approver as a member of `reviewer_team`.
    pub approver_in_team: bool,
    /// Whether the reviewer is the author of the revision; such a review is refused.
    pub reviewer_is_author: bool,
}

/// The knobs the tier gate reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TierKnobs {
    /// `skills.always_on_max`: most always-on skills per tenant.
    pub always_on_max: u64,
    /// `eval.min_cases`: fewest cases a suite report needs to count.
    pub min_cases: u64,
}

/// One skill revision of a load set with the tier it holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct TieredSkill {
    /// The skill revision.
    pub skill_ref: String,
    /// Its tier, when it holds one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<SkillTier>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DECISION-ENGINE-R714.1
    #[test]
    fn tier_types_round_trip() {
        for tier in [SkillTier::Domain, SkillTier::Core, SkillTier::AlwaysOn] {
            assert_eq!(SkillTier::parse(tier.as_str()), Some(tier));
            let text = serde_json::to_string(&tier).unwrap();
            assert_eq!(text, format!("\"{}\"", tier.as_str()));
            assert_eq!(serde_json::from_str::<SkillTier>(&text).unwrap(), tier);
        }
        assert!(SkillTier::Domain < SkillTier::Core && SkillTier::Core < SkillTier::AlwaysOn);
        assert_eq!(SkillTier::parse("platform"), None);
        let review = TierReview {
            tier: SkillTier::Core,
            reviewer_team: "platform".into(),
            approver_principal: "principal:ana".into(),
            eval_report_ref: None,
        };
        let value = serde_json::to_value(&review).unwrap();
        assert!(value.get("eval_report_ref").is_none());
        assert_eq!(serde_json::from_value::<TierReview>(value).unwrap(), review);
        // A field outside the type is refused.
        assert!(serde_json::from_value::<TierReview>(serde_json::json!({
            "tier": "core", "reviewer_team": "t", "approver_principal": "p", "extra": 1
        }))
        .is_err());
    }
}
