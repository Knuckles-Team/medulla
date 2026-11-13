use medulla_program::{ParetoFront, PolicyCandidate};

fn candidate(id: &str, scores: [f32; 2], cost_micro_usd: u64) -> PolicyCandidate<2> {
    PolicyCandidate {
        id: id.to_owned(),
        parent: None,
        genes_digest: "a".repeat(64),
        scores,
        cost_micro_usd,
    }
}

#[test]
// spec: MEDULLA-DECISION-ENGINE-R800.1
fn dominated_candidate_is_dropped() {
    let mut front = ParetoFront::<2>::default();
    assert!(front
        .insert(candidate("efficient", [0.9, 0.8], 100))
        .unwrap());
    assert!(!front
        .insert(candidate("dominated", [0.8, 0.7], 200))
        .unwrap());
    assert_eq!(front.candidates().len(), 1);
    assert_eq!(front.candidates()[0].id, "efficient");
}

#[test]
// spec: MEDULLA-DECISION-ENGINE-R800.1
fn scores_outside_unit_interval_are_refused() {
    let mut front = ParetoFront::<2>::default();
    let result = front.insert(candidate("invalid", [1.01, 0.5], 100));
    assert_eq!(result, Err(medulla_program::FrontError::ScoreOutOfBounds));
    assert!(front.candidates().is_empty());
}

#[test]
// spec: MEDULLA-DECISION-ENGINE-R800.1
fn deserialization_rejects_invalid_candidates_and_dominated_fronts() {
    let invalid_candidate = r#"{
        "id":"invalid","parent":null,"genes_digest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "scores":[1.01,0.5],"cost_micro_usd":100
    }"#;
    assert!(serde_json::from_str::<PolicyCandidate<2>>(invalid_candidate).is_err());

    let dominated_front = r#"{"candidates":[
        {"id":"efficient","parent":null,"genes_digest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","scores":[0.9,0.8],"cost_micro_usd":100},
        {"id":"dominated","parent":null,"genes_digest":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","scores":[0.8,0.7],"cost_micro_usd":200}
    ]}"#;
    assert!(serde_json::from_str::<ParetoFront<2>>(dominated_front).is_err());
}

fn spread_front() -> ParetoFront<2> {
    let mut front = ParetoFront::<2>::default();
    for (id, scores, cost) in [
        ("a", [0.9, 0.1], 900u64),
        ("b", [0.7, 0.4], 500),
        ("c", [0.6, 0.45], 400),
        ("d", [0.3, 0.9], 100),
    ] {
        assert!(front.insert(candidate(id, scores, cost)).unwrap());
    }
    front
}

#[test]
// spec: MEDULLA-DECISION-ENGINE-R800.2
fn same_seed_reproduces_front_digest() {
    let front = spread_front();
    let digest = front.digest();
    assert_eq!(digest.len(), 64);
    assert!(digest
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    // Insertion order never changes the digest.
    let mut reversed = ParetoFront::<2>::default();
    for item in front.candidates().iter().rev() {
        reversed.insert(item.clone()).unwrap();
    }
    assert_eq!(reversed.digest(), digest);
    // One seed, one parent; the parent is a member of the front.
    for seed in [0u64, 1, 7, 42, u64::MAX] {
        let first = front.next_parent(seed).expect("parent").id.clone();
        assert_eq!(front.next_parent(seed).expect("parent").id, first);
        assert!(front.candidates().iter().any(|c| c.id == first));
    }
    assert!(ParetoFront::<2>::default().next_parent(3).is_none());
    // Boundary candidates are infinitely far; the inner ones are finite.
    let distance = front.crowding_distances();
    let by_id = |id: &str| {
        front
            .candidates()
            .iter()
            .position(|c| c.id == id)
            .map(|i| distance[i])
            .unwrap()
    };
    assert!(by_id("a").is_infinite() && by_id("d").is_infinite());
    assert!(by_id("b").is_finite() && by_id("c").is_finite());
    // The bound drops the smallest crowding distance first.
    let mut bounded = front.clone();
    bounded.bound(3);
    assert_eq!(bounded.candidates().len(), 3);
    assert!(bounded.candidates().iter().any(|c| c.id == "a"));
    assert!(bounded.candidates().iter().any(|c| c.id == "d"));
    // A changed front changes the digest.
    let mut other = front.clone();
    other.bound(3);
    assert_ne!(other.digest(), digest);
}

#[test]
// spec: MEDULLA-DECISION-ENGINE-R800.2
fn scoreboard_ranks_accuracy_per_cost() {
    let front = spread_front();
    let ids: Vec<&str> = front.scoreboard().iter().map(|c| c.id.as_str()).collect();
    // 0.3/100, 0.6/400, 0.7/500, 0.9/900 per micro-USD.
    assert_eq!(ids, ["d", "c", "b", "a"]);
    let mut tied = ParetoFront::<2>::default();
    tied.insert(candidate("y", [0.5, 0.2], 100)).unwrap();
    tied.insert(candidate("x", [0.5, 0.2], 100)).unwrap();
    let ids: Vec<&str> = tied.scoreboard().iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["x", "y"]);
}
