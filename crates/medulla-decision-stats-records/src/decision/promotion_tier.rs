//! The tier gate of a skill promotion (MEDULLA-DECISION-ENGINE-R714.2, R714.5).
//!
//! Pure: no I/O and no clock. The served `Promotion` family reads the review, the suite report,
//! the always-on count and the approver's team membership, then asks [`tier_gate`] whether the
//! tier may be granted.

use medulla_decision_statistical_types::decision::codes::PROMOTION_EVAL_TOO_SMALL;
use medulla_types::refusal_code::RefusalCode;
use {
    medulla_decision_policy_types::skill_eval::SuiteBand,
    medulla_decision_policy_types::skill_eval::SuiteReport,
};
use {
    medulla_decision_statistical_types::decision::promotion_tier::SkillTier,
    medulla_decision_statistical_types::decision::promotion_tier::TierContext,
    medulla_decision_statistical_types::decision::promotion_tier::TierKnobs,
    medulla_decision_statistical_types::decision::promotion_tier::TierReview,
    medulla_decision_statistical_types::decision::promotion_tier::ALWAYS_ON_CAPACITY_REACHED,
    medulla_decision_statistical_types::decision::promotion_tier::PLATFORM_TEAM,
    medulla_decision_statistical_types::decision::promotion_tier::TIER_DOWNGRADE_NEEDS_REVIEW,
    medulla_decision_statistical_types::decision::promotion_tier::TIER_REVIEW_MISSING,
    medulla_decision_statistical_types::decision::promotion_tier::TIER_SELF_REVIEW,
};

