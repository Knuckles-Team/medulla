//! `PROPAGATE ... RISK HISTORY` (MEDULLA-FEDERATED-QUERY-R032.100): the layer- and
//! history-weighted impact of each seed row, in both dependency directions.
//!
//! The failure history is graph facts on the caller's pinned, row-projected snapshot: a
//! visible `:Observation` node with `dataset` equal to the stage's dataset id, an
//! `observedEntity` edge to the observed node, `observationTime` (Unix ms) inside the
//! window `[from_ms, to_ms)` and `outcome` equal to `failure` or `success`. Any other
//! `outcome` counts in neither and is reported as a `HISTORY_MALFORMED` warning.
//!
//! Per node: `failure_rate = failures / observations` and
//! `layer_contribution = layer_weight * failure_rate` (a layer outside the taxonomy weighs 0).
//! Per path (no node repeats): `path_risk = 1 - product(1 - layer_contribution)` over the
//! path's nodes, the seed included. A node reached by several paths keeps the maximum
//! `path_risk`, ties broken by fewer hops, then node id, then the edge-id sequence. A node
//! with no observation is `Coverage::Unknown`, contributes nothing and makes the aggregate
//! null with `partial: true`.

use std::collections::{BTreeMap, HashMap};

use medulla_graph_core::graph::GraphView;
use medulla_query_types::impact_risk::{Coverage, HistoricalRisk, NodeRisk};
use medulla_query_types::wire::{EdgeDir, Op, Pred, RiskHistorySpec};
use medulla_types::contract::Digest256;
use petgraph::stable_graph::NodeIndex;

use super::expand::{admitted_edges, EdgeFilter};
use super::propagate::node_props;
use super::PlanCtx;
use crate::budget::Budget;
use crate::rowset::RowSet;

/// The observation label.
const OBSERVATION_LABEL: &str = "Observation";
/// The edge from an observation to the node it observed.
const OBSERVED_ENTITY: &str = "observedEntity";

/// What the history says about one node.
#[derive(Clone, Default)]
struct NodeHistory {
    observations: u64,
    failures: u64,
}

/// The counted history of the window, with the digest over it.
struct History {
    nodes: HashMap<String, NodeHistory>,
    dataset_digest: String,
    warnings: Vec<String>,
}

fn framed(domain: &[u8], fields: &[&[u8]]) -> Result<String, String> {
    Digest256::framed(domain, fields).map(|digest| digest.to_hex())
}

/// The window's observations of the dataset, read from the pinned snapshot.
fn load_history(view: &GraphView, spec: &RiskHistorySpec) -> Result<History, String> {
    let mut nodes: HashMap<String, NodeHistory> = HashMap::new();
    let mut counted: Vec<String> = Vec::new();
    let mut warnings = Vec::new();
    let mut in_window = false;
    let observation_filter = EdgeFilter {
        rel: Some(OBSERVED_ENTITY),
        preds: &[],
        pin: None,
    };
    let mut ids: Vec<&String> = view.node_properties.keys().collect();
    ids.sort_unstable();
    for id in ids {
        let props = node_props(view, id);
        if !medulla_graph_core::graph::node_carries_label(
            &serde_json::Value::Object(props.clone()),
            OBSERVATION_LABEL,
        ) {
            continue;
        }
        if props.get("dataset").and_then(serde_json::Value::as_str) != Some(spec.dataset.as_str()) {
            continue;
        }
        let Some(at) = props
            .get("observationTime")
            .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
        else {
            warnings.push(format!(
                "HISTORY_MALFORMED: observation `{id}` has no numeric observationTime"
            ));
            continue;
        };
        if at < spec.from_ms || at >= spec.to_ms {
            continue;
        }
        in_window = true;
        let failed = match props.get("outcome").and_then(serde_json::Value::as_str) {
            Some("failure") => true,
            Some("success") => false,
            other => {
                warnings.push(format!(
                    "HISTORY_MALFORMED: observation `{id}` has outcome {:?}, not failure or success",
                    other.unwrap_or("<absent>")
                ));
                continue;
            }
        };
        let Some(&index) = view.node_map.get(id) else {
            continue;
        };
        let observed = admitted_edges(view, index, EdgeDir::Out, &observation_filter);
        let Some((target, _)) = observed.first() else {
            warnings.push(format!(
                "HISTORY_MALFORMED: observation `{id}` has no observedEntity edge"
            ));
            continue;
        };
        let entry = nodes.entry(view.graph[*target].clone()).or_default();
        entry.observations += 1;
        entry.failures += u64::from(failed);
        counted.push(id.clone());
    }
    if !in_window {
        // The same refusal answers an absent dataset and one the caller cannot see.
        return Err(format!(
            "HISTORY_DATASET_UNAVAILABLE: no visible observation of dataset `{}` in the window",
            spec.dataset
        ));
    }
    let (from, to) = (spec.from_ms.to_be_bytes(), spec.to_ms.to_be_bytes());
    let mut fields: Vec<&[u8]> = vec![spec.dataset.as_bytes(), &from, &to];
    fields.extend(counted.iter().map(|id| id.as_bytes()));
    let dataset_digest = framed(b"eg/risk-history-dataset/v1", &fields)?;
    Ok(History {
        nodes,
        dataset_digest,
        warnings,
    })
}

