//! Canonical process form (MEDULLA-CQ-ORCH-R701.2 and the stages of R701.11).

use medulla_core::process_import::{admit_canonical, canonicalize, lower_rules, normalize_text};
use medulla_types::process_import::{
    AnnexEntry, GateKind, ImportError, LoweredFlow, LoweredStep, ProcessSourceSystem, StepKind,
};

fn step(id: &str, kind: StepKind, gate: Option<GateKind>, label: &str) -> LoweredStep {
    LoweredStep {
        element_id: id.into(),
        kind,
        gate,
        label: label.into(),
        role: None,
        reference: None,
    }
}

fn flow(source: &str, target: &str) -> LoweredFlow {
    LoweredFlow {
        source: source.into(),
        target: target.into(),
        condition: None,
        default: false,
    }
}

/// A process of `n` steps: a chain with a skip edge every third step and one
/// back edge, so Kahn order and the cycle rule are both exercised.
fn process(n: usize) -> (Vec<LoweredStep>, Vec<LoweredFlow>) {
    let kinds = [
        StepKind::Human,
        StepKind::Agent,
        StepKind::Decision,
        StepKind::Wait,
    ];
    let mut steps = vec![step("e0", StepKind::Start, None, "Start")];
    for i in 1..n - 1 {
        steps.push(step(
            &format!("e{i}"),
            kinds[i % kinds.len()],
            None,
            &format!("Task {}", i % 7),
        ));
    }
    steps.push(step(&format!("e{}", n - 1), StepKind::End, None, "End"));
    let mut flows = Vec::new();
    for i in 0..n - 1 {
        flows.push(flow(&format!("e{i}"), &format!("e{}", i + 1)));
        if i % 3 == 0 && i + 2 < n {
            flows.push(flow(&format!("e{i}"), &format!("e{}", i + 2)));
        }
    }
    if n > 4 {
        flows.push(flow(&format!("e{}", n - 2), "e1"));
    }
    (steps, flows)
}

/// A deterministic permutation (linear congruential shuffle).
fn shuffle<T>(items: &mut [T], seed: u64) {
    let mut state = seed;
    for i in (1..items.len()).rev() {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        items.swap(i, (state >> 33) as usize % (i + 1));
    }
}

// spec: MEDULLA-CQ-ORCH-R701.2
#[test]
fn canonical_ids_and_digest_are_stable() {
    for n in [10usize, 1_000, 100_000] {
        let (steps, flows) = process(n);
        let first = canonicalize(
            "P",
            ProcessSourceSystem::Bpmn20,
            "m",
            steps.clone(),
            flows.clone(),
        )
        .unwrap();
        assert_eq!(first.digest.len(), 64);
        assert!(first
            .digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        assert_eq!(first.steps[0].id, "s1");
        assert_eq!(first.steps.len(), n);
        let (mut s2, mut f2) = (steps.clone(), flows.clone());
        shuffle(&mut s2, n as u64);
        shuffle(&mut f2, n as u64 + 1);
        let second = canonicalize("P", ProcessSourceSystem::ArisEpc, "other", s2, f2).unwrap();
        assert_eq!(second.steps, first.steps, "n={n}");
        assert_eq!(second.flows, first.flows, "n={n}");
        assert_eq!(second.digest, first.digest, "n={n}");
        if n == 10 {
            let mut relabelled = steps.clone();
            relabelled[3].label = "Changed".into();
            let changed = canonicalize(
                "P",
                ProcessSourceSystem::Bpmn20,
                "m",
                relabelled,
                flows.clone(),
            )
            .unwrap();
            assert_ne!(changed.digest, first.digest);
            let mut annexed = first.clone();
            annexed.annex.push(AnnexEntry {
                schema_version: 1,
                after: Some("s2".into()),
                before: None,
                construct: "epc-state-event".into(),
                element_id: "x".into(),
                label: "noted".into(),
                type_num: None,
            });
            assert_eq!(admit_canonical(&annexed).unwrap().digest, first.digest);
        }
    }
}

// spec: MEDULLA-CQ-ORCH-R701.2
#[test]
fn admission_refuses_a_non_canonical_or_misdigested_document() {
    let (steps, flows) = process(10);
    let workflow = canonicalize("P", ProcessSourceSystem::Bpmn20, "m", steps, flows).unwrap();
    assert_eq!(admit_canonical(&workflow).unwrap(), workflow);

    let mut wrong_digest = workflow.clone();
    wrong_digest.digest = "0".repeat(64);
    assert!(matches!(
        admit_canonical(&wrong_digest),
        Err(ImportError::DigestMismatch { .. })
    ));

    let mut reordered = workflow.clone();
    reordered.flows.reverse();
    assert!(matches!(
        admit_canonical(&reordered),
        Err(ImportError::NotCanonical { .. })
    ));
}

// spec: MEDULLA-CQ-ORCH-R701.2
#[test]
fn text_is_nfc_and_degree_gives_split_and_join() {
    assert_eq!(
        normalize_text("  Nai\u{308}ve\n  review\t"),
        "Na\u{ef}ve review"
    );
    let steps = vec![
        step("a", StepKind::Start, None, "a"),
        step("g", StepKind::Branch, Some(GateKind::Xor), "g"),
        step("b", StepKind::Human, None, "b"),
        step("c", StepKind::Human, None, "c"),
        step("d", StepKind::End, None, "d"),
    ];
    let flows = vec![
        flow("a", "g"),
        flow("b", "g"),
        flow("g", "c"),
        flow("g", "d"),
    ];
    let (lowered, lowered_flows) = lower_rules(steps, flows);
    let kinds: Vec<_> = lowered
        .iter()
        .map(|s| (s.element_id.as_str(), s.kind))
        .collect();
    assert!(kinds.contains(&("g", StepKind::Join)));
    assert!(kinds.contains(&("g#split", StepKind::Branch)));
    assert!(lowered_flows
        .iter()
        .any(|f| f.source == "g" && f.target == "g#split"));
    assert!(
        lowered_flows
            .iter()
            .filter(|f| f.source == "g#split")
            .count()
            == 2
    );
}
