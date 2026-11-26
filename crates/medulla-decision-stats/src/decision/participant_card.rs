//! The participant card: the claims worth keeping for one subject as one observer sees them,
//! chosen inside a token budget (MEDULLA-DECISION-ENGINE-R655.3).
//!
//! Pure numerics: the same inputs give the same card, and a digest of the card tells a caller
//! whether it changed. Nothing here reads a store or a clock; the caller passes `now_ms`.

use sha2::{Digest, Sha256};

use medulla_decision_types::participant_memory::CardClaim;

/// The smallest token budget a card may have; a smaller configured budget is raised to it.
pub const CARD_TOKEN_FLOOR: u32 = 64;

const PPM: u64 = 1_000_000;

/// One claim offered to the card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardInput {
    pub claim_id: String,
    /// The sentence the claim states.
    pub text: String,
    /// Signed log-odds in micro-nats over the claim's evidence.
    pub log_odds_micro: i64,
    /// When the claim last changed.
    pub updated_ms: i64,
    pub token_estimate: u32,
}

/// What bounds a card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CardBudget {
    /// The token budget; raised to [`CARD_TOKEN_FLOOR`] when smaller.
    pub tokens: u32,
    /// The age at which a claim's recency halves (`participant.card_recency_half_life`).
    pub half_life_ms: u64,
    /// The instant the card is computed at.
    pub now_ms: i64,
}

/// The tokens a sentence costs: one per four characters, at least one.
pub fn token_estimate(text: &str) -> u32 {
    let chars = u32::try_from(text.chars().count()).unwrap_or(u32::MAX);
    (chars.saturating_add(3) / 4).max(1)
}

/// The probability, in parts per million, that log-odds of `log_odds_micro` micro-nats stand for.
pub fn confidence_ppm(log_odds_micro: i64) -> u32 {
    let nats = (log_odds_micro as f64 / 1_000_000.0).clamp(-20.0, 20.0);
    let probability = 1.0 / (1.0 + (-nats).exp());
    (probability * PPM as f64).round() as u32
}

/// `0.5` to the power of age over the half-life, in parts per million; a claim from the future
/// counts as new.
pub fn recency_ppm(age_ms: i64, half_life_ms: u64) -> u32 {
    if age_ms <= 0 {
        return PPM as u32;
    }
    let halves = age_ms as f64 / half_life_ms.max(1) as f64;
    (0.5_f64.powf(halves) * PPM as f64).round() as u32
}

/// The claims of the card, in rank order. The rank is `confidence_ppm` times `recency_ppm`
/// divided by one million; a tie breaks by the lowest `claim_id`. Claims are added while the sum
/// of their `token_estimate` stays within the budget, and the first one that does not fit ends the
/// card. Cost is O(n log n).
pub fn rank_card(claims: &[CardInput], budget: CardBudget) -> Vec<CardClaim> {
    let limit = u64::from(budget.tokens.max(CARD_TOKEN_FLOOR));
    let mut scored: Vec<(u64, &CardInput, u32, u32)> = claims
        .iter()
        .map(|claim| {
            let confidence = confidence_ppm(claim.log_odds_micro);
            let recency = recency_ppm(budget.now_ms.saturating_sub(claim.updated_ms), budget.half_life_ms);
            (u64::from(confidence) * u64::from(recency) / PPM, claim, confidence, recency)
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.claim_id.cmp(&b.1.claim_id)));
    let mut spent = 0_u64;
    let mut card = Vec::new();
    for (_, claim, confidence_ppm, recency_ppm) in scored {
        spent += u64::from(claim.token_estimate);
        if spent > limit {
            break;
        }
        card.push(CardClaim {
            claim_id: claim.claim_id.clone(),
            text: claim.text.clone(),
            confidence_ppm,
            recency_ppm,
        });
    }
    card
}

