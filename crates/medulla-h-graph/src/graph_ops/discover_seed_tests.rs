//! Caller seeds and the per-hop filter of `Discover` (MEDULLA-FEDERATED-QUERY-R741.2 and
//! R741.3). The refusals name their code and hide their cause; the filter reaches the one
//! `expand_hops` call.

use super::discover_plan::{check_seeds, plan_discover, DeepArms};
use super::semantic::{discover, DiscoverCall};
use medulla_graph_core::graph::GraphCore;
use medulla_ingestion_types::ingestion_wire::{
    DiscoverPlanRequest, HopFilter, DISCOVER_SEEDS_MAX, HOP_FILTER_INVALID, SEED_LIMIT_EXCEEDED,
    SEED_NOT_VISIBLE,
};
use medulla_protocol_types::protocol::{Response, ResultPayload};
use medulla_query_types::wire::DiscoverResult;

fn props(value: serde_json::Value) -> Vec<u8> {
    rmp_serde::to_vec_named(&value).unwrap()
}

fn result_of(response: &Response) -> DiscoverResult {
    match response.result.as_ref() {
        Some(ResultPayload::Json(value)) => serde_json::from_value(value.clone()).unwrap(),
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(bytes).unwrap(),
        other => panic!("expected a discover answer, got {other:?} / {:?}", response.error),
    }
}

/// `a` and `b` are seeds; `a` cites `x` and knows `y`; `b` cites `z`.
fn fixture() -> GraphCore {
    let core = GraphCore::new();
    for id in ["a", "b", "x", "y", "z"] {
        core.add_node(id.into(), props(serde_json::json!({"type": "Doc", "name": id})));
    }
    for (from, to, label) in [("a", "x", "CITES"), ("a", "y", "KNOWS"), ("b", "z", "CITES")] {
        core.add_edge(
            from.into(),
            to.into(),
            props(serde_json::json!({"relationship": label})),
        )
        .unwrap();
    }
    core
}

fn ids_of(core: &GraphCore, seeds: &[&str], hops: u32, filter: &HopFilter) -> Vec<String> {
    let seeds: Vec<String> = seeds.iter().map(ToString::to_string).collect();
    let keywords: Vec<String> = Vec::new();
    let call = DiscoverCall {
        keywords: &keywords,
        query_embedding: &[],
        k: 32,
        hops,
        as_of: None,
        graph_name: "default",
        tenant: None,
        adapter_digest: None,
        quality_policy_ref: None,
        implicit_feedback: &[],
        allowed: None,
        scope: None,
        scope_digest: "",
        seeds: &seeds,
        hop_filter: filter,
    };
    let response = discover(core, &call, DeepArms::default(), 1);
    assert!(response.error.is_none(), "{:?}", response.error);
    result_of(&response)
        .hits
        .into_iter()
        .map(|hit| hit.id)
        .collect()
}

// spec: MEDULLA-FEDERATED-QUERY-R741.2
#[test]
fn each_refusal_names_its_code_and_hides_the_cause() {
    let core = fixture();

    // An absent seed and an empty one refuse byte-equal; neither names the id. A hidden
    // seed is absent from the caller's projected core, so it refuses the same way.
    let absent = check_seeds(&core, &["missing-one".to_string()]).unwrap_err();
    let hidden = check_seeds(&core, &["missing-two".to_string()]).unwrap_err();
    assert_eq!(absent.code, SEED_NOT_VISIBLE);
    assert_eq!(absent.message(), hidden.message());
    assert!(!absent.message().contains("missing"), "{}", absent.message());
    assert_eq!(
        check_seeds(&core, &[String::new()]).unwrap_err().message(),
        absent.message()
    );
    // One unreadable seed among readable ones refuses the request.
    assert!(check_seeds(&core, &["a".to_string(), "missing".to_string()]).is_err());
    assert!(check_seeds(&core, &["a".to_string(), "b".to_string()]).is_ok());

    // More than 64 seeds.
    let over = DiscoverPlanRequest {
        seed_ids: (0..=DISCOVER_SEEDS_MAX).map(|i| format!("n{i}")).collect(),
        ..Default::default()
    };
    let refused = plan_discover(&over, &[], 4, None).unwrap_err();
    assert_eq!(refused.code, SEED_LIMIT_EXCEEDED);

    // A list over 32 entries, an empty entry and a language that is not a BCP 47 tag.
    let invalid = |filter: HopFilter| {
        let request = DiscoverPlanRequest {
            hop_filter: Some(filter),
            ..Default::default()
        };
        plan_discover(&request, &[], 4, None).unwrap_err()
    };
    let long_list = invalid(HopFilter {
        node_labels: (0..33).map(|i| format!("L{i}")).collect(),
        ..Default::default()
    });
    assert_eq!(long_list.code, HOP_FILTER_INVALID);
    let empty_entry = invalid(HopFilter {
        edge_labels: vec!["CITES".into(), String::new()],
        ..Default::default()
    });
    assert_eq!(empty_entry.code, HOP_FILTER_INVALID);
    for language in ["", "e", "en_US", "en-", "1n", "en-toolongsubtag"] {
        let refused = invalid(HopFilter {
            language: Some(language.into()),
            ..Default::default()
        });
        assert_eq!(refused.code, HOP_FILTER_INVALID, "{language:?}");
    }

    // The valid forms pass.
    for language in ["en", "en-US", "zh-Hant-TW"] {
        let request = DiscoverPlanRequest {
            hop_filter: Some(HopFilter {
                language: Some(language.into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(plan_discover(&request, &[], 4, None), Ok(DeepArms::default()));
    }

    // With seeds the planner asks no provider, even in deep mode with no provider bound.
    let deep_seeded = DiscoverPlanRequest {
        query_text: Some("anything".into()),
        mode: medulla_ingestion_types::ingestion_wire::DiscoverMode::Deep,
        seed_ids: vec!["a".into()],
        ..Default::default()
    };
    assert_eq!(plan_discover(&deep_seeded, &[], 4, None), Ok(DeepArms::default()));
}

// spec: MEDULLA-FEDERATED-QUERY-R741.3
#[test]
fn discover_routes_filter_to_expand_hops() {
    let core = fixture();

    // Seeds replace the arms: they come back in the given order, score 1.0, no filter.
    assert_eq!(ids_of(&core, &["b", "a"], 0, &HopFilter::default()), ["b", "a"]);

    // An empty filter returns the walk as it was before the filter existed.
    let mut unfiltered = ids_of(&core, &["a", "b"], 1, &HopFilter::default());
    unfiltered.sort();
    assert_eq!(unfiltered, ["a", "b", "x", "y", "z"]);

    // An edge-label filter reaches the one expansion call: `y` (KNOWS) drops out.
    let cites = HopFilter {
        edge_labels: vec!["CITES".into()],
        ..Default::default()
    };
    let mut filtered = ids_of(&core, &["a", "b"], 1, &cites);
    filtered.sort();
    assert_eq!(filtered, ["a", "b", "x", "z"]);

    // The filter narrows neighbours only: the seeds themselves stay.
    let nothing = HopFilter {
        node_labels: vec!["Nothing".into()],
        ..Default::default()
    };
    let mut seeds_only = ids_of(&core, &["a", "b"], 1, &nothing);
    seeds_only.sort();
    assert_eq!(seeds_only, ["a", "b"]);
}
