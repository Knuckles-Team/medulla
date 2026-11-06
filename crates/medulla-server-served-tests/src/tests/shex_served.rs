//! Served ShEx data-selection proof for MEDULLA-FEDERATED-QUERY-R084.2.
//!
//! The handler must validate nonblank `data_graph` as inline Turtle, while an
//! empty or whitespace-only value must select the projected live graph.  The
//! paired calls below use identical data with different schemas to prove that
//! whitespace actually selects the live graph rather than an empty graph.
//!
//! The module also carries the refusal (R084.6), scoped-read (R084.7) and journey
//! (R084.8) proofs. It compiles in every build so the refusal proof can assert the
//! `METHOD_NOT_YET_SERVED` reply of a build without the `shex` feature.
#![cfg_attr(not(feature = "shex"), allow(dead_code, unused_imports))]

use super::*;

const PERSON_NAME_SCHEMA: &str = r#"{
  "type": "Schema",
  "shapes": [
    {
      "type": "Shape",
      "id": "http://example.org/PersonShape",
      "expression": {
        "type": "TripleConstraint",
        "predicate": "http://example.org/name",
        "valueExpr": {
          "type": "NodeConstraint",
          "datatype": "http://www.w3.org/2001/XMLSchema#string"
        },
        "min": 1,
        "max": 1
      }
    }
  ]
}"#;

const OTHER_VALUE_SCHEMA: &str = r#"{
  "type": "Schema",
  "shapes": [
    {
      "type": "Shape",
      "id": "http://example.org/OtherShape",
      "expression": {
        "type": "TripleConstraint",
        "predicate": "http://example.org/other",
        "valueExpr": {
          "type": "NodeConstraint",
          "datatype": "http://www.w3.org/2001/XMLSchema#string"
        },
        "min": 1,
        "max": 1
      }
    }
  ]
}"#;

const INLINE_PERSON_GRAPH: &str =
    r#"<http://example.org/alice> <http://example.org/name> "Inline Alice" ."#;
const LIVE_OTHER_GRAPH: &str =
    r#"<http://example.org/alice> <http://example.org/other> "live-only" ."#;
const ALICE_SHAPE_MAP: [[&str; 2]; 1] =
    [["http://example.org/alice", "http://example.org/PersonShape"]];
const ALICE_OTHER_SHAPE_MAP: [[&str; 2]; 1] =
    [["http://example.org/alice", "http://example.org/OtherShape"]];

fn shex_report(response: &Response) -> medulla_types::rdf_report::ShexValidationReport {
    assert_ok(response);
    match &response.result {
        Some(ResultPayload::Json(value)) => serde_json::from_value(value.clone())
            .expect("ShexValidate JSON must decode as its wire report"),
        other => panic!(
            "expected ShexValidate JSON result, got {other:?}; error={:?}",
            response.error
        ),
    }
}

