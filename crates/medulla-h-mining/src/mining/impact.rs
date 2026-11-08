//! The probabilistic impact models of `MineRiskPropagation` (EH-526): noisy-OR
//! and independent cascade over `medulla_graph_algos::graph_algos::impact`, with hop
//! depth, the strongest paths, per-seed Shapley attribution, a provenance
//! digest, and `:ImpactAssessment` writeback that a WAL replay reproduces
//! (the request carries the scope, the as-of time and the Monte Carlo seed).

use super::*;
use super::{process::edge_indices, vector::link_writeback_source, writeback::*};
use medulla_graph_algos::{
    graph_algos::impact::independent_cascade, graph_algos::impact::noisy_or,
    graph_algos::impact::noisy_or_attribution, graph_algos::impact::strongest_paths,
    graph_algos::impact::CascadeSpec, graph_algos::impact::ImpactGraph,
    graph_algos::impact::ImpactPath, graph_algos::impact::Seed, graph_algos::impact::Semantics,
};
use medulla_protocol_types::result_contract::compute as results;
use medulla_query_types::solve_scalar::digest_text;
use medulla_result_types::{
    compute_result::mining::EcosystemQuery, compute_result::mining::EcosystemSeverity,
    compute_result::mining::ImpactOptions, compute_result::mining::ImpactPathRow,
    compute_result::mining::ImpactReport, compute_result::mining::ImpactSemantics,
    compute_result::mining::PackageImpactRow, compute_result::mining::RiskPropagationMiningResult,
    compute_result::mining::RiskScoreRow, compute_result::mining::SeedAttributionRow,
};

/// Domain of an impact run's provenance digest.
pub(crate) const IMPACT_DIGEST_DOMAIN: &str = "eg/impact-propagation/v1";
/// The kernel revision folded into every digest.
const KERNEL_REVISION: u32 = 1;
/// The scope of an assessment written without one.
const ADHOC_SCOPE: &str = "adhoc";

/// Which impact model runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ImpactKind {
    NoisyOr,
    IndependentCascade,
}

/// A model run: its rows, report and round count.
pub(crate) struct ImpactRun {
    pub(crate) rows: Vec<RiskScoreRow>,
    pub(crate) report: ImpactReport,
    pub(crate) rounds: usize,
}

/// The inputs an impact run is a function of.
pub(crate) struct ImpactInputs<'a> {
    pub(crate) nodes: &'a [String],
    pub(crate) seed: &'a [f64],
    pub(crate) edges: &'a [(String, String, f64)],
    pub(crate) kind: ImpactKind,
    pub(crate) options: &'a ImpactOptions,
    /// MEDULLA-SUPPLY-VOCAB-R009.3: the ecosystem finding, when the run walks
    /// the stored dependency graph.
    pub(crate) ecosystem: Option<&'a EcosystemQuery>,
}

impl ImpactInputs<'_> {
    fn seeds(&self) -> Vec<Seed> {
        self.seed
            .iter()
            .enumerate()
            .filter(|(node, p)| *node < self.nodes.len() && **p > 0.0)
            .map(|(node, p)| Seed {
                node,
                probability: *p,
            })
            .collect()
    }

    /// Provenance: the model, the options that change the answer, and every input.
    fn digest(&self) -> String {
        let o = self.options;
        digest_text(
            IMPACT_DIGEST_DOMAIN,
            &(
                KERNEL_REVISION,
                self.kind,
                (
                    o.hops,
                    o.samples,
                    o.rng_seed,
                    o.top_paths,
                    o.attribute_seeds,
                ),
                self.nodes,
                self.seed,
                self.edges,
            ),
        )
    }
}

/// Run one impact model.
pub(crate) fn run_impact(inputs: &ImpactInputs<'_>) -> ImpactRun {
    let graph = ImpactGraph::new(
        inputs.nodes.len(),
        &edge_indices(inputs.nodes, inputs.edges),
    );
    let seeds = inputs.seeds();
    let hops = inputs.options.hops;
    let paths = top_paths(&graph, &seeds, inputs);
    let targets: Vec<usize> = paths.iter().map(|(node, _)| *node).collect();
    let mut run = match inputs.kind {
        ImpactKind::NoisyOr => noisy_or_run(&graph, &seeds, &targets, inputs),
        ImpactKind::IndependentCascade => cascade_run(&graph, &seeds, &targets, inputs),
    };
    run.report.hops = hops;
    run.report.digest = inputs.digest();
    run.report.paths = paths
        .into_iter()
        .map(|(node, path)| path_row(inputs.nodes, node, path))
        .collect();
    run
}

