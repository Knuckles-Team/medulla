//! The first `Decide` stage over the work market: derive the LEGAL offers,
//! then rank them by the deterministic utility rate.
//!
//! This is a pure function of the Gaps and the caller's resource facts. It is
//! not a wire method and must not become one: design §3.1 forbids a
//! `SelectNextOffer` shortcut. `Decide` calls it to build its candidate set,
//! records the ranking in a `DecisionRecord`, and only a committed decision
//! reaches the native `ClaimWorkItem` fence. Nothing here leases, claims or
//! persists anything.
//!
//! Legality is a table of rules, each naming the exclusion it produces, so a
//! refused offer always says why. Ties order by the WorkItem priority bucket,
//! then the oldest Gap, then the WorkItem id -- a total order, so the ranking
//! reproduces exactly.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::gap::GapView;
use super::offer::{work_offer_utility_rate, BlastRadius, WorkOfferView};

/// The resource and policy facts one selection runs under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct MarketFacts {
    pub now_ms: u64,
    /// Canonical ids of Gaps already resolved (dependency satisfaction).
    pub resolved_gap_ids: BTreeSet<String>,
    pub available_capabilities: BTreeSet<String>,
    /// Repositories an attempt may touch; `None` places no restriction.
    pub repository_scope: Option<BTreeSet<String>>,
    pub max_blast_radius: BlastRadius,
    /// The largest single-attempt cost the budgets can admit.
    pub attempt_budget_microunits: u64,
    pub cost_floor_microunits: u64,
}

/// Why an offer is not a legal candidate. The wire name is the code
/// ([`OfferExclusion::code`]) an excluded Gap carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum OfferExclusion {
    /// The Gap is resolved or deferred.
    NotLive,
    /// The Gap carries no offer for its current WorkItem.
    Unpriced,
    DependencyOpen,
    CoolingDown,
    CapabilityUnavailable,
    RepositoryOutOfScope,
    BlastRadiusExceeded,
    OverBudget,
    /// The rate at the policy's cost floor does not fit the native range.
    RateOutOfRange,
}

impl OfferExclusion {
    /// The closed code of the exclusion, equal to its wire name.
    pub const fn code(self) -> &'static str {
        match self {
            Self::NotLive => "NOT_LIVE",
            Self::Unpriced => "UNPRICED",
            Self::DependencyOpen => "DEPENDENCY_OPEN",
            Self::CoolingDown => "COOLING_DOWN",
            Self::CapabilityUnavailable => "CAPABILITY_UNAVAILABLE",
            Self::RepositoryOutOfScope => "REPOSITORY_OUT_OF_SCOPE",
            Self::BlastRadiusExceeded => "BLAST_RADIUS_EXCEEDED",
            Self::OverBudget => "OVER_BUDGET",
            Self::RateOutOfRange => "RATE_OUT_OF_RANGE",
        }
    }
}

/// One legal offer, ranked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RankedOffer {
    pub gap_id: String,
    pub work_item_id: String,
    pub utility_rate: u64,
    pub priority_bucket: u8,
    pub created_at_ms: u64,
}

/// A selection: the legal offers best first, and every excluded Gap with its
/// reason.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct OfferSelection {
    pub ranked: Vec<RankedOffer>,
    pub excluded: Vec<(String, OfferExclusion)>,
}

type LegalityRule = fn(&WorkOfferView, &MarketFacts) -> bool;

fn dependencies_resolved(offer: &WorkOfferView, facts: &MarketFacts) -> bool {
    offer
        .offer
        .depends_on_gap_ids
        .iter()
        .all(|gap_id| facts.resolved_gap_ids.contains(gap_id))
}

fn cooled_down(offer: &WorkOfferView, facts: &MarketFacts) -> bool {
    offer.offer.cooldown_until_ms <= facts.now_ms
}

fn capabilities_available(offer: &WorkOfferView, facts: &MarketFacts) -> bool {
    offer
        .offer
        .required_capabilities
        .iter()
        .all(|capability| facts.available_capabilities.contains(capability))
}