// spec: MEDULLA-FEDERATED-QUERY-R084.2
#[cfg(feature = "shex")]
#[tokio::test]
async fn inline_and_live_data_selection() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let state = test_state();

    // Seed a genuine RDF literal through the RDF write surface. A plain AddNode
    // string property is not a typed RDF literal cell and is intentionally not
    // emitted by the live RDF exporter.
    #[cfg(feature = "shacl")]
    super::configure_icv_enforce(&state, 0, "__commons__").await;
    let live_seed = Method::AddTriples {
        turtle: LIVE_OTHER_GRAPH.into(),
        ntriples: String::new(),
    };
    assert_ok(&dispatch_on_heap(&state, request(1, "__commons__", None, live_seed)).await);

    let inline = dispatch_on_heap(
        &state,
        request(
            2,
            "__commons__",
            None,
            Method::ShexValidate {
                schema: PERSON_NAME_SCHEMA.into(),
                data_graph: INLINE_PERSON_GRAPH.into(),
                shape_map: ALICE_SHAPE_MAP
                    .iter()
                    .map(|[node, shape]| [(*node).into(), (*shape).into()])
                    .collect(),
            },
        ),
    )
    .await;
    let inline_report = shex_report(&inline);
    assert!(
        inline_report.conforms,
        "inline Turtle must satisfy the shape"
    );
    assert_eq!(inline_report.results.len(), 1);
    assert!(inline_report.results[0].conforms);

    // Whitespace is the live-graph sentinel.  The inline result above must not
    // have been imported, so the projected live graph still fails the required
    // `ex:name` constraint.
    let live = dispatch_on_heap(
        &state,
        request(
            3,
            "__commons__",
            None,
            Method::ShexValidate {
                schema: PERSON_NAME_SCHEMA.into(),
                data_graph: " \n\t ".into(),
                shape_map: ALICE_SHAPE_MAP
                    .iter()
                    .map(|[node, shape]| [(*node).into(), (*shape).into()])
                    .collect(),
            },
        ),
    )
    .await;
    let live_report = shex_report(&live);
    assert!(
        !live_report.conforms,
        "whitespace data_graph must select live RDF"
    );
    assert_eq!(live_report.results.len(), 1);
    assert!(!live_report.results[0].conforms);

    // Positively identify the selected source: the same inline document has no
    // `ex:other`, while the live graph contains it. An empty graph would fail
    // both cases and therefore could not satisfy this pair of assertions.
    let inline_other = dispatch_on_heap(
        &state,
        request(
            4,
            "__commons__",
            None,
            Method::ShexValidate {
                schema: OTHER_VALUE_SCHEMA.into(),
                data_graph: INLINE_PERSON_GRAPH.into(),
                shape_map: ALICE_OTHER_SHAPE_MAP
                    .iter()
                    .map(|[node, shape]| [(*node).into(), (*shape).into()])
                    .collect(),
            },
        ),
    )
    .await;
    let inline_other_report = shex_report(&inline_other);
    assert!(
        !inline_other_report.conforms,
        "the inline document must not contain the live-only predicate"
    );
    assert_eq!(inline_other_report.results.len(), 1);
    assert!(!inline_other_report.results[0].conforms);

    let live_other = dispatch_on_heap(
        &state,
        request(
            5,
            "__commons__",
            None,
            Method::ShexValidate {
                schema: OTHER_VALUE_SCHEMA.into(),
                data_graph: " \n\t ".into(),
                shape_map: ALICE_OTHER_SHAPE_MAP
                    .iter()
                    .map(|[node, shape]| [(*node).into(), (*shape).into()])
                    .collect(),
            },
        ),
    )
    .await;
    let live_other_report = shex_report(&live_other);
    assert!(
        live_other_report.conforms,
        "whitespace data_graph must use the seeded live ex:other value"
    );
    assert_eq!(live_other_report.results.len(), 1);
    assert!(live_other_report.results[0].conforms);
}

const EX_OTHER: &str = "http://example.org/other";

fn shape_map(pairs: &[(&str, &str)]) -> Vec<[String; 2]> {
    pairs
        .iter()
        .map(|(node, shape)| [(*node).to_string(), (*shape).to_string()])
        .collect()
}

fn shex_validate(schema: &str, data_graph: &str, pairs: &[(&str, &str)]) -> Method {
    Method::ShexValidate {
        schema: schema.into(),
        data_graph: data_graph.into(),
        shape_map: shape_map(pairs),
    }
}

/// The refusal as `CODE: detail`, the text `Response::err` accepts.
fn error_text(response: &Response) -> String {
    response
        .refusal_text()
        .unwrap_or_else(|| panic!("expected a refusal, got {:?}", response.result))
}

/// `(node, conforms)` of every result, in shape-map order.
fn outcomes(report: &medulla_types::rdf_report::ShexValidationReport) -> Vec<(String, bool)> {
    report
        .results
        .iter()
        .map(|r| (r.node.clone(), r.conforms))
        .collect()
}