/// The `top_paths` strongest paths into non-seed nodes, strongest first.
fn top_paths(
    graph: &ImpactGraph,
    seeds: &[Seed],
    inputs: &ImpactInputs<'_>,
) -> Vec<(usize, ImpactPath)> {
    let mut paths: Vec<(usize, ImpactPath)> = strongest_paths(graph, seeds, inputs.options.hops)
        .into_iter()
        .enumerate()
        .filter_map(|(node, path)| path.filter(|p| p.nodes.len() > 1).map(|p| (node, p)))
        .collect();
    paths.sort_by(|a, b| {
        b.1.probability
            .total_cmp(&a.1.probability)
            .then(a.0.cmp(&b.0))
    });
    paths.truncate(inputs.options.top_paths as usize);
    paths
}

fn path_row(nodes: &[String], node: usize, path: ImpactPath) -> ImpactPathRow {
    ImpactPathRow {
        node: nodes[node].clone(),
        path: path.nodes.iter().map(|&i| nodes[i].clone()).collect(),
        probability: path.probability,
    }
}

fn empty_report(semantics: ImpactSemantics, expected_spread: f64) -> ImpactReport {
    ImpactReport {
        semantics,
        digest: String::new(),
        hops: 0,
        expected_spread,
        spread_lower: None,
        spread_upper: None,
        paths: Vec::new(),
        attribution: Vec::new(),
        attribution_note: None,
        packages: Vec::new(),
    }
}

fn semantics_of(semantics: Semantics) -> ImpactSemantics {
    match semantics {
        Semantics::Exact => ImpactSemantics::Exact,
        Semantics::UpperBound => ImpactSemantics::UpperBound,
        Semantics::CyclicUnroll => ImpactSemantics::CyclicUnroll,
    }
}

fn score_rows(
    nodes: &[String],
    probability: &[f64],
    depth: &[Option<u32>],
    bounds: Option<(&[f64], &[f64])>,
) -> Vec<RiskScoreRow> {
    nodes
        .iter()
        .enumerate()
        .map(|(i, id)| RiskScoreRow {
            node: id.clone(),
            score: probability[i],
            hops: depth[i],
            lower: bounds.map(|(lower, _)| lower[i]),
            upper: bounds.map(|(_, upper)| upper[i]),
        })
        .collect()
}

fn noisy_or_run(
    graph: &ImpactGraph,
    seeds: &[Seed],
    targets: &[usize],
    inputs: &ImpactInputs<'_>,
) -> ImpactRun {
    let out = noisy_or(graph, seeds, inputs.options.hops);
    let rows = score_rows(inputs.nodes, &out.probability, &out.depth, None);
    let mut report = empty_report(semantics_of(out.semantics), out.probability.iter().sum());
    if inputs.options.attribute_seeds {
        match noisy_or_attribution(graph, seeds, targets, inputs.options.hops) {
            Some(table) => {
                report.attribution = attribution_rows(inputs.nodes, seeds, targets, &table)
            }
            None => {
                report.attribution_note = Some(format!(
                    "exact noisy-OR attribution is limited to {} seeds; use independent_cascade",
                    medulla_graph_algos::graph_algos::impact::MAX_EXACT_SEEDS
                ))
            }
        }
    }
    ImpactRun {
        rows,
        report,
        rounds: out.rounds as usize,
    }
}

fn cascade_run(
    graph: &ImpactGraph,
    seeds: &[Seed],
    targets: &[usize],
    inputs: &ImpactInputs<'_>,
) -> ImpactRun {
    let options = inputs.options;
    let attributed: &[usize] = if options.attribute_seeds {
        targets
    } else {
        &[]
    };
    let spec = CascadeSpec {
        hops: options.hops,
        samples: options.samples,
        seed: options.rng_seed,
    };
    let out = independent_cascade(graph, seeds, attributed, &spec);
    let depth = medulla_graph_algos::graph_algos::impact::hop_depths(graph, seeds, options.hops);
    let rows = score_rows(
        inputs.nodes,
        &out.probability,
        &depth,
        Some((&out.lower, &out.upper)),
    );
    let mut report = empty_report(ImpactSemantics::MonteCarlo, out.expected_spread);
    report.spread_lower = Some(out.spread_lower);
    report.spread_upper = Some(out.spread_upper);
    report.attribution = attribution_rows(inputs.nodes, seeds, attributed, &out.attribution);
    ImpactRun {
        rows,
        report,
        rounds: out.samples as usize,
    }
}

