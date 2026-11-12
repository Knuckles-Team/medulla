use std::collections::BTreeSet;

use medulla_enrich::semantic::{store_card, CapabilityCard, CARD_FACT_UNKNOWN, CARD_UNGROUNDED};
use medulla_enrich::Id;

fn id(value: &str) -> Id {
    Id::new(value).unwrap()
}

fn card(facts: &[&str], summary: &str) -> CapabilityCard {
    CapabilityCard {
        id: id("card:1"),
        subject: id("src/lib.rs::main"),
        summary: summary.into(),
        model: id("eg:head/text-7b"),
        prompt_digest: "ab".repeat(32),
        source_fact_ids: facts.iter().map(|f| id(f)).collect(),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R614.2.1
#[test]
fn card_store_accepts_grounded_and_refuses_ungrounded() {
    let known: BTreeSet<Id> = [id("fact:1"), id("fact:2")].into_iter().collect();

    let node = store_card(
        &card(&["fact:1", "fact:2"], "wires parser to store"),
        &known,
    )
    .unwrap();
    assert_eq!(node.properties["summary"], "wires parser to store");
    assert_eq!(node.properties["model"], "eg:head/text-7b");
    assert_eq!(node.properties["prompt_digest"], "ab".repeat(32));
    assert_eq!(node.properties["source_fact_ids"], "fact:1,fact:2");

    // The same subject and prompt digest replace the first card.
    let again = store_card(&card(&["fact:1"], "revised summary"), &known).unwrap();
    assert_eq!(again.id, node.id);
    let mut graph = std::collections::BTreeMap::new();
    graph.insert(node.id.clone(), node);
    graph.insert(again.id.clone(), again);
    assert_eq!(graph.len(), 1);
    assert_eq!(
        graph.values().next().unwrap().properties["summary"],
        "revised summary"
    );

    // A different prompt digest is a different node.
    let mut other = card(&["fact:1"], "other prompt");
    other.prompt_digest = "cd".repeat(32);
    assert_ne!(store_card(&other, &known).unwrap().id, again_id(&graph));

    assert_eq!(
        store_card(&card(&[], "no citation"), &known)
            .unwrap_err()
            .code,
        CARD_UNGROUNDED
    );
    assert_eq!(
        store_card(&card(&["fact:1", "fact:9"], "bad citation"), &known)
            .unwrap_err()
            .code,
        CARD_FACT_UNKNOWN
    );
}

fn again_id(graph: &std::collections::BTreeMap<Id, medulla_enrich::semantic::CardNode>) -> Id {
    graph.keys().next().unwrap().clone()
}