/// Seed one RDF literal row straight on a graph's core: `<node> ex:other "value"`,
/// owned by `owner` when given, else untagged (hidden from every non-System caller).
async fn seed_literal_row(
    state: &Arc<RwLock<ServerState>>,
    graph: &str,
    node_iri: &str,
    value: &str,
    owner: Option<&str>,
) {
    let core = {
        let s = state.read().await;
        s.registry
            .get(graph)
            .expect("the graph exists after its first served write")
            .core
            .clone()
    };
    let mut props = serde_json::json!({ EX_OTHER: { "value": value } });
    if let Some(owner) = owner {
        props["_owner_id"] = serde_json::json!(owner);
    }
    core.add_node(
        format!("<{node_iri}>"),
        rmp_serde::to_vec_named(&props).unwrap(),
    );
    core.mark_dirty();
}

/// MEDULLA-FEDERATED-QUERY-R084.6: a bad schema, bad inline data and a failed live
/// export each refuse with their own prefix; an unknown shape label is a
/// nonconforming result with a reason; a build without `shex` replies
/// `METHOD_NOT_YET_SERVED` and never a report.
// spec: MEDULLA-FEDERATED-QUERY-R084.6
#[tokio::test]
async fn parse_and_dispatch_failures_are_distinct() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let state = test_state();

    #[cfg(not(feature = "shex"))]
    {
        let resp = dispatch_on_heap(
            &state,
            request(
                1,
                "__commons__",
                None,
                shex_validate(PERSON_NAME_SCHEMA, INLINE_PERSON_GRAPH, &ALICE),
            ),
        )
        .await;
        assert!(resp.result.is_none(), "no fabricated report without `shex`");
        assert!(
            error_text(&resp).starts_with("METHOD_NOT_YET_SERVED"),
            "got {:?}",
            resp.error
        );
    }

    #[cfg(feature = "shex")]
    {
        let bad_schema = dispatch_on_heap(
            &state,
            request(
                1,
                "__commons__",
                None,
                shex_validate("{ not json", INLINE_PERSON_GRAPH, &ALICE),
            ),
        )
        .await;
        let bad_schema = error_text(&bad_schema);
        assert!(
            bad_schema.contains("ShexValidate: bad schema:"),
            "{bad_schema}"
        );

        let bad_data = dispatch_on_heap(
            &state,
            request(
                2,
                "__commons__",
                None,
                shex_validate(
                    PERSON_NAME_SCHEMA,
                    "<http://example.org/alice> <unterminated",
                    &ALICE,
                ),
            ),
        )
        .await;
        let bad_data = error_text(&bad_data);
        assert!(
            bad_data.contains("ShexValidate: bad data graph:"),
            "{bad_data}"
        );

        // An unknown label parses and dispatches: it is a nonconforming result.
        let unknown = dispatch_on_heap(
            &state,
            request(
                3,
                "__commons__",
                None,
                shex_validate(
                    PERSON_NAME_SCHEMA,
                    INLINE_PERSON_GRAPH,
                    &[("http://example.org/alice", "http://example.org/NoSuchShape")],
                ),
            ),
        )
        .await;
        let unknown = shex_report(&unknown);
        assert!(!unknown.conforms);
        assert_eq!(unknown.results.len(), 1);
        assert!(!unknown.results[0].conforms);
        assert!(unknown.results[0]
            .reason
            .as_deref()
            .unwrap_or("")
            .contains("unknown shape label"));

        // A plain property-graph edge has no RDF identity, so the live export fails.
        for (id, node) in [(4, "plain-a"), (5, "plain-b")] {
            assert_ok(
                &dispatch_on_heap(&state, request(id, "__commons__", None, add_node(node))).await,
            );
        }
        let edge = Method::AddEdge {
            source_id: "plain-a".into(),
            target_id: "plain-b".into(),
            properties_msgpack: rmp_serde::to_vec_named(
                &serde_json::json!({"relationship": "KNOWS"}),
            )
            .unwrap(),
        };
        assert_ok(&dispatch_on_heap(&state, request(6, "__commons__", None, edge)).await);
        let export = dispatch_on_heap(
            &state,
            request(
                7,
                "__commons__",
                None,
                shex_validate(PERSON_NAME_SCHEMA, "", &ALICE),
            ),
        )
        .await;
        let export = error_text(&export);
        assert!(
            export.contains("ShexValidate: export live graph:"),
            "{export}"
        );

        // Three distinct failures, three distinct prefixes.
        for (a, b) in [
            (&bad_schema, &bad_data),
            (&bad_schema, &export),
            (&bad_data, &export),
        ] {
            assert_ne!(a, b);
        }
    }
}