fn attribution_rows(
    nodes: &[String],
    seeds: &[Seed],
    targets: &[usize],
    table: &[Vec<f64>],
) -> Vec<SeedAttributionRow> {
    let mut rows = Vec::new();
    for (&target, shares) in targets.iter().zip(table) {
        for (seed, &shapley) in seeds.iter().zip(shares) {
            rows.push(SeedAttributionRow {
                node: nodes[target].clone(),
                seed: nodes[seed.node].clone(),
                shapley,
            });
        }
    }
    rows
}

/// Serve an impact model, writing `:ImpactAssessment` nodes on writeback.
pub(crate) fn handle_impact(
    req_id: u64,
    core: &GraphCore,
    inputs: &ImpactInputs<'_>,
    writeback: WritebackOptions,
) -> Response {
    if let Some(query) = inputs.ecosystem {
        return handle_ecosystem_impact(req_id, core, query);
    }
    let run = run_impact(inputs);
    let written = if writeback.enabled {
        materialize_assessments(core, inputs, &run)
    } else {
        0
    };
    #[cfg(feature = "epistemic")]
    if writeback.enabled && writeback.as_claim {
        materialize_assessment_claims(core, inputs, &run);
    }
    Response::ok(
        req_id,
        ResultPayload::of::<results::MineRiskPropagation>(RiskPropagationMiningResult {
            scores: run.rows,
            iterations: run.rounds,
            converged: true,
            written_back: written,
            impact: Some(run.report),
        }),
    )
}

/// Inclusive bounds of a request's `max_depth`, the same as the
/// `ecosystem.max_dependency_depth` knob.
pub(crate) const ECOSYSTEM_MAX_DEPTH_RANGE: std::ops::RangeInclusive<u32> = 1..=64;
/// Refusal of a finding against a package absent from the graph.
pub(crate) const ECOSYSTEM_PACKAGE_UNKNOWN: &str = "ECOSYSTEM_PACKAGE_UNKNOWN";
/// Refusal of a hop bound outside [`ECOSYSTEM_MAX_DEPTH_RANGE`].
pub(crate) const ECOSYSTEM_DEPTH_OUT_OF_RANGE: &str = "ECOSYSTEM_DEPTH_OUT_OF_RANGE";

fn core_severity(severity: EcosystemSeverity) -> medulla_core::ecosystem::Severity {
    use medulla_core::ecosystem::Severity;
    match severity {
        EcosystemSeverity::Low => Severity::Low,
        EcosystemSeverity::Medium => Severity::Medium,
        EcosystemSeverity::High => Severity::High,
        EcosystemSeverity::Critical => Severity::Critical,
    }
}

fn wire_severity(severity: medulla_core::ecosystem::Severity) -> EcosystemSeverity {
    use medulla_core::ecosystem::Severity;
    match severity {
        Severity::Low => EcosystemSeverity::Low,
        Severity::Medium => EcosystemSeverity::Medium,
        Severity::High => EcosystemSeverity::High,
        Severity::Critical => EcosystemSeverity::Critical,
    }
}