/// The digest of a card's claims, as `sha256:<hex>`. Two cards with the same claims, ranks and
/// text have the same digest, so an unchanged card announces nothing.
pub fn card_digest(claims: &[CardClaim]) -> String {
    let mut hasher = Sha256::new();
    for claim in claims {
        for part in [claim.claim_id.as_str(), claim.text.as_str()] {
            hasher.update((part.len() as u64).to_be_bytes());
            hasher.update(part.as_bytes());
        }
        hasher.update(claim.confidence_ppm.to_be_bytes());
        hasher.update(claim.recency_ppm.to_be_bytes());
    }
    format!("sha256:{}", hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 86_400_000;

    fn input(id: &str, text: &str, log_odds: i64, updated_ms: i64) -> CardInput {
        CardInput {
            claim_id: id.into(),
            text: text.into(),
            log_odds_micro: log_odds,
            updated_ms,
            token_estimate: token_estimate(text),
        }
    }

    fn budget(tokens: u32) -> CardBudget {
        CardBudget { tokens, half_life_ms: 30 * DAY, now_ms: 100 * DAY as i64 }
    }

    // spec: MEDULLA-DECISION-ENGINE-R655.3
    #[test]
    fn card_respects_budget_and_floor() {
        let claims = [
            input("c-strong", &"x".repeat(100), 3_000_000, 100 * DAY as i64),
            input("c-weak", &"y".repeat(100), 500_000, 100 * DAY as i64),
            input("c-old", &"z".repeat(100), 3_000_000, 10 * DAY as i64),
        ];
        // 100 characters cost 25 tokens each. The floor is 64, so a budget below it still holds
        // two claims (50 tokens) but not a third (75).
        let tiny = rank_card(&claims, budget(1));
        assert_eq!(tiny.len(), 2);
        assert_eq!(tiny[0].claim_id, "c-strong");
        // Rank orders by confidence times recency: the old strong claim ranks below the fresh weak one.
        let wide = rank_card(&claims, budget(4_000));
        assert_eq!(wide.iter().map(|c| c.claim_id.as_str()).collect::<Vec<_>>(), ["c-strong", "c-weak", "c-old"]);
        // A budget that cannot hold the first claim gives an empty card, never a larger one.
        assert!(rank_card(&[input("big", &"w".repeat(400), 0, 100 * DAY as i64)], budget(64)).is_empty());
        // A tie breaks by the lowest claim id.
        let tie = rank_card(
            &[input("b", "same", 1_000_000, 100 * DAY as i64), input("a", "same", 1_000_000, 100 * DAY as i64)],
            budget(400),
        );
        assert_eq!(tie.iter().map(|c| c.claim_id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
    }

    // spec: MEDULLA-DECISION-ENGINE-R655.3
    #[test]
    fn confidence_and_recency_follow_their_formulas() {
        assert_eq!(confidence_ppm(0), 500_000);
        assert!(confidence_ppm(3_000_000) > 940_000);
        assert!(confidence_ppm(-3_000_000) < 60_000);
        assert_eq!(recency_ppm(0, 30 * DAY), 1_000_000);
        assert_eq!(recency_ppm(30 * DAY as i64, 30 * DAY), 500_000);
        assert_eq!(recency_ppm(-5, 30 * DAY), 1_000_000);
        assert_eq!(token_estimate(""), 1);
        assert_eq!(token_estimate("abcde"), 2);
    }

    // spec: MEDULLA-DECISION-ENGINE-R655.3
    #[test]
    fn the_digest_changes_only_when_the_card_changes() {
        let claims = [input("c1", "prefers tea", 1_000_000, 100 * DAY as i64)];
        let card = rank_card(&claims, budget(400));
        assert_eq!(card_digest(&card), card_digest(&rank_card(&claims, budget(400))));
        assert!(card_digest(&card).starts_with("sha256:"));
        let changed = rank_card(&[input("c1", "prefers coffee", 1_000_000, 100 * DAY as i64)], budget(400));
        assert_ne!(card_digest(&card), card_digest(&changed));
        assert_eq!(card_digest(&[]), card_digest(&[]));
    }
}