const ALICE: [(&str, &str); 1] = [("http://example.org/alice", "http://example.org/PersonShape")];

const SCOPE_MAP: [(&str, &str); 3] = [
    ("http://example.org/alice", "http://example.org/OtherShape"),
    ("http://example.org/bob", "http://example.org/OtherShape"),
    ("http://example.org/carol", "http://example.org/OtherShape"),
];

/// MEDULLA-FEDERATED-QUERY-R084.7: live validation reads only the projected core of
/// the caller's served-read authority; inline Turtle reads no stored row; the call
/// leaves the graph unchanged.
// spec: MEDULLA-FEDERATED-QUERY-R084.7
#[cfg(feature = "shex")]
#[tokio::test]
async fn live_validation_obeys_read_authority() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let state = multi_tenant_state().await;

    // Create both tenants' graphs with a served write, then seed RDF rows.
    assert_ok(
        &dispatch_on_heap(
            &state,
            request(1, "agent:worker1", Some("worker1"), add_node("seed1")),
        )
        .await,
    );
    assert_ok(
        &dispatch_on_heap(
            &state,
            request(2, "agent:worker2", Some("worker2"), add_node("seed2")),
        )
        .await,
    );
    seed_literal_row(
        &state,
        "agent:worker1",
        "http://example.org/alice",
        "mine",
        Some("worker1"),
    )
    .await;
    seed_literal_row(
        &state,
        "agent:worker2",
        "http://example.org/carol",
        "theirs",
        Some("worker2"),
    )
    .await;

    let live = |id: u64, agent: Option<&'static str>| {
        let state = state.clone();
        async move {
            let resp = dispatch_on_heap(
                &state,
                request(
                    id,
                    "agent:worker1",
                    agent,
                    shex_validate(OTHER_VALUE_SCHEMA, "", &SCOPE_MAP),
                ),
            )
            .await;
            shex_report(&resp)
        }
    };

    let before_hidden = live(3, Some("worker1")).await;
    assert_eq!(
        outcomes(&before_hidden),
        vec![
            ("<http://example.org/alice>".to_string(), true),
            ("<http://example.org/bob>".to_string(), false),
            ("<http://example.org/carol>".to_string(), false),
        ],
        "worker1 sees its own row and never the other tenant's"
    );

    // An untagged row is hidden from worker1: it must make no difference.
    seed_literal_row(
        &state,
        "agent:worker1",
        "http://example.org/bob",
        "hidden",
        None,
    )
    .await;
    let core = {
        let s = state.read().await;
        s.registry.get("agent:worker1").unwrap().core.clone()
    };
    let nodes_before = core.get_nodes().into_iter().count();
    let after_hidden = live(4, Some("worker1")).await;
    assert_eq!(outcomes(&after_hidden), outcomes(&before_hidden));

    // The System principal sees the untagged row, so the difference is the scope.
    let system = live(5, None).await;
    assert_eq!(
        outcomes(&system),
        vec![
            ("<http://example.org/alice>".to_string(), true),
            ("<http://example.org/bob>".to_string(), true),
            ("<http://example.org/carol>".to_string(), false),
        ],
        "the other tenant's graph never reaches the validator"
    );

    // Inline Turtle is caller data: no stored row satisfies the shape for it.
    let inline = dispatch_on_heap(
        &state,
        request(
            6,
            "agent:worker1",
            None,
            shex_validate(OTHER_VALUE_SCHEMA, INLINE_PERSON_GRAPH, &SCOPE_MAP),
        ),
    )
    .await;
    assert!(outcomes(&shex_report(&inline)).iter().all(|(_, ok)| !ok));

    // Read-only: no row added or removed.
    assert_eq!(core.get_nodes().into_iter().count(), nodes_before);
}

