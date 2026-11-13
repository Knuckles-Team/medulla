//! Edge type-check and substitution rule (MEDULLA-DECISION-ENGINE-R036.100.2),
//! over an exhaustive small model and checked against an independent oracle.

use std::collections::BTreeSet;

use medulla_decision_types::decision::primitive::{EffectClass, PrimitiveContract, PrimitiveKind};

use super::super::primitive::{
    check_graph_size, edge_type_checks, require_edge, substitutes, PrimitiveRefusal, REFUSALS,
};

const KINDS: [PrimitiveKind; 4] = [
    PrimitiveKind::Prompt,
    PrimitiveKind::Tool,
    PrimitiveKind::Memory,
    PrimitiveKind::ControlFlow,
];
const SCHEMAS: [char; 2] = ['a', 'b'];

fn digest(c: char) -> String {
    c.to_string().repeat(64)
}

/// Four effect sets: none, read-only, read plus graph write, and everything.
fn effect_sets() -> [BTreeSet<EffectClass>; 4] {
    [
        BTreeSet::new(),
        BTreeSet::from([EffectClass::ReadOnly]),
        BTreeSet::from([EffectClass::ReadOnly, EffectClass::GraphWrite]),
        BTreeSet::from([
            EffectClass::ReadOnly,
            EffectClass::GraphWrite,
            EffectClass::External,
            EffectClass::Irreversible,
        ]),
    ]
}

fn capability_sets() -> [BTreeSet<String>; 3] {
    let cap = |name: &str| name.to_string();
    [
        BTreeSet::new(),
        BTreeSet::from([cap("read")]),
        BTreeSet::from([cap("read"), cap("write")]),
    ]
}

fn contract(
    kind: PrimitiveKind,
    input: char,
    output: char,
    effects: &BTreeSet<EffectClass>,
    capabilities: &BTreeSet<String>,
) -> PrimitiveContract {
    PrimitiveContract {
        primitive_id: "p".to_string(),
        kind,
        version: "1".to_string(),
        input_schema_digest: digest(input),
        output_schema_digest: digest(output),
        effects: effects.clone(),
        capabilities: capabilities.clone(),
    }
}

/// The rule as the row states it, written without the function under test.
fn oracle(old: &PrimitiveContract, new: &PrimitiveContract) -> Option<&'static str> {
    if old.kind != new.kind {
        return Some("PRIMITIVE_KIND_MISMATCH");
    }
    if old.input_schema_digest != new.input_schema_digest
        || old.output_schema_digest != new.output_schema_digest
    {
        return Some("PRIMITIVE_SCHEMA_CHANGED");
    }
    if old
        .capabilities
        .iter()
        .any(|c| !new.capabilities.contains(c))
    {
        return Some("PRIMITIVE_CAPABILITY_LOST");
    }
    if new.effects.iter().any(|e| !old.effects.contains(e)) {
        return Some("PRIMITIVE_EFFECT_WIDENED");
    }
    None
}

// spec: MEDULLA-DECISION-ENGINE-R036.100.2
#[test]
fn substitution_rules_cover_every_refusal() {
    let effects = effect_sets();
    let caps = capability_sets();
    let mut seen: BTreeSet<&'static str> = BTreeSet::new();
    let mut legal = 0usize;
    let mut total = 0usize;
    for old_kind in KINDS {
        for new_kind in KINDS {
            for (old_in, old_out) in SCHEMAS
                .iter()
                .flat_map(|i| SCHEMAS.iter().map(move |o| (*i, *o)))
            {
                for (new_in, new_out) in SCHEMAS
                    .iter()
                    .flat_map(|i| SCHEMAS.iter().map(move |o| (*i, *o)))
                {
                    for (old_fx, new_fx) in effects
                        .iter()
                        .flat_map(|a| effects.iter().map(move |b| (a, b)))
                    {
                        for (old_cap, new_cap) in
                            caps.iter().flat_map(|a| caps.iter().map(move |b| (a, b)))
                        {
                            let old = contract(old_kind, old_in, old_out, old_fx, old_cap);
                            let new = contract(new_kind, new_in, new_out, new_fx, new_cap);
                            let got = substitutes(&old, &new).err().map(PrimitiveRefusal::code);
                            assert_eq!(got, oracle(&old, &new), "{old:?} -> {new:?}");
                            total += 1;
                            match got {
                                Some(code) => {
                                    seen.insert(code);
                                }
                                None => legal += 1,
                            }
                        }
                    }
                }
            }
        }
    }
    assert_eq!(total, 4 * 4 * 4 * 4 * 4 * 4 * 3 * 3);
    assert!(legal > 0, "the model has legal replacements");
    let declared: BTreeSet<&str> = REFUSALS
        .iter()
        .map(|r| r.code())
        .filter(|code| *code != "PRIMITIVE_EDGE_MISMATCH" && *code != "PRIMITIVE_GRAPH_TOO_LARGE")
        .collect();
    assert_eq!(seen, declared, "every substitution refusal is reached");
}

// spec: MEDULLA-DECISION-ENGINE-R036.100.2
#[test]
fn edges_type_check_on_the_schema_digest_and_graphs_are_bounded() {
    let none = BTreeSet::new();
    let no_caps = BTreeSet::new();
    for (out, input) in SCHEMAS
        .iter()
        .flat_map(|o| SCHEMAS.iter().map(move |i| (*o, *i)))
    {
        let producer = contract(PrimitiveKind::Tool, 'a', out, &none, &no_caps);
        let consumer = contract(PrimitiveKind::Prompt, input, 'b', &none, &no_caps);
        assert_eq!(edge_type_checks(&producer, &consumer), out == input);
        assert_eq!(
            require_edge(&producer, &consumer),
            if out == input {
                Ok(())
            } else {
                Err(PrimitiveRefusal::EdgeMismatch)
            }
        );
    }
    assert_eq!(check_graph_size(256, 256), Ok(()));
    assert_eq!(
        check_graph_size(257, 256),
        Err(PrimitiveRefusal::GraphTooLarge)
    );
    assert_eq!(check_graph_size(0, 16), Ok(()));
    let codes: BTreeSet<&str> = REFUSALS.iter().map(|r| r.code()).collect();
    assert_eq!(codes.len(), REFUSALS.len(), "codes are distinct");
    assert!(codes.iter().all(|c| c.starts_with("PRIMITIVE_")));
}