/// One node's own numbers.
struct Own {
    layer: Option<String>,
    observations: u64,
    failures: u64,
    contribution: Option<f64>,
}

fn own(view: &GraphView, history: &History, spec: &RiskHistorySpec, node: &str) -> Own {
    let layer = node_props(view, node)
        .get("layer")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let stats = history.nodes.get(node).cloned().unwrap_or_default();
    let weight = layer
        .as_deref()
        .and_then(|name| spec.layers.iter().find(|l| l.layer == name))
        .map_or(0.0, |l| l.weight);
    let contribution = (stats.observations > 0)
        .then(|| weight * (stats.failures as f64 / stats.observations as f64));
    Own {
        layer,
        observations: stats.observations,
        failures: stats.failures,
        contribution,
    }
}

/// The best path found so far to one node.
#[derive(Clone)]
struct Best {
    path: Vec<String>,
    edge_ids: Vec<String>,
    risk: f64,
}

impl Best {
    /// Whether `self` is a better path than `other` to the same node: larger risk, then
    /// fewer hops, then the smaller edge-id sequence (the node id is the same).
    fn beats(&self, other: &Best) -> bool {
        self.risk
            .total_cmp(&other.risk)
            .then_with(|| other.edge_ids.len().cmp(&self.edge_ids.len()))
            .then_with(|| other.edge_ids.cmp(&self.edge_ids))
            .is_gt()
    }
}

struct Walk<'a> {
    view: &'a GraphView,
    filter: &'a EdgeFilter<'a>,
    dir: EdgeDir,
    hops: usize,
    budget: &'a Budget,
    contribution: &'a HashMap<NodeIndex, Option<f64>>,
    best: BTreeMap<String, Best>,
    expanded: usize,
}

impl Walk<'_> {
    fn go(
        &mut self,
        node: NodeIndex,
        path: &mut Vec<NodeIndex>,
        edge_ids: &mut Vec<String>,
        survival: f64,
    ) -> Result<(), String> {
        if path.len() > self.hops {
            return Ok(());
        }
        for (next, _) in admitted_edges(self.view, node, self.dir, self.filter) {
            if path.contains(&next) {
                continue;
            }
            self.expanded += 1;
            self.budget.check_traversal(self.expanded)?;
            let survival = survival * (1.0 - self.contribution[&next].unwrap_or(0.0));
            path.push(next);
            edge_ids.push(format!(
                "{}->{}",
                self.view.graph[node], self.view.graph[next]
            ));
            let candidate = Best {
                path: path.iter().map(|n| self.view.graph[*n].clone()).collect(),
                edge_ids: edge_ids.clone(),
                risk: 1.0 - survival,
            };
            let id = self.view.graph[next].clone();
            match self.best.get(&id) {
                Some(current) if !candidate.beats(current) => {}
                _ => {
                    self.best.insert(id, candidate);
                }
            }
            self.go(next, path, edge_ids, survival)?;
            path.pop();
            edge_ids.pop();
        }
        Ok(())
    }
}