const PERSON_IRI: &str = "http://example.org/PersonShape";

fn single_shape_schema(label: &str, expression: serde_json::Value, closed: bool) -> String {
    serde_json::json!({
        "type": "Schema",
        "shapes": [{ "type": "Shape", "id": label, "closed": closed, "expression": expression }]
    })
    .to_string()
}

fn tc(predicate: &str, min: i64, max: i64) -> serde_json::Value {
    serde_json::json!({ "type": "TripleConstraint", "predicate": predicate, "min": min, "max": max })
}

async fn report_for(
    state: &Arc<RwLock<ServerState>>,
    id: u64,
    schema: &str,
    data: &str,
    pairs: &[(&str, &str)],
) -> medulla_types::rdf_report::ShexValidationReport {
    shex_report(
        &dispatch_on_heap(
            state,
            request(id, "__commons__", None, shex_validate(schema, data, pairs)),
        )
        .await,
    )
}

/// MEDULLA-FEDERATED-QUERY-R084.8: the served contract journey over inline and live
/// data, ordered mixed outcomes, parse failures, unknown labels, denied scope and the
/// known matcher limits of `crates/medulla-shex/src/validate.rs`. It asserts the limits
/// as they are; it makes no claim of complete ShEx language support.
// spec: MEDULLA-FEDERATED-QUERY-R084.8
#[cfg(feature = "shex")]
#[tokio::test]
async fn served_shex_contract_journey() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let state = multi_tenant_state().await;

    // Inline data, ordered mixed outcomes: results follow shape-map order.
    let mixed_data = r#"<http://example.org/alice> <http://example.org/name> "Alice" .
