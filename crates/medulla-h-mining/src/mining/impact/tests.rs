//! The served impact models: `MineRiskPropagation` with `noisy_or` /
//! `independent_cascade`, their `:ImpactAssessment` writeback, and the WAL
//! replay reproducing the same facts.

use std::sync::Arc;

use medulla_result_types::{
    compute_result::mining::EcosystemQuery, compute_result::mining::EcosystemSeverity,
    compute_result::mining::ImpactOptions, compute_result::mining::RiskModel,
};

use super::super::{dispatch_for_test, replay};
use medulla_graph_core::graph::GraphCore;
use medulla_protocol_types::protocol::Method;
use medulla_protocol_types::protocol::ResultPayload;

fn method(model: RiskModel, writeback: bool) -> Method {
    // svc-a <- db, svc-b <- db, api <- svc-a, api <- svc-b: an incident on db.
    Method::MineRiskPropagation {
        nodes: ["db", "svc-a", "svc-b", "api"].map(String::from).to_vec(),
        seed: vec![1.0, 0.0, 0.0, 0.0],
        edges: vec![
            ("db".into(), "svc-a".into(), 0.8),
            ("db".into(), "svc-b".into(), 0.5),
            ("svc-a".into(), "api".into(), 0.9),
            ("svc-b".into(), "api".into(), 0.9),
        ],
        damping: 0.85,
        tolerance: 1e-9,
        max_iterations: 100,
        model,
        writeback,
        #[cfg(feature = "epistemic")]
        as_claim: false,
    }
}

fn options(scope: &str) -> ImpactOptions {
    ImpactOptions {
        scope: scope.into(),
        as_of_ms: 1_700_000_000_000,
        attribute_seeds: true,
        ..ImpactOptions::default()
    }
}

fn json(core: &Arc<GraphCore>, method: Method) -> serde_json::Value {
    let resp = dispatch_for_test(7, Arc::clone(core), method).expect("a mining method");
    let Some(ResultPayload::Json(v)) = resp.result else {
        panic!("expected a JSON result: {:?}", resp.error);
    };
    v
}