/// Whether a revision may take `tier`.
///
/// * A review by the author of the revision is refused first (`TIER_SELF_REVIEW`).
/// * A move to a lower tier than `ctx.current` needs an approver and nothing else.
/// * `Domain` needs a review whose approver sits in the reviewer team.
/// * `Core` needs the same review from the platform team and a suite report that is `Healthy`
///   with at least `knobs.min_cases` cases.
/// * `AlwaysOn` needs everything `Core` needs, a revision that already holds `Core`, and an
///   always-on count below `knobs.always_on_max`.
pub fn tier_gate(
    tier: SkillTier,
    review: Option<&TierReview>,
    report: Option<&SuiteReport>,
    always_on_count: u64,
    ctx: &TierContext,
    knobs: &TierKnobs,
) -> Result<(), RefusalCode> {
    if review.is_some() && ctx.reviewer_is_author {
        return Err(TIER_SELF_REVIEW);
    }
    if ctx.current.is_some_and(|current| current > tier) {
        let approved = review.is_some_and(|review| {
            review.tier == tier && !review.approver_principal.is_empty() && ctx.approver_in_team
        });
        return if approved {
            Ok(())
        } else {
            Err(TIER_DOWNGRADE_NEEDS_REVIEW)
        };
    }
    let review = review
        .filter(|review| review.tier == tier)
        .filter(|review| !review.reviewer_team.is_empty() && !review.approver_principal.is_empty())
        .ok_or(TIER_REVIEW_MISSING)?;
    if !ctx.approver_in_team {
        return Err(TIER_REVIEW_MISSING);
    }
    if tier == SkillTier::Domain {
        return Ok(());
    }
    if review.reviewer_team != PLATFORM_TEAM {
        return Err(TIER_REVIEW_MISSING);
    }
    let report = report.ok_or(TIER_REVIEW_MISSING)?;
    if report.band != SuiteBand::Healthy {
        return Err(TIER_REVIEW_MISSING);
    }
    if report.case_count < knobs.min_cases {
        return Err(PROMOTION_EVAL_TOO_SMALL);
    }
    if tier == SkillTier::AlwaysOn {
        if ctx.current < Some(SkillTier::Core) {
            return Err(TIER_REVIEW_MISSING);
        }
        if always_on_count >= knobs.always_on_max {
            return Err(ALWAYS_ON_CAPACITY_REACHED);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KNOBS: TierKnobs = TierKnobs {
        always_on_max: 100,
        min_cases: 20,
    };

    fn review(tier: SkillTier, team: &str) -> TierReview {
        TierReview {
            tier,
            reviewer_team: team.into(),
            approver_principal: "principal:ana".into(),
            eval_report_ref: None,
        }
    }

    fn report(band: SuiteBand, cases: u64) -> SuiteReport {
        SuiteReport {
            golden_ppm: 50_000,
            real_ppm: 800_000,
            judged_ppm: 150_000,
            pass_rate_ppm: 800_000,
            band,
            case_count: cases,
        }
    }

    fn ctx(current: Option<SkillTier>, approver_in_team: bool) -> TierContext {
        TierContext {
            current,
            approver_in_team,
            reviewer_is_author: false,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R714.2
    #[test]
    fn each_tier_needs_its_review() {
        let healthy = report(SuiteBand::Healthy, 40);
        let in_team = ctx(Some(SkillTier::Core), true);
        for (tier, team) in [
            (SkillTier::Domain, "support"),
            (SkillTier::Core, "platform"),
            (SkillTier::AlwaysOn, "platform"),
        ] {
            let ok = review(tier, team);
            assert_eq!(
                tier_gate(tier, Some(&ok), Some(&healthy), 0, &in_team, &KNOBS),
                Ok(()),
                "{tier:?}"
            );
            assert_eq!(
                tier_gate(tier, None, Some(&healthy), 0, &ctx(None, true), &KNOBS),
                Err(TIER_REVIEW_MISSING),
                "{tier:?}"
            );
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R714.2
    #[test]
    fn domain_needs_a_team_approver() {
        let ok = review(SkillTier::Domain, "support");
        assert_eq!(
            tier_gate(
                SkillTier::Domain,
                Some(&ok),
                None,
                0,
                &ctx(None, true),
                &KNOBS
            ),
            Ok(())
        );
        assert_eq!(
            tier_gate(
                SkillTier::Domain,
                Some(&ok),
                None,
                0,
                &ctx(None, false),
                &KNOBS
            ),
            Err(TIER_REVIEW_MISSING)
        );
        assert_eq!(
            tier_gate(SkillTier::Domain, None, None, 0, &ctx(None, true), &KNOBS),
            Err(TIER_REVIEW_MISSING)
        );
        // A review of another tier is no receipt for this one.
        let other = review(SkillTier::Core, "platform");
        assert_eq!(
            tier_gate(
                SkillTier::Domain,
                Some(&other),
                None,
                0,
                &ctx(None, true),
                &KNOBS
            ),
            Err(TIER_REVIEW_MISSING)
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R714.2
    #[test]
    fn core_needs_the_platform_team_and_a_healthy_report() {
        let ok = review(SkillTier::Core, "platform");
        let healthy = report(SuiteBand::Healthy, 40);
        let go = |review: &TierReview, report: Option<&SuiteReport>| {
            tier_gate(
                SkillTier::Core,
                Some(review),
                report,
                0,
                &ctx(Some(SkillTier::Domain), true),
                &KNOBS,
            )
        };
        assert_eq!(go(&ok, Some(&healthy)), Ok(()));
        assert_eq!(go(&ok, None), Err(TIER_REVIEW_MISSING));
        assert_eq!(
            go(&ok, Some(&report(SuiteBand::Saturated, 40))),
            Err(TIER_REVIEW_MISSING)
        );
        assert_eq!(
            go(&ok, Some(&report(SuiteBand::Healthy, 19))),
            Err(PROMOTION_EVAL_TOO_SMALL)
        );
        assert_eq!(
            go(&review(SkillTier::Core, "support"), Some(&healthy)),
            Err(TIER_REVIEW_MISSING)
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R714.2
    #[test]
    fn always_on_is_bounded_and_needs_core() {
        let ok = review(SkillTier::AlwaysOn, "platform");
        let healthy = report(SuiteBand::Healthy, 40);
        let go = |count: u64, current: Option<SkillTier>| {
            tier_gate(
                SkillTier::AlwaysOn,
                Some(&ok),
                Some(&healthy),
                count,
                &ctx(current, true),
                &KNOBS,
            )
        };
        assert_eq!(go(99, Some(SkillTier::Core)), Ok(()));
        assert_eq!(
            go(100, Some(SkillTier::Core)),
            Err(ALWAYS_ON_CAPACITY_REACHED)
        );
        assert_eq!(go(0, Some(SkillTier::Domain)), Err(TIER_REVIEW_MISSING));
        assert_eq!(go(0, None), Err(TIER_REVIEW_MISSING));
    }

    // spec: MEDULLA-DECISION-ENGINE-R714.5
    #[test]
    fn a_downgrade_needs_an_approver() {
        let down = review(SkillTier::Domain, "support");
        let go = |review: Option<&TierReview>, in_team: bool| {
            tier_gate(
                SkillTier::Domain,
                review,
                None,
                0,
                &ctx(Some(SkillTier::Core), in_team),
                &KNOBS,
            )
        };
        assert_eq!(go(Some(&down), true), Ok(()));
        assert_eq!(go(None, true), Err(TIER_DOWNGRADE_NEEDS_REVIEW));
        assert_eq!(go(Some(&down), false), Err(TIER_DOWNGRADE_NEEDS_REVIEW));
    }

    // spec: MEDULLA-IDENTITY-R739
    #[test]
    fn the_author_never_reviews_its_own_revision() {
        let ok = review(SkillTier::Domain, "support");
        let own = TierContext {
            reviewer_is_author: true,
            ..ctx(None, true)
        };
        assert_eq!(
            tier_gate(SkillTier::Domain, Some(&ok), None, 0, &own, &KNOBS),
            Err(TIER_SELF_REVIEW)
        );
        // A downgrade by the author is refused too.
        let own_down = TierContext {
            reviewer_is_author: true,
            ..ctx(Some(SkillTier::Core), true)
        };
        assert_eq!(
            tier_gate(SkillTier::Domain, Some(&ok), None, 0, &own_down, &KNOBS),
            Err(TIER_SELF_REVIEW)
        );
        // Another reviewer passes.
        assert_eq!(
            tier_gate(
                SkillTier::Domain,
                Some(&ok),
                None,
                0,
                &ctx(None, true),
                &KNOBS
            ),
            Ok(())
        );
    }
}