/// MEDULLA-SUPPLY-VOCAB-R009.3: propagate one finding from `query.package` to
/// every transitive dependent of the request graph. Writes nothing.
fn handle_ecosystem_impact(req_id: u64, core: &GraphCore, query: &EcosystemQuery) -> Response {
    let depth = match query.max_depth {
        Some(depth) if ECOSYSTEM_MAX_DEPTH_RANGE.contains(&depth) => depth,
        Some(depth) => {
            return Response::err(
                req_id,
                format!("{ECOSYSTEM_DEPTH_OUT_OF_RANGE}: max_depth {depth} is outside [1, 64]"),
            )
        }
        None => medulla_server_api::knob_config::knobs().ecosystem_max_dependency_depth as u32,
    };
    let finding = medulla_core::ecosystem::PackageFinding {
        id: query.finding_id.clone(),
        severity: core_severity(query.severity),
    };
    let impacts = match medulla_core::ecosystem::propagate_package_risk_with(
        core,
        &query.package,
        &finding,
        depth as usize,
        medulla_core::ecosystem::inherit_severity,
    ) {
        Ok(impacts) => impacts,
        Err(medulla_core::ecosystem::PackageRefusal::UnknownPackage(package)) => {
            return Response::err(
                req_id,
                format!("{ECOSYSTEM_PACKAGE_UNKNOWN}: no package '{package}' in the graph"),
            )
        }
    };
    let packages: Vec<PackageImpactRow> = impacts
        .into_iter()
        .map(|impact| PackageImpactRow {
            package: impact.package,
            severity: wire_severity(impact.severity),
            path: impact.path,
        })
        .collect();
    let rows = packages
        .iter()
        .map(|row| RiskScoreRow {
            node: row.package.clone(),
            score: 1.0,
            hops: Some(row.path.len().saturating_sub(1) as u32),
            lower: None,
            upper: None,
        })
        .collect();
    let mut report = empty_report(ImpactSemantics::Exact, packages.len() as f64);
    report.hops = depth;
    report.digest = digest_text(
        IMPACT_DIGEST_DOMAIN,
        &serde_json::json!({
            "kernel": KERNEL_REVISION,
            "ecosystem": query,
            "depth": depth,
            "packages": packages,
        }),
    );
    report.packages = packages;
    Response::ok(
        req_id,
        ResultPayload::of::<results::MineRiskPropagation>(RiskPropagationMiningResult {
            scores: rows,
            iterations: 1,
            converged: true,
            written_back: 0,
            impact: Some(report),
        }),
    )
}

/// The assessment node id of `node` under `options.scope`.
pub(crate) fn assessment_node_id(options: &ImpactOptions, node: &str) -> String {
    WritebackNodeId::new("impact_assessment", &[scope_of(options), node]).into_string()
}

fn scope_of(options: &ImpactOptions) -> &str {
    if options.scope.is_empty() {
        ADHOC_SCOPE
    } else {
        &options.scope
    }
}

/// The rows an assessment is written for: the `assess` list (zero included,
/// so a cleared impact is recorded), else every hit node.
fn assessed<'r>(options: &ImpactOptions, rows: &'r [RiskScoreRow]) -> Vec<&'r RiskScoreRow> {
    rows.iter()
        .filter(|row| {
            if options.assess.is_empty() {
                row.score > 0.0
            } else {
                options.assess.contains(&row.node)
            }
        })
        .collect()
}

/// Materialize one `:ImpactAssessment` per assessed node, linked `ASSESSES`.
pub(crate) fn materialize_assessments(
    core: &GraphCore,
    inputs: &ImpactInputs<'_>,
    run: &ImpactRun,
) -> usize {
    let options = inputs.options;
    let seeds: Vec<&str> = inputs
        .seeds()
        .iter()
        .map(|s| inputs.nodes[s.node].as_str())
        .collect();
    let mut written = 0usize;
    for row in assessed(options, &run.rows) {
        let node_id = assessment_node_id(options, &row.node);
        let props = serde_json::json!({
            "type": "ImpactAssessment",
            "of": row.node,
            "scope": scope_of(options),
            "model": inputs.kind,
            "semantics": run.report.semantics,
            "probability": row.score,
            "hops": row.hops,
            "lower": row.lower,
            "upper": row.upper,
            "seeds": seeds,
            "digest": run.report.digest,
            "as_of_ms": options.as_of_ms,
        });
        if writeback_node(core, &node_id, &props) {
            link_writeback_source(core, &node_id, &row.node, "ASSESSES");
            written += 1;
        }
    }
    written
}

/// A `:Claim` per assessment, its confidence the impact probability.
#[cfg(feature = "epistemic")]
pub(crate) fn materialize_assessment_claims(
    core: &GraphCore,
    inputs: &ImpactInputs<'_>,
    run: &ImpactRun,
) {
    for row in assessed(inputs.options, &run.rows) {
        materialize_claim(
            core,
            &assessment_node_id(inputs.options, &row.node),
            "impact_propagation",
            row.score.clamp(0.0, 1.0),
            &format!("impact:{}", scope_of(inputs.options)),
        );
    }
}

#[cfg(test)]
mod tests;