fn repositories_in_scope(offer: &WorkOfferView, facts: &MarketFacts) -> bool {
    facts.repository_scope.as_ref().is_none_or(|scope| {
        offer
            .offer
            .repository_scope
            .iter()
            .all(|repository| scope.contains(repository))
    })
}

fn blast_radius_admitted(offer: &WorkOfferView, facts: &MarketFacts) -> bool {
    offer.offer.blast_radius <= facts.max_blast_radius
}

fn within_budget(offer: &WorkOfferView, facts: &MarketFacts) -> bool {
    offer.offer.expected_cost_microunits <= facts.attempt_budget_microunits
}

/// Every legality rule, in the order a refusal is reported.
const LEGALITY_RULES: [(LegalityRule, OfferExclusion); 6] = [
    (dependencies_resolved, OfferExclusion::DependencyOpen),
    (cooled_down, OfferExclusion::CoolingDown),
    (
        capabilities_available,
        OfferExclusion::CapabilityUnavailable,
    ),
    (repositories_in_scope, OfferExclusion::RepositoryOutOfScope),
    (blast_radius_admitted, OfferExclusion::BlastRadiusExceeded),
    (within_budget, OfferExclusion::OverBudget),
];

/// The current-generation offer of a live Gap, or why it has none.
fn live_offer(gap: &GapView) -> Result<&WorkOfferView, OfferExclusion> {
    if !gap.status.is_live() {
        return Err(OfferExclusion::NotLive);
    }
    gap.offer
        .as_ref()
        .filter(|offer| offer.work_item_id == gap.work_item_id)
        .ok_or(OfferExclusion::Unpriced)
}

/// Rank one Gap, or say why it is not a legal candidate.
fn rank(gap: &GapView, facts: &MarketFacts) -> Result<RankedOffer, OfferExclusion> {
    let offer = live_offer(gap)?;
    if let Some((_, exclusion)) = LEGALITY_RULES.iter().find(|(rule, _)| !rule(offer, facts)) {
        return Err(*exclusion);
    }
    let utility_rate = work_offer_utility_rate(&offer.offer, facts.cost_floor_microunits)
        .ok_or(OfferExclusion::RateOutOfRange)?;
    Ok(RankedOffer {
        gap_id: gap.gap_id.clone(),
        work_item_id: gap.work_item_id.clone(),
        utility_rate,
        priority_bucket: gap.priority_bucket,
        created_at_ms: gap.created_at_ms,
    })
}

/// Derive the legal candidate set over `gaps` and rank it: highest rate
/// first; ties by priority bucket, then oldest Gap, then WorkItem id.
pub fn derive_legal_offers(gaps: &[GapView], facts: &MarketFacts) -> OfferSelection {
    let mut selection = OfferSelection::default();
    for gap in gaps {
        match rank(gap, facts) {
            Ok(ranked) => selection.ranked.push(ranked),
            Err(exclusion) => selection.excluded.push((gap.gap_id.clone(), exclusion)),
        }
    }
    selection.ranked.sort_by(|a, b| {
        b.utility_rate
            .cmp(&a.utility_rate)
            .then(a.priority_bucket.cmp(&b.priority_bucket))
            .then(a.created_at_ms.cmp(&b.created_at_ms))
            .then_with(|| a.work_item_id.cmp(&b.work_item_id))
    });
    selection
}

/// The market allocation (MEDULLA-ORCH-VOCAB-R750.751): the legal offers of
/// [`derive_legal_offers`] in their [`RankedOffer`] order, cut to the top
/// one. `ranked` holds at most that offer, with its `priority_bucket`;
/// `excluded` keeps every excluded Gap with its [`OfferExclusion`], and no
/// allocation is ever made from an excluded offer. Names no new ranker.
pub fn allocate_offer(gaps: &[GapView], facts: &MarketFacts) -> OfferSelection {
    let mut selection = derive_legal_offers(gaps, facts);
    selection.ranked.truncate(1);
    selection
}

impl OfferSelection {
    /// The allocated offer, when one is legal.
    pub fn top(&self) -> Option<&RankedOffer> {
        self.ranked.first()
    }
}