fn assessments(core: &GraphCore) -> Vec<(String, serde_json::Value)> {
    core.mark_dirty();
    let mut rows: Vec<(String, serde_json::Value)> = core
        .get_nodes_by_label("ImpactAssessment", 0)
        .into_iter()
        .map(|(id, blob)| (id, rmp_serde::from_slice(&blob).expect("assessment props")))
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

#[test]
fn noisy_or_reports_probabilities_semantics_paths_and_attribution() {
    let core = Arc::new(GraphCore::new());
    let v = json(&core, method(RiskModel::NoisyOr(options("w1")), false));
    let score = |i: usize| v["scores"][i]["score"].as_f64().unwrap();
    assert_eq!(score(0), 1.0);
    assert!((score(1) - 0.8).abs() < 1e-12);
    // Shared ancestry (db) makes the noisy-OR an upper bound at api.
    let api = 1.0 - (1.0 - 0.9 * 0.8) * (1.0 - 0.9 * 0.5);
    assert!((score(3) - api).abs() < 1e-12);
    assert_eq!(v["impact"]["semantics"], "upper_bound");
    assert!(v["impact"]["digest"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
    assert_eq!(v["scores"][3]["hops"], 2);
    let paths = v["impact"]["paths"].as_array().unwrap();
    assert_eq!(paths[0]["node"], "svc-a");
    assert_eq!(
        v["impact"]["paths"][1]["path"],
        serde_json::json!(["db", "svc-a", "api"])
    );
    let attributed: f64 = v["impact"]["attribution"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["node"] == "api")
        .map(|row| row["shapley"].as_f64().unwrap())
        .sum();
    assert!((attributed - api).abs() < 1e-12, "one seed carries it all");
}

#[test]
fn share_output_is_unchanged_mass_share() {
    let core = Arc::new(GraphCore::new());
    let v = json(&core, method(RiskModel::Share, false));
    let total: f64 = (0..4)
        .map(|i| v["scores"][i]["score"].as_f64().unwrap())
        .sum();
    assert!((total - 1.0).abs() < 1e-6, "PPR shares sum to one");
    assert!(v.get("impact").is_none());
    assert!(v["scores"][0].get("hops").is_none());
}

#[test]
fn writeback_is_reproduced_by_replay() {
    let model = RiskModel::IndependentCascade(ImpactOptions {
        samples: 500,
        rng_seed: 5,
        ..options("watch-1")
    });
    let served = Arc::new(GraphCore::new());
    let v = json(&served, method(model.clone(), true));
    assert_eq!(v["written_back"], 4);
    assert_eq!(v["impact"]["semantics"], "monte_carlo");
    let written = assessments(&served);
    assert_eq!(written.len(), 4);
    let db = written.iter().find(|(_, p)| p["of"] == "db").unwrap();
    assert_eq!(db.1["scope"], "watch-1");
    assert_eq!(db.1["as_of_ms"], 1_700_000_000_000u64);
    assert_eq!(db.1["seeds"], serde_json::json!(["db"]));
    let replayed = Arc::new(GraphCore::new());
    replay(&replayed, &method(model, true));
    assert_eq!(assessments(&replayed), written);
}

#[test]
fn assess_limits_writeback_and_records_a_cleared_node() {
    let core = Arc::new(GraphCore::new());
    let mut cleared = method(
        RiskModel::NoisyOr(ImpactOptions {
            assess: vec!["api".into()],
            ..options("w2")
        }),
        true,
    );
    if let Method::MineRiskPropagation { seed, .. } = &mut cleared {
        *seed = vec![0.0; 4];
    }
    let v = json(&core, cleared);
    assert_eq!(v["written_back"], 1);
    let written = assessments(&core);
    assert_eq!(written[0].1["of"], "api");
    assert_eq!(written[0].1["probability"], 0.0);
}

// ── Ecosystem package risk (MEDULLA-SUPPLY-VOCAB-R009.3 / R009.6) ──────────

fn dependency_graph(edges: &[(&str, &str)]) -> Arc<GraphCore> {
    let core = Arc::new(GraphCore::new());
    let mut ids: Vec<&str> = edges.iter().flat_map(|(a, b)| [*a, *b]).collect();
    ids.sort_unstable();
    ids.dedup();
    for id in ids {
        core.add_node(
            id.into(),
            rmp_serde::to_vec_named(&serde_json::json!({"type": "EcosystemPackage"})).unwrap(),
        );
    }
    for (dependent, dependency) in edges {
        core.add_edge(
            (*dependent).into(),
            (*dependency).into(),
            rmp_serde::to_vec_named(&serde_json::json!({"relationship": "DEPENDS_ON"})).unwrap(),
        )
        .unwrap();
    }
    core
}

fn ecosystem_method(package: &str, severity: EcosystemSeverity) -> Method {
    let options = ImpactOptions {
        ecosystem: Some(EcosystemQuery {
            package: package.into(),
            finding_id: "finding-1".into(),
            severity,
            max_depth: None,
        }),
        ..ImpactOptions::default()
    };
    Method::MineRiskPropagation {
        nodes: Vec::new(),
        seed: Vec::new(),
        edges: Vec::new(),
        damping: 0.85,
        tolerance: 1e-9,
        max_iterations: 100,
        model: RiskModel::NoisyOr(options),
        writeback: false,
        #[cfg(feature = "epistemic")]
        as_claim: false,
    }
}

fn error_of(core: &Arc<GraphCore>, method: Method) -> String {
    let resp = dispatch_for_test(9, Arc::clone(core), method).expect("a mining method");
    assert!(resp.result.is_none(), "expected a refusal");
    format!("{:?}", resp.error)
}

fn packages(v: &serde_json::Value) -> Vec<(String, String, Vec<String>)> {
    v["impact"]["packages"]
        .as_array()
        .expect("packages listed")
        .iter()
        .map(|row| {
            (
                row["package"].as_str().unwrap().to_string(),
                row["severity"].as_str().unwrap().to_string(),
                row["path"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| p.as_str().unwrap().to_string())
                    .collect(),
            )
        })
        .collect()
}

// spec: MEDULLA-SUPPLY-VOCAB-R009.3
#[test]
fn served_impact_lists_dependents() {
    // Tenant A's graph: app -> lib -> core, tool -> core.
    let tenant_a = dependency_graph(&[("lib", "core"), ("app", "lib"), ("tool", "core")]);
    let v = json(&tenant_a, ecosystem_method("core", EcosystemSeverity::High));
    let path = |ids: &[&str]| ids.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(
        packages(&v),
        vec![
            ("lib".into(), "high".into(), path(&["core", "lib"])),
            ("tool".into(), "high".into(), path(&["core", "tool"])),
            ("app".into(), "high".into(), path(&["core", "lib", "app"])),
        ]
    );
    assert_eq!(v["written_back"], 0, "an ecosystem run writes nothing");
    assert_eq!(v["impact"]["hops"], 8, "the knob default bounds the walk");
    // An unknown package is the typed refusal.
    let refused = error_of(
        &tenant_a,
        ecosystem_method("absent", EcosystemSeverity::Low),
    );
    assert!(refused.contains("ECOSYSTEM_PACKAGE_UNKNOWN"), "{refused}");
    // Cross-tenant: tenant B's graph never sees tenant A's packages.
    let tenant_b = dependency_graph(&[("other", "base")]);
    let refused = error_of(&tenant_b, ecosystem_method("core", EcosystemSeverity::High));
    assert!(refused.contains("ECOSYSTEM_PACKAGE_UNKNOWN"), "{refused}");
    // A hop bound outside [1, 64] is refused.
    let mut deep = ecosystem_method("core", EcosystemSeverity::High);
    if let Method::MineRiskPropagation {
        model: RiskModel::NoisyOr(options),
        ..
    } = &mut deep
    {
        options.ecosystem.as_mut().unwrap().max_depth = Some(65);
    }
    let refused = error_of(&tenant_a, deep);
    assert!(
        refused.contains("ECOSYSTEM_DEPTH_OUT_OF_RANGE"),
        "{refused}"
    );
    // Without an ecosystem query, empty `nodes` is still refused.
    let refused = error_of(
        &tenant_a,
        Method::MineRiskPropagation {
            nodes: Vec::new(),
            seed: Vec::new(),
            edges: Vec::new(),
            damping: 0.85,
            tolerance: 1e-9,
            max_iterations: 100,
            model: RiskModel::NoisyOr(ImpactOptions::default()),
            writeback: false,
            #[cfg(feature = "epistemic")]
            as_claim: false,
        },
    );
    assert!(refused.contains("non-empty `nodes`"), "{refused}");
}

// spec: MEDULLA-SUPPLY-VOCAB-R009.6
#[test]
fn served_impact_over_dependency_chain() {
    // Five packages with one diamond: b and c both depend on a; d depends on
    // both; e depends on d.
    let core = dependency_graph(&[("b", "a"), ("c", "a"), ("d", "b"), ("d", "c"), ("e", "d")]);
    let v = json(&core, ecosystem_method("a", EcosystemSeverity::Critical));
    let rows = packages(&v);
    let reached: Vec<&str> = rows.iter().map(|(p, _, _)| p.as_str()).collect();
    assert_eq!(reached, ["b", "c", "d", "e"], "every transitive dependent");
    assert!(rows.iter().all(|(_, s, _)| s == "critical"));
    let path_of = |id: &str| rows.iter().find(|(p, _, _)| p == id).unwrap().2.clone();
    // The diamond is reached once, along its shortest (first) path.
    assert_eq!(path_of("d"), ["a", "b", "d"]);
    assert_eq!(path_of("e"), ["a", "b", "d", "e"]);
    assert_eq!(v["scores"].as_array().unwrap().len(), 4);
    assert_eq!(v["scores"][3]["hops"], 3);
}
