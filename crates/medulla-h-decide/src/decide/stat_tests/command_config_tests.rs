//! Command config served through `Decide` (MEDULLA-DECISION-ENGINE-R148.3): a
//! mode is chosen from the `CommandMode` and `CommandConfigSchema` nodes of the
//! graph the question names, the config is built against the schema, and
//! another tenant's nodes are never read.

use super::*;
use medulla_decision_types::decision::command_config::{
    CommandConfig, CommandConfigSchema, CommandMode, ConfigScalar, FieldType, ModeDecision,
    COMMAND_CONFIG_SCHEMA_VERSION,
};
use medulla_decision_types::decision::knobs::{KnobName, KnobValue};
use medulla_decision_types::decision::{DecisionPolicy, ResolutionKind};
use std::collections::BTreeMap;

const GRAPH: &str = "kg-command-modes";
const OTHER: &str = "tenant-other";

fn settings(retries: i64, label: &str) -> BTreeMap<String, ConfigScalar> {
    BTreeMap::from([
        ("retries".to_string(), ConfigScalar::Int(retries)),
        ("label".to_string(), ConfigScalar::Text(label.to_string())),
    ])
}

fn schema() -> CommandConfigSchema {
    CommandConfigSchema {
        schema_version: COMMAND_CONFIG_SCHEMA_VERSION,
        schema_id: "schema:cmd".into(),
        revision: 3,
        fields: BTreeMap::from([
            ("retries".to_string(), FieldType::Int),
            ("label".to_string(), FieldType::Text),
        ]),
    }
}

fn mode(
    id: &str,
    ordinal: u32,
    revision: u64,
    values: BTreeMap<String, ConfigScalar>,
) -> CommandMode {
    CommandMode {
        schema_version: COMMAND_CONFIG_SCHEMA_VERSION,
        mode_id: id.into(),
        mode_ordinal: ordinal,
        mode_revision: revision,
        schema_id: "schema:cmd".into(),
        settings: values,
    }
}

fn add<T: serde::Serialize>(
    core: &medulla_graph_core::graph::GraphCore,
    node_id: &str,
    label: &str,
    record: &T,
    tenant: Option<&str>,
) {
    let mut props = serde_json::to_value(record).unwrap();
    props["type"] = serde_json::json!(label);
    if let Some(tenant) = tenant {
        props["tenant_id"] = serde_json::json!(tenant);
    }
    core.add_node(
        node_id.to_string(),
        rmp_serde::to_vec_named(&props).unwrap(),
    );
}

/// The tenant's graph: schema `cmd` at revision 3, mode `safe` (ordinal 0),
/// mode `fast` (ordinal 1, revision 7), a mode `broken` that misses a setting,
/// and another tenant's own `fast` (other settings), `theirs` and ordinal-5 mode.
async fn harness() -> Harness {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    {
        let mut guard = h.state.write().await;
        guard
            .registry
            .create_graph(
                GRAPH,
                medulla_protocol_types::protocol::GraphType::Team,
                None,
            )
            .unwrap();
        let core = guard.registry.get(GRAPH).unwrap().core.clone();
        add(
            &core,
            "schema:cmd:3",
            "CommandConfigSchema",
            &schema(),
            None,
        );
        add(
            &core,
            "mode:safe",
            "CommandMode",
            &mode("safe", 0, 1, settings(1, "safe")),
            None,
        );
        add(
            &core,
            "mode:fast",
            "CommandMode",
            &mode("fast", 1, 7, settings(5, "fast")),
            Some(TENANT),
        );
        let mut broken = mode("broken", 2, 1, settings(1, "x"));
        broken.settings.remove("label");
        add(&core, "mode:broken", "CommandMode", &broken, None);
        add(
            &core,
            "other:fast",
            "CommandMode",
            &mode("fast", 1, 99, settings(50, "theirs")),
            Some(OTHER),
        );
        add(
            &core,
            "other:theirs",
            "CommandMode",
            &mode("theirs", 4, 1, settings(2, "t")),
            Some(OTHER),
        );
        add(
            &core,
            "other:ordinal5",
            "CommandMode",
            &mode("five", 5, 1, settings(2, "five")),
            Some(OTHER),
        );
    }
    h
}

fn text(name: &str, value: &str) -> TypedParam {
    TypedParam {
        name: name.to_string(),
        value: TypedValue::Text(value.to_string()),
    }
}

