//! Process compilation (MEDULLA-CQ-ORCH-R701.2, R701.8 and the stages of R701.11).

use medulla_core::process_import::{
    admit_canonical, allocate, canonicalize, emit, export_canonical, export_canonical_strict,
    lower, normalize_text, relower,
};
use medulla_types::process_import::{
    AllocatedProcess, AnnexEntry, CanonicalWorkflow, GateKind, ImportError, KindAnnotation,
    LoweredFlow, LoweredProcess, LoweredStep, ProcessSourceSystem, SourceGraph, SourceNode,
    StepKind, ANNEX_EPC_ATTACHMENT, ANNEX_EPC_STATE_EVENT,
};

fn canon(
    name: &str,
    system: ProcessSourceSystem,
    model_id: &str,
    steps: Vec<LoweredStep>,
    flows: Vec<LoweredFlow>,
) -> Result<CanonicalWorkflow, ImportError> {
    canonicalize(LoweredProcess {
        name: name.into(),
        system,
        model_id: model_id.into(),
        steps,
        flows,
        annex: Vec::new(),
    })
}

/// Lower a hand-built graph whose classes are the kind names.
fn lower_steps(
    steps: Vec<LoweredStep>,
    flows: Vec<LoweredFlow>,
) -> (Vec<LoweredStep>, Vec<LoweredFlow>) {
    const ANNOTATIONS: &[KindAnnotation] = &[
        KindAnnotation {
            class: "start",
            kind: Some(StepKind::Start),
            gate: None,
        },
        KindAnnotation {
            class: "human",
            kind: Some(StepKind::Human),
            gate: None,
        },
        KindAnnotation {
            class: "xor",
            kind: Some(StepKind::Branch),
            gate: Some(GateKind::Xor),
        },
        KindAnnotation {
            class: "end",
            kind: Some(StepKind::End),
            gate: None,
        },
    ];
    let class = |s: &LoweredStep| match s.kind {
        StepKind::Start => "start",
        StepKind::Human => "human",
        StepKind::End => "end",
        _ => "xor",
    };
    let graph = SourceGraph {
        name: "g".into(),
        system: ProcessSourceSystem::Bpmn20,
        model_id: "m".into(),
        nodes: steps
            .iter()
            .map(|s| SourceNode {
                element_id: s.element_id.clone(),
                class: class(s).into(),
                label: s.label.clone(),
                role: None,
                reference: None,
            })
            .collect(),
        edges: flows,
        annex: Vec::new(),
    };
    let lowered = lower(&graph, ANNOTATIONS).unwrap();
    (lowered.steps, lowered.flows)
}

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
        let first = canon(
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
        let second = canon("P", ProcessSourceSystem::ArisEpc, "other", s2, f2).unwrap();
        assert_eq!(second.steps, first.steps, "n={n}");
        assert_eq!(second.flows, first.flows, "n={n}");
        assert_eq!(second.digest, first.digest, "n={n}");
        if n == 10 {
            let mut relabelled = steps.clone();
            relabelled[3].label = "Changed".into();
            let changed = canon(
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
    let workflow = canon("P", ProcessSourceSystem::Bpmn20, "m", steps, flows).unwrap();
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
    let (lowered, lowered_flows) = lower_steps(steps, flows);
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

// spec: MEDULLA-CQ-ORCH-R701.8
#[test]
fn losses_are_listed_never_dropped() {
    let steps = vec![
        step("e1", StepKind::Start, None, "Invoice received"),
        step("e2", StepKind::Wait, None, "Payment due"),
        step("e3", StepKind::Decision, None, "Check limits"),
        step("e4", StepKind::End, None, "Done"),
    ];
    let flows = vec![flow("e1", "e2"), flow("e2", "e3"), flow("e3", "e4")];
    let mut workflow = canon("P", ProcessSourceSystem::ArisEpc, "m", steps, flows).unwrap();
    for (n, construct) in [ANNEX_EPC_STATE_EVENT, ANNEX_EPC_ATTACHMENT]
        .iter()
        .enumerate()
    {
        workflow.annex.push(AnnexEntry {
            schema_version: 1,
            after: Some("s1".into()),
            before: Some("s2".into()),
            construct: construct.to_string(),
            element_id: format!("x{n}"),
            label: "kept".into(),
            type_num: None,
        });
    }

    let to_bpmn = export_canonical(&workflow, ProcessSourceSystem::Bpmn20);
    assert_eq!(to_bpmn.losses.len(), 2, "one loss per annex entry");
    assert!(to_bpmn.annex.is_empty());
    assert_eq!(admit_canonical(&to_bpmn).unwrap().digest, workflow.digest);
    let refused =
        export_canonical_strict(&workflow, ProcessSourceSystem::Bpmn20, true).unwrap_err();
    assert_eq!(refused.code(), "PROCESS_EXPORT_LOSSY");
    assert_eq!(refused.losses, to_bpmn.losses);

    let same = export_canonical(&workflow, ProcessSourceSystem::ArisEpc);
    assert_eq!(same.annex, workflow.annex, "same source keeps the annex");
    let treatments: Vec<&str> = same.losses.iter().map(|l| l.treatment.as_str()).collect();
    assert_eq!(treatments, ["typed-trigger", "decision-table"]);
    assert_eq!(admit_canonical(&same).unwrap().digest, workflow.digest);
}

fn lowered_process(n: usize) -> LoweredProcess {
    let (steps, flows) = process(n);
    LoweredProcess {
        name: "P".into(),
        system: ProcessSourceSystem::Bpmn20,
        model_id: "m".into(),
        steps,
        flows,
        annex: Vec::new(),
    }
}

// spec: MEDULLA-CQ-ORCH-R701.11
#[test]
fn each_stage_reads_only_the_previous_form() {
    // Each stage is a function from one typed form to the next.
    let _lower: fn(&SourceGraph, &[KindAnnotation]) -> Result<LoweredProcess, ImportError> = lower;
    let _allocate: fn(LoweredProcess) -> Result<AllocatedProcess, ImportError> = allocate;
    let _emit: fn(AllocatedProcess) -> CanonicalWorkflow = emit;

    let once = allocate(lowered_process(1_000)).unwrap();
    let twice = allocate(relower(&once)).unwrap();
    assert_eq!(twice, once, "allocation is idempotent");

    let graph = SourceGraph {
        name: " Two  steps ".into(),
        system: ProcessSourceSystem::ArisEpc,
        model_id: "m".into(),
        nodes: vec![
            SourceNode {
                element_id: "a".into(),
                class: "go".into(),
                label: "Go".into(),
                role: None,
                reference: None,
            },
            SourceNode {
                element_id: "x".into(),
                class: "noted".into(),
                label: "Noted".into(),
                role: None,
                reference: None,
            },
            SourceNode {
                element_id: "b".into(),
                class: "stop".into(),
                label: "Stop".into(),
                role: None,
                reference: None,
            },
        ],
        edges: vec![flow("a", "x"), flow("x", "b")],
        annex: Vec::new(),
    };
    let annotations = [
        KindAnnotation {
            class: "go",
            kind: Some(StepKind::Start),
            gate: None,
        },
        KindAnnotation {
            class: "noted",
            kind: None,
            gate: None,
        },
        KindAnnotation {
            class: "stop",
            kind: Some(StepKind::End),
            gate: None,
        },
    ];
    let lowered = lower(&graph, &annotations).unwrap();
    assert_eq!(lowered.name, "Two steps");
    assert_eq!(lowered.steps.len(), 2, "a state never reaches a step");
    assert_eq!(lowered.flows, vec![flow("a", "b")]);
    assert_eq!(lowered.annex[0].element_id, "x");
    assert!(matches!(
        lower(&graph, &annotations[..1]),
        Err(ImportError::UnmappedElement { element_type }) if element_type == "noted"
    ));

    // No stage names a source format.
    let source = include_str!("../src/process_import.rs");
    for word in ["Bpmn", "Epc", "bpmn", "aris"] {
        assert!(!source.contains(word), "process_import.rs names {word}");
    }
}

// spec: MEDULLA-CQ-ORCH-R701.11
#[test]
fn canonicalize_is_the_composition_of_the_stages() {
    for n in [10usize, 1_000] {
        let lowered = lowered_process(n);
        let composed = emit(allocate(lowered.clone()).unwrap());
        assert_eq!(canonicalize(lowered).unwrap(), composed);
        assert_eq!(admit_canonical(&composed).unwrap(), composed);
    }
}
