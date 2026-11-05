//! The typed knob loader reads `--knob`, then env, then default, and refuses
//! bad values at startup (MEDULLA-CONTRACT-R058).

use std::collections::HashMap;

use medulla_server_api::knob_config::{load_knobs, KnobClass, KnobConfig, KNOB_TABLE};
use medulla_types::contract::ServerErrorCode;

fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    move |name| map.get(name).cloned()
}

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| v.to_string()).collect()
}

// spec: MEDULLA-CONTRACT-R058
#[test]
fn loader_reads_cli_then_env_then_default_and_refuses_bad_values() {
    // Defaults.
    let defaults = load_knobs(&[], env_of(&[])).expect("defaults load");
    assert_eq!(defaults, KnobConfig::default());
    assert_eq!(defaults.topology_max_width, 16);
    assert!(defaults.sdlc_delivery_owner_approval_required);

    // Table shape: unique names, defaults within range, Fixed is one value.
    let mut names: Vec<_> = KNOB_TABLE.iter().map(|spec| spec.name).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), KNOB_TABLE.len());
    for spec in KNOB_TABLE {
        if spec.class == KnobClass::Fixed {
            assert_eq!(spec.min, spec.max, "{} is Fixed", spec.name);
        }
    }
    assert_eq!(
        KNOB_TABLE[6].env_name(),
        "MEDULLA_TOPOLOGY_MAX_WIDTH",
        "dots become underscores"
    );

    // Env overrides the default; --knob overrides env.
    let env = env_of(&[
        ("MEDULLA_TOPOLOGY_MAX_WIDTH", "4"),
        ("MEDULLA_COUNCIL_QUORUM_FRACTION", "0.75"),
    ]);
    let from_env = load_knobs(&[], &env).expect("env loads");
    assert_eq!(from_env.topology_max_width, 4);
    assert_eq!(from_env.council_quorum_fraction, 0.75);
    let from_cli = load_knobs(&args(&["topology.max_width=2"]), &env).expect("cli loads");
    assert_eq!(from_cli.topology_max_width, 2);
    assert_eq!(from_cli.council_quorum_fraction, 0.75);

    // KNOB_INVALID: a value that does not parse as the type, from either source.
    let bad = load_knobs(&[], env_of(&[("MEDULLA_TOPOLOGY_MAX_WIDTH", "wide")])).unwrap_err();
    assert_eq!(bad.code, ServerErrorCode::KnobInvalid);
    assert_eq!(bad.knob, "topology.max_width");
    let bad = load_knobs(
        &args(&["sdlc.mode.evolving_requires_approval=yes"]),
        env_of(&[]),
    )
    .unwrap_err();
    assert_eq!(bad.code, ServerErrorCode::KnobInvalid);
    let bad = load_knobs(&args(&["topology.max_width"]), env_of(&[])).unwrap_err();
    assert_eq!(bad.code, ServerErrorCode::KnobInvalid);

    // KNOB_OUT_OF_RANGE: outside the inclusive range, and a Fixed knob changed.
    let out = load_knobs(&args(&["topology.max_width=17"]), env_of(&[])).unwrap_err();
    assert_eq!(out.code, ServerErrorCode::KnobOutOfRange);
    assert_eq!(out.knob, "topology.max_width");
    assert!(out
        .to_string()
        .starts_with("KNOB_OUT_OF_RANGE: knob topology.max_width"));
    let out = load_knobs(
        &[],
        env_of(&[("MEDULLA_SDLC_DELIVERY_OWNER_APPROVAL_REQUIRED", "false")]),
    )
    .unwrap_err();
    assert_eq!(out.code, ServerErrorCode::KnobOutOfRange);
    assert!(
        load_knobs(&args(&["topology.max_width=1"]), env_of(&[])).is_ok(),
        "inclusive"
    );

    // KNOB_UNKNOWN applies only to --knob; unrelated MEDULLA_* vars are untouched.
    let unknown = load_knobs(&args(&["no.such_knob=1"]), env_of(&[])).unwrap_err();
    assert_eq!(unknown.code, ServerErrorCode::KnobUnknown);
    assert_eq!(unknown.knob, "no.such_knob");
    assert!(load_knobs(&[], env_of(&[("MEDULLA_NO_SUCH_KNOB", "x")])).is_ok());

    // The codes are registered, closed server error codes.
    for code in ["KNOB_INVALID", "KNOB_OUT_OF_RANGE", "KNOB_UNKNOWN"] {
        assert!(ServerErrorCode::ALL.iter().any(|c| c.as_str() == code));
    }
}