fn int(name: &str, value: i64) -> TypedParam {
    TypedParam {
        name: name.to_string(),
        value: TypedValue::Int(value),
    }
}

fn request(
    schema_pin: &ComponentDependency,
    policy: DecisionPolicyRef,
    mut params: Vec<TypedParam>,
) -> DecideRequest {
    params.push(text("graph", GRAPH));
    declared_request(
        schema_pin,
        TENANT,
        QuestionKind::CommandConfig,
        "mode",
        policy,
        params,
    )
}

fn options(record: &StatisticalDecisionRecord) -> Vec<String> {
    let StatisticalOutcome::Advisory { scores, .. } = &record.outcome else {
        panic!("expected an advisory answer, got {:?}", record.outcome);
    };
    scores.iter().map(|s| s.option_id.clone()).collect()
}

fn refusal_code(record: &StatisticalDecisionRecord) -> String {
    assert_eq!(record.resolution_kind, ResolutionKind::Abstention);
    let StatisticalOutcome::Abstained { reasons } = &record.outcome else {
        panic!("expected an abstention, got {:?}", record.outcome);
    };
    reasons.as_slice()[0].code().as_str().to_string()
}

async fn ask(
    h: &Harness,
    schema_pin: &ComponentDependency,
    policy: DecisionPolicyRef,
    params: Vec<TypedParam>,
) -> StatisticalDecisionRecord {
    decide(h, request(schema_pin, policy, params))
        .await
        .unwrap()
        .records
        .as_slice()[0]
        .clone()
}

fn digest_of(id: &str, values: BTreeMap<String, ConfigScalar>) -> String {
    CommandConfig::new("schema:cmd", id, values)
        .config_digest
        .to_hex()
}

/// Verifies MEDULLA-DECISION-ENGINE-R148.3: the served answer names the mode and
/// the digest of its config, keeps the whole decision, and refuses with the
/// registered codes.
// spec: MEDULLA-DECISION-ENGINE-R148.3
#[tokio::test]
async fn decide_routes_mode() {
    let h = harness().await;
    let (schema_pin, _) = publish_route_schema(&h);
    let default = DecisionPolicyRef::Default;

    // A named mode: its id, the digest of its settings, the full decision.
    let record = ask(
        &h,
        &schema_pin,
        default.clone(),
        vec![text("mode_id", "fast")],
    )
    .await;
    assert_eq!(record.question.kind, QuestionKind::CommandConfig);
    let fast_digest = digest_of("fast", settings(5, "fast"));
    assert_eq!(
        options(&record),
        vec!["mode:fast".to_string(), format!("config:{fast_digest}")]
    );
    let decision_fact = record
        .premises
        .iter()
        .find_map(|p| p.fact.strip_prefix("command_config:decision="))
        .expect("the premise keeps the full mode decision");
    let decision: ModeDecision = serde_json::from_str(decision_fact).unwrap();
    assert_eq!(decision.mode_id, "fast");
    assert_eq!(decision.mode_revision, 7);
    assert_eq!(decision.config.settings, settings(5, "fast"));
    assert_eq!(decision.config.config_digest.to_hex(), fast_digest);
    let facts: Vec<&str> = record.premises.iter().map(|p| p.fact.as_str()).collect();
    assert!(facts.contains(&"command_config:mode=fast@7"));
    assert!(facts.contains(&"command_config:schema=schema:cmd@3"));

    // Without a request the default ordinal (0) names `safe`.
    let record = ask(&h, &schema_pin, default.clone(), vec![]).await;
    assert_eq!(
        options(&record)[0],
        "mode:safe",
        "the default_mode_id knob names ordinal 0"
    );
    // A pinned default ordinal names another mode.
    let mut policy = DecisionPolicy::engine_default();
    policy
        .knobs
        .insert(KnobName::new("mode.default_mode_id"), KnobValue::Integer(1));
    let pinned = DecisionPolicyRef::Pinned {
        component: h.publish_policy("mode-default-one", &policy),
    };
    let record = ask(&h, &schema_pin, pinned, vec![]).await;
    assert_eq!(options(&record)[0], "mode:fast");

    // Expected revision and digest match, then each mismatch refuses.
    let record = ask(
        &h,
        &schema_pin,
        default.clone(),
        vec![
            text("mode_id", "fast"),
            int("expected_mode_revision", 7),
            text("expected_config_digest", &fast_digest),
        ],
    )
    .await;
    assert_eq!(options(&record)[0], "mode:fast");
    let record = ask(
        &h,
        &schema_pin,
        default.clone(),
        vec![text("mode_id", "fast"), int("expected_mode_revision", 6)],
    )
    .await;
    assert_eq!(refusal_code(&record), "MODE_UNKNOWN");
    let record = ask(
        &h,
        &schema_pin,
        default.clone(),
        vec![
            text("mode_id", "fast"),
            text(
                "expected_config_digest",
                &digest_of("fast", settings(6, "fast")),
            ),
        ],
    )
    .await;
    assert_eq!(refusal_code(&record), "CONFIG_INVALID");

    // An unknown mode, and a mode whose settings miss a schema field.
    let record = ask(
        &h,
        &schema_pin,
        default.clone(),
        vec![text("mode_id", "nope")],
    )
    .await;
    assert_eq!(refusal_code(&record), "MODE_UNKNOWN");
    let record = ask(
        &h,
        &schema_pin,
        default.clone(),
        vec![text("mode_id", "broken")],
    )
    .await;
    assert_eq!(refusal_code(&record), "CONFIG_INVALID");

    // Exactly the one declared option `mode`, and the graph param is required.
    let mut two = request(&schema_pin, default.clone(), vec![]);
    two.candidates = CandidateSource::Declared {
        options: BoundedVec::new(vec![declared_option("mode"), declared_option("other")]).unwrap(),
    };
    assert!(decide(&h, two)
        .await
        .unwrap_err()
        .starts_with("PARAMETER_INVALID"));
    let mut no_graph = request(&schema_pin, default, vec![]);
    no_graph.params = BoundedVec::default();
    assert!(decide(&h, no_graph)
        .await
        .unwrap_err()
        .starts_with("PARAMETER_INVALID"));
}