fn reversed(dir: EdgeDir) -> EdgeDir {
    match dir {
        EdgeDir::Out => EdgeDir::In,
        EdgeDir::In => EdgeDir::Out,
        EdgeDir::Both => EdgeDir::Both,
    }
}

/// The historical risk of every seed, with the warnings the history raised.
pub(crate) struct RiskReport {
    pub(crate) risks: Vec<HistoricalRisk>,
    pub(crate) warnings: Vec<String>,
}

#[allow(clippy::too_many_arguments)]
fn risk_report(
    view: &GraphView,
    seeds: &[String],
    spec: &RiskHistorySpec,
    filter: &EdgeFilter<'_>,
    dir: EdgeDir,
    hops: usize,
    budget: &Budget,
) -> Result<RiskReport, String> {
    let history = load_history(view, spec)?;
    let mut owns: HashMap<NodeIndex, Own> = HashMap::new();
    for (name, index) in &view.node_map {
        owns.insert(*index, own(view, &history, spec, name));
    }
    let contribution: HashMap<NodeIndex, Option<f64>> =
        owns.iter().map(|(i, o)| (*i, o.contribution)).collect();
    let mut risks = Vec::new();
    for seed in seeds {
        let Some(&start) = view.node_map.get(seed) else {
            continue;
        };
        let seed_survival = 1.0 - contribution[&start].unwrap_or(0.0);
        let mut sides: Vec<Vec<NodeRisk>> = Vec::new();
        for walked in [dir, reversed(dir)] {
            let mut walk = Walk {
                view,
                filter,
                dir: walked,
                hops,
                budget,
                contribution: &contribution,
                best: BTreeMap::new(),
                expanded: 0,
            };
            walk.go(start, &mut vec![start], &mut Vec::new(), seed_survival)?;
            let nodes = walk
                .best
                .into_iter()
                .map(|(node_id, best)| {
                    let index = view.node_map[&node_id];
                    let own = &owns[&index];
                    let known = own.observations > 0;
                    NodeRisk {
                        node_id,
                        path: best.path,
                        edge_ids: best.edge_ids,
                        layer: own.layer.clone(),
                        observations: own.observations,
                        failures: own.failures,
                        layer_contribution: own.contribution,
                        path_risk: known.then_some(best.risk),
                        coverage: if known {
                            Coverage::Known
                        } else {
                            Coverage::Unknown
                        },
                    }
                })
                .collect();
            sides.push(nodes);
        }
        let downstream = sides.remove(0);
        let upstream = sides.remove(0);
        let seed_known = owns[&start].observations > 0;
        let partial = !seed_known
            || downstream
                .iter()
                .chain(&upstream)
                .any(|n| n.coverage == Coverage::Unknown);
        let aggregate_risk = if partial {
            None
        } else {
            downstream
                .iter()
                .chain(&upstream)
                .filter_map(|n| n.path_risk)
                .reduce(f64::max)
                .or(Some(1.0 - seed_survival))
        };
        let hop_bytes = (hops as u64).to_be_bytes();
        let weights: Vec<[u8; 8]> = spec
            .layers
            .iter()
            .map(|l| l.weight.to_bits().to_be_bytes())
            .collect();
        let mut fields: Vec<&[u8]> = vec![
            seed.as_bytes(),
            &hop_bytes,
            history.dataset_digest.as_bytes(),
        ];
        for (layer, weight) in spec.layers.iter().zip(&weights) {
            fields.push(layer.layer.as_bytes());
            fields.push(weight);
        }
        let request_digest = framed(b"eg/risk-history-request/v1", &fields)?;
        risks.push(HistoricalRisk {
            target: seed.clone(),
            graph_revision: 0,
            hops,
            layers: spec.layers.clone(),
            dataset: spec.dataset.clone(),
            from_ms: spec.from_ms,
            to_ms: spec.to_ms,
            dataset_digest: history.dataset_digest.clone(),
            upstream,
            downstream,
            aggregate_risk,
            partial,
            request_digest,
        });
    }
    Ok(RiskReport {
        risks,
        warnings: history.warnings,
    })
}

