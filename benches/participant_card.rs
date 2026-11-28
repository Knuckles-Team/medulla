//! The participant card lookup over 10,000 stored cards (MEDULLA-DECISION-ENGINE-R655.6).
//!
//! Nightly bench group `decide/participant_card`: `read_card`, the lookup behind
//! `ParticipantRead` `Card`, over one graph core holding 10,000 cards of one tenant. Each of
//! 10,000 samples reads one card by subject and observer. Budget: p99 below 1 ms. The run exits
//! non-zero when the budget is exceeded. A service level, not a pull request gate.
//!
// spec: MEDULLA-DECISION-ENGINE-R655.6
//!
//! Run: cargo bench --bench participant_card

use std::time::{Duration, Instant};

use medulla_decision_statistical_types::claims::ObserverRef;
use medulla_decision_types::participant_memory::{CardClaim, ParticipantCard};
use medulla_graph_core::graph::GraphCore;
use medulla_h_graph::graph_ops::participant_card::{card_node_id, read_card, CARD_LABEL};
use medulla_work_types::participant::ParticipantId;

const TENANT: &str = "bench";
const CARDS: usize = 10_000;
const BUDGET_P99: Duration = Duration::from_millis(1);

fn subject(index: usize) -> ParticipantId {
    ParticipantId::new(format!("p-{index:05}")).expect("a valid slug")
}

fn main() {
    let core = GraphCore::new();
    for index in 0..CARDS {
        let card = ParticipantCard {
            subject: subject(index),
            observer: ObserverRef::System,
            claims: vec![CardClaim {
                claim_id: format!("claim-{index:05}"),
                text: "Prefers green tea".to_string(),
                confidence_ppm: 700_000,
                recency_ppm: 900_000,
            }],
            token_estimate: 5,
            computed_at_ms: 1,
            digest: format!("sha256:{index:064x}"),
        };
        let properties = serde_json::json!({
            "type": CARD_LABEL,
            "tenant_scope": TENANT,
            "json": serde_json::to_string(&card).expect("bench card"),
        });
        core.add_node(
            card_node_id(TENANT, &card.subject, &card.observer),
            rmp_serde::to_vec_named(&properties).expect("bench node"),
        );
    }
    let mut samples = Vec::with_capacity(CARDS);
    for index in 0..CARDS {
        // A stride that visits every card once, out of order.
        let at = (index * 7_919) % CARDS;
        let started = Instant::now();
        let card = read_card(&core, TENANT, &subject(at), &ObserverRef::System);
        samples.push(started.elapsed());
        assert!(card.is_some(), "card {at} is stored");
    }
    samples.sort();
    let p99 = samples[(samples.len() * 99 / 100).min(samples.len() - 1)];
    println!("decide/participant_card p99 = {p99:?} (budget {BUDGET_P99:?})");
    assert!(p99 < BUDGET_P99, "p99 {p99:?} exceeds {BUDGET_P99:?}");
}