<http://example.org/bob> <http://example.org/other> "no name" .
<http://example.org/carol> <http://example.org/name> "Carol" ."#;
    let mixed = report_for(
        &state,
        1,
        PERSON_NAME_SCHEMA,
        mixed_data,
        &[
            ("http://example.org/carol", PERSON_IRI),
            ("http://example.org/bob", PERSON_IRI),
            ("http://example.org/alice", PERSON_IRI),
        ],
    )
    .await;
    assert!(!mixed.conforms, "one failing node fails the whole map");
    assert_eq!(
        outcomes(&mixed),
        vec![
            ("<http://example.org/carol>".to_string(), true),
            ("<http://example.org/bob>".to_string(), false),
            ("<http://example.org/alice>".to_string(), true),
        ]
    );
    assert!(mixed.results[1].reason.is_some());

    // Live data: worker1's own row validates on its graph.
    assert_ok(
        &dispatch_on_heap(
            &state,
            request(2, "agent:worker1", Some("worker1"), add_node("seed1")),
        )
        .await,
    );
    seed_literal_row(
        &state,
        "agent:worker1",
        "http://example.org/alice",
        "live",
        Some("worker1"),
    )
    .await;
    let live = shex_report(
        &dispatch_on_heap(
            &state,
            request(
                3,
                "agent:worker1",
                Some("worker1"),
                shex_validate(
                    OTHER_VALUE_SCHEMA,
                    " ",
                    &ALICE_OTHER_SHAPE_MAP.map(|[n, s]| (n, s)),
                ),
            ),
        )
        .await,
    );
    assert!(live.conforms);

    // Denied scope: worker2 may not read worker1's graph.
    assert_denied(
        &dispatch_on_heap(
            &state,
            request(
                4,
                "agent:worker1",
                Some("worker2"),
                shex_validate(OTHER_VALUE_SCHEMA, "", &ALICE),
            ),
        )
        .await,
    );

    // Parse failures and an unknown label.
    let bad = dispatch_on_heap(
        &state,
        request(
            5,
            "__commons__",
            None,
            shex_validate("[]", INLINE_PERSON_GRAPH, &ALICE),
        ),
    )
    .await;
    assert!(error_text(&bad).contains("ShexValidate: bad schema:"));
    let bad = dispatch_on_heap(
        &state,
        request(
            6,
            "__commons__",
            None,
            shex_validate(PERSON_NAME_SCHEMA, "@prefix", &ALICE),
        ),
    )
    .await;
    assert!(error_text(&bad).contains("ShexValidate: bad data graph:"));
    let unknown = report_for(
        &state,
        7,
        PERSON_NAME_SCHEMA,
        INLINE_PERSON_GRAPH,
        &[("http://example.org/alice", "http://example.org/Missing")],
    )
    .await;
    assert!(!unknown.results[0].conforms);
    assert!(unknown.results[0].reason.is_some());

    // Matcher limit 1: `OneOf` commits the first matching branch. A zero-minimum first
    // branch matches with no arcs, so a closed shape then rejects the `q` arc that the
    // second branch would have consumed.
    let one_of = single_shape_schema(
        PERSON_IRI,
        serde_json::json!({ "type": "OneOf", "expressions": [
            tc("http://example.org/p", 0, -1),
            tc("http://example.org/q", 1, 1),
        ]}),
        true,
    );
    let r = report_for(
        &state,
        8,
        &one_of,
        r#"<http://example.org/n> <http://example.org/q> "v" ."#,
        &[("http://example.org/n", PERSON_IRI)],
    )
    .await;
    assert!(!r.conforms, "OneOf takes the first matching branch");

    // Matcher limit 2: constraints that share a predicate match greedily: the first
    // takes both arcs and the second finds none.
    let shared = single_shape_schema(
        PERSON_IRI,
        serde_json::json!({ "type": "EachOf", "expressions": [
            tc("http://example.org/p", 1, -1),
            tc("http://example.org/p", 1, 1),
        ]}),
        false,
    );
    let r = report_for(
        &state,
        9,
        &shared,
        r#"<http://example.org/n> <http://example.org/p> "a" .
<http://example.org/n> <http://example.org/p> "b" ."#,
        &[("http://example.org/n", PERSON_IRI)],
    )
    .await;
    assert!(!r.conforms, "shared predicates match greedily");

    // Matcher limit 3: the inverse flag is ignored, so an OUTGOING arc satisfies it.
    let mut inverse = tc("http://example.org/p", 1, 1);
    inverse["inverse"] = serde_json::json!(true);
    let inverse = single_shape_schema(PERSON_IRI, inverse, false);
    let r = report_for(
        &state,
        10,
        &inverse,
        r#"<http://example.org/n> <http://example.org/p> <http://example.org/m> ."#,
        &[("http://example.org/n", PERSON_IRI)],
    )
    .await;
    assert!(r.conforms, "the inverse flag is ignored");

    // Matcher limit 4: above depth 40 the validator assumes success. A recursive
    // `next` chain whose last node has no `next` fails when short and conforms when
    // it is deep enough to cross the depth guard.
    let chain_schema = single_shape_schema(
        PERSON_IRI,
        serde_json::json!({
            "type": "TripleConstraint",
            "predicate": "http://example.org/next",
            "valueExpr": PERSON_IRI,
            "min": 1,
            "max": 1
        }),
        false,
    );
    let chain = |len: usize| {
        (0..len)
            .map(|i| {
                format!("<http://example.org/c{i}> <http://example.org/next> <http://example.org/c{}> .\n", i + 1)
            })
            .collect::<String>()
    };
    let short = report_for(
        &state,
        11,
        &chain_schema,
        &chain(3),
        &[("http://example.org/c0", PERSON_IRI)],
    )
    .await;
    assert!(!short.conforms, "a short chain reaches its end and fails");
    let deep = report_for(
        &state,
        12,
        &chain_schema,
        &chain(30),
        &[("http://example.org/c0", PERSON_IRI)],
    )
    .await;
    assert!(deep.conforms, "depth above 40 assumes success");
}