/// Verifies MEDULLA-DECISION-ENGINE-R148.3: another tenant's modes are never
/// read, by name, by ordinal or when both tenants hold the same mode id.
// spec: MEDULLA-DECISION-ENGINE-R148.3
#[tokio::test]
async fn other_tenant_modes_are_not_read() {
    let h = harness().await;
    let (schema_pin, _) = publish_route_schema(&h);
    let default = DecisionPolicyRef::Default;

    // Both tenants hold a `fast`; the caller reads its own (revision 7, retries 5).
    let record = ask(
        &h,
        &schema_pin,
        default.clone(),
        vec![text("mode_id", "fast")],
    )
    .await;
    assert_eq!(
        options(&record)[1],
        format!("config:{}", digest_of("fast", settings(5, "fast")))
    );
    assert!(record
        .premises
        .iter()
        .any(|p| p.fact == "command_config:mode=fast@7"));
    assert!(!record.premises.iter().any(|p| p.fact.contains("@99")));

    // A mode only the other tenant holds is unknown.
    let record = ask(
        &h,
        &schema_pin,
        default.clone(),
        vec![text("mode_id", "theirs")],
    )
    .await;
    assert_eq!(refusal_code(&record), "MODE_UNKNOWN");

    // A default ordinal only the other tenant holds names nothing.
    let mut policy = DecisionPolicy::engine_default();
    policy
        .knobs
        .insert(KnobName::new("mode.default_mode_id"), KnobValue::Integer(5));
    let pinned = DecisionPolicyRef::Pinned {
        component: h.publish_policy("mode-default-five", &policy),
    };
    let record = ask(&h, &schema_pin, pinned, vec![]).await;
    assert_eq!(refusal_code(&record), "MODE_UNKNOWN");

    // The other tenant's caller cannot ask for this tenant's graph.
    let mut foreign = request(&schema_pin, default, vec![text("mode_id", "fast")]);
    foreign.tenant_id = OTHER.to_string();
    let other = VerifiedRequestContext::verified_for_test_in_tenant("decider", OTHER);
    let error = decode::<DecisionBatch>(
        super::super::statistical::handle_decide(&h.state, 1, &other, foreign).await,
    )
    .unwrap_err();
    assert!(error.starts_with("COMPONENT_PIN_MISMATCH"), "{error}");
}