/// `historical_risk(view, seeds, spec, rel, dir, edge_preds, hops)`: one [`HistoricalRisk`]
/// per seed.
pub fn historical_risk(
    view: &GraphView,
    seeds: &[String],
    spec: &RiskHistorySpec,
    rel: Option<&str>,
    dir: EdgeDir,
    edge_preds: &[Pred],
    hops: usize,
) -> Result<Vec<HistoricalRisk>, String> {
    let filter = EdgeFilter {
        rel,
        preds: edge_preds,
        pin: None,
    };
    risk_report(view, seeds, spec, &filter, dir, hops, &Budget::default()).map(|r| r.risks)
}

/// The risks of a `PROPAGATE ... RISK HISTORY` stage and the warnings raised while computing them.
pub type StageReport = (Vec<HistoricalRisk>, Vec<String>);

/// The risk report of a `PROPAGATE` stage that carries `RISK HISTORY` over its input rows;
/// `None` for any other op or a `PROPAGATE` without the tail.
pub fn stage_report(op: &Op, input: &RowSet, ctx: &PlanCtx) -> Result<Option<StageReport>, String> {
    let Op::Propagate {
        rel,
        dir,
        edge_preds,
        hops,
        risk_history: Some(spec),
        ..
    } = op
    else {
        return Ok(None);
    };
    let filter = EdgeFilter {
        rel: rel.as_deref(),
        preds: edge_preds,
        pin: ctx.as_of_pin,
    };
    let seeds = input.ids();
    let report = risk_report(ctx.view, &seeds, spec, &filter, *dir, *hops, &ctx.budget)?;
    Ok(Some((report.risks, report.warnings)))
}

/// The `history_risk` channel of the output rows: each node's largest `path_risk` over the
/// seeds' entries (a node with no known risk is absent).
pub(crate) fn history_channel(risks: &[HistoricalRisk]) -> Vec<(String, f32)> {
    let mut best: BTreeMap<&str, f64> = BTreeMap::new();
    for risk in risks {
        for node in risk.downstream.iter().chain(&risk.upstream) {
            if let Some(value) = node.path_risk {
                let slot = best.entry(node.node_id.as_str()).or_insert(value);
                *slot = slot.max(value);
            }
        }
    }
    best.into_iter()
        .map(|(id, v)| (id.to_string(), v as f32))
        .collect()
}

#[cfg(test)]
mod tests {
    use medulla_graph_core::graph::GraphCore;
    use medulla_query_types::wire::{LayerWeight, RiskHistorySpec};

    use super::*;

    fn blob(value: serde_json::Value) -> Vec<u8> {
        rmp_serde::to_vec_named(&value).unwrap()
    }

    fn spec() -> RiskHistorySpec {
        RiskHistorySpec {
            dataset: "deploys".into(),
            from_ms: 0,
            to_ms: 1_000,
            layers: vec![
                LayerWeight {
                    layer: "service".into(),
                    weight: 1.0,
                },
                LayerWeight {
                    layer: "os".into(),
                    weight: 0.5,
                },
            ],
        }
    }

    /// a -> b -> c -> a (a cycle) with layers service/os/service.
    fn graph(observe_c: bool) -> GraphView {
        let core = GraphCore::new();
        for (id, layer) in [("a", "service"), ("b", "os"), ("c", "service")] {
            core.add_node(
                id.into(),
                blob(serde_json::json!({"node_type": "Service", "layer": layer})),
            );
        }
        for (from, to) in [("a", "b"), ("b", "c"), ("c", "a")] {
            core.add_edge(
                from.into(),
                to.into(),
                blob(serde_json::json!({"relationship": "dependsOn"})),
            )
            .unwrap();
        }
        let mut observations = vec![("a", "failure"), ("a", "success"), ("b", "failure")];
        if observe_c {
            observations.push(("c", "success"));
        }
        for (i, (target, outcome)) in observations.into_iter().enumerate() {
            let id = format!("obs-{i}");
            core.add_node(
                id.clone(),
                blob(serde_json::json!({
                    "node_type": "Observation", "dataset": "deploys",
                    "observationTime": 10 + i as i64, "outcome": outcome,
                })),
            );
            core.add_edge(
                id,
                target.into(),
                blob(serde_json::json!({"relationship": "observedEntity"})),
            )
            .unwrap();
        }
        core.analysis_snapshot()
    }

    // spec: MEDULLA-FEDERATED-QUERY-R032.100
    #[test]
    fn path_risk_and_cycles() {
        let view = graph(true);
        let risks = historical_risk(
            &view,
            &["a".to_string()],
            &spec(),
            Some("dependsOn"),
            EdgeDir::Out,
            &[],
            4,
        )
        .unwrap();
        let risk = &risks[0];
        assert_eq!(risk.target, "a");
        // a: 1 failure of 2 at weight 1.0 = 0.5; b: 1 of 1 at weight 0.5 = 0.5; c: 0 of 1.
        let b = risk.downstream.iter().find(|n| n.node_id == "b").unwrap();
        assert_eq!(b.layer_contribution, Some(0.5));
        assert_eq!(b.path_risk, Some(1.0 - 0.5 * 0.5));
        // The cycle never repeats a node: the seed is not reached again.
        assert!(risk.downstream.iter().all(|n| n.node_id != "a"));
        let c = risk.downstream.iter().find(|n| n.node_id == "c").unwrap();
        assert_eq!(c.path, vec!["a", "b", "c"]);
        assert_eq!(c.path_risk, Some(1.0 - 0.5 * 0.5 * 1.0));
        // Upstream walks the reversed edges: c then b.
        let up_c = risk.upstream.iter().find(|n| n.node_id == "c").unwrap();
        assert_eq!(up_c.path, vec!["a", "c"]);
        assert!(!risk.partial);
        assert_eq!(risk.aggregate_risk, Some(0.75));
        assert_eq!(risk.dataset_digest.len(), 64);
        assert_eq!(risk.request_digest.len(), 64);
    }

    // spec: MEDULLA-FEDERATED-QUERY-R032.100
    #[test]
    fn unobserved_node_is_partial_and_missing_dataset_refuses() {
        let view = graph(false);
        let risks = historical_risk(
            &view,
            &["a".to_string()],
            &spec(),
            Some("dependsOn"),
            EdgeDir::Out,
            &[],
            4,
        )
        .unwrap();
        let c = risks[0]
            .downstream
            .iter()
            .find(|n| n.node_id == "c")
            .unwrap();
        assert_eq!(c.coverage, Coverage::Unknown);
        assert_eq!(c.path_risk, None);
        assert!(risks[0].partial);
        assert_eq!(risks[0].aggregate_risk, None);

        let mut missing = spec();
        missing.dataset = "nope".into();
        let error = historical_risk(
            &view,
            &["a".to_string()],
            &missing,
            None,
            EdgeDir::Out,
            &[],
            4,
        )
        .unwrap_err();
        assert!(error.starts_with("HISTORY_DATASET_UNAVAILABLE"), "{error}");
    }
}
