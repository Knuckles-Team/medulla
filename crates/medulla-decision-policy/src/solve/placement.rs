//! Service-to-host placement as a bounded 0-1 program (MEDULLA-PLACEMENT-R700.2).
//!
//! [`plan_services`] assigns every service to exactly one host so that each
//! host's CPU, memory and storage sums stay within its budget and every
//! service's required capability is declared by its host. The objective is
//! lexicographic: fewest moves away from `current_host`, then fewest hosts in
//! use. The search is the crate's one exact solver ([`super::solve`]) under the
//! `placement.node_budget` knob; nothing here is a second solver.
//!
//! Exact decomposition. Moving a service never lowers the move count, so in an
//! optimal plan every service whose current host is declared, capable and still
//! fits stays where it is whenever all such services fit together: the move
//! level is then at its lower bound (the services that cannot stay), and those
//! stayers already occupy their hosts, so the host level cannot fall by moving
//! them either. The program built is therefore over the services that cannot
//! stay only, one binary per (service, capable host) pair plus one "host used"
//! binary per host no stayer occupies. When the stayers do not fit together
//! every service enters the program. A program above the solver's variable
//! limit, or a search that exhausts its node budget, abstains `BudgetExhausted`
//! (MEDULLA-DECISION-ENGINE-R001.101); it never returns a guessed plan.
//!
//! Memory and storage are counted in whole KiB, demand rounded up and budget
//! rounded down, so the plan never over-commits a host.

use std::collections::BTreeMap;

use medulla_decision_types::decision::knob_families::placement::PlanKnobs;
use medulla_decision_types::decision::statistical::placement_plan::{
    HostBudget, PlacementPlan, ServiceMove, ServicePlacement, ServiceSpec,
};
use medulla_decision_types::decision::AbstainReason;
use medulla_types::contract::BoundedVec;

use super::certificate::{SolveStatus, SolverConfig, SolverConfigSpec, DEFAULT_CERTIFICATE_LEAVES};
use super::model::{
    Coefficient, ConstraintBody, ConstraintSpec, Model, ModelSpec, ObjectiveLevelSpec,
    ObjectiveTerm, Relation, Term, VarId, MAX_VARIABLES,
};
use super::scalar::{Scalar, Sha256Digest};

/// Digest domain of a plan in which every service stays (no program is built).
const STAY_PLAN_DOMAIN: &str = "medulla/placement-plan/stay/v1";
/// Bytes per counted memory or storage unit.
const KIB: u64 = 1024;

/// The answer of [`plan_services`]: a plan, or why there is none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanOutcome {
    Planned(PlacementPlan),
    Abstained(Vec<AbstainReason>),
}

/// The three budgeted dimensions of a host or a service, in counted units.
fn demand(service: &ServiceSpec) -> [u64; 3] {
    [
        service.cpu_millis,
        service.memory_bytes.div_ceil(KIB),
        service.storage_bytes.div_ceil(KIB),
    ]
}

fn budget(host: &HostBudget) -> [u64; 3] {
    [
        host.cpu_millis,
        host.memory_bytes / KIB,
        host.storage_bytes / KIB,
    ]
}

const DIMENSIONS: [&str; 3] = ["cpu", "memory", "storage"];

fn capable(host: &HostBudget, service: &ServiceSpec) -> bool {
    service
        .required_capability
        .is_none_or(|capability| host.capabilities.contains(&capability))
}

fn fits(used: &[u64; 3], add: &[u64; 3], cap: &[u64; 3]) -> bool {
    (0..3).all(|d| used[d].saturating_add(add[d]) <= cap[d])
}

/// Plan every service onto one host. See the module doc for the program and
/// its exact decomposition.
pub fn plan_services(
    hosts: &[HostBudget],
    services: &[ServiceSpec],
    knobs: &PlanKnobs,
) -> Result<PlanOutcome, String> {
    let index: BTreeMap<&str, usize> = hosts
        .iter()
        .enumerate()
        .map(|(i, host)| (host.host_iri.as_str(), i))
        .collect();
    if index.len() != hosts.len() {
        return Err("PARAMETER_INVALID: a host is named twice".to_string());
    }

    // A service no host can take abstains naming the capability gap.
    let gaps: Vec<String> = services
        .iter()
        .filter(|service| !hosts.iter().any(|host| capable(host, service)))
        .map(|service| {
            format!(
                "{}: requires capability {:?}",
                service.service_iri,
                service
                    .required_capability
                    .expect("only a required capability can be missing")
            )
        })
        .collect();
    if !gaps.is_empty() {
        return infeasible(gaps);
    }

    // Stayers: services whose current host is declared, capable and fits.
    let mut used = vec![[0u64; 3]; hosts.len()];
    let mut stays: Vec<Option<usize>> = vec![None; services.len()];
    for (s, service) in services.iter().enumerate() {
        let Some(h) = service
            .current_host
            .as_deref()
            .and_then(|iri| index.get(iri).copied())
        else {
            continue;
        };
        let need = demand(service);
        if capable(&hosts[h], service) && fits(&used[h], &need, &budget(&hosts[h])) {
            for d in 0..3 {
                used[h][d] += need[d];
            }
            stays[s] = Some(h);
        }
    }
    // When a stayer was refused only for room, every service enters the program.
    let crowded = services.iter().enumerate().any(|(s, service)| {
        stays[s].is_none()
            && service
                .current_host
                .as_deref()
                .and_then(|iri| index.get(iri).copied())
                .is_some_and(|h| capable(&hosts[h], service))
    });
    if crowded {
        stays.iter_mut().for_each(|stay| *stay = None);
        used.iter_mut().for_each(|row| *row = [0; 3]);
    }

    let residual: Vec<usize> = (0..services.len())
        .filter(|s| stays[*s].is_none())
        .collect();
    if residual.is_empty() {
        let placements = assemble(hosts, services, &stays);
        let digest = Sha256Digest::of_json(STAY_PLAN_DOMAIN, &placements);
        return Ok(PlanOutcome::Planned(PlacementPlan {
            placements,
            moves: Vec::new(),
            certificate_digest: String::from(digest),
        }));
    }
    solve_residual(hosts, services, &residual, (&mut stays, &used), knobs)
}

fn infeasible(constraints: Vec<String>) -> Result<PlanOutcome, String> {
    let mut constraints = constraints;
    constraints.truncate(64);
    Ok(PlanOutcome::Abstained(vec![AbstainReason::Infeasible {
        constraints: BoundedVec::new(constraints).map_err(|e| e.to_string())?,
    }]))
}

fn budget_exhausted(lower_bound: Option<Scalar>) -> Result<PlanOutcome, String> {
    Ok(PlanOutcome::Abstained(vec![
        AbstainReason::BudgetExhausted {
            incumbent: None,
            lower_bound: lower_bound.unwrap_or(Scalar::new(0)),
        },
    ]))
}

/// Build and solve the program over the services that cannot stay.
fn solve_residual(
    hosts: &[HostBudget],
    services: &[ServiceSpec],
    residual: &[usize],
    (stays, used): (&mut [Option<usize>], &[[u64; 3]]),
    knobs: &PlanKnobs,
) -> Result<PlanOutcome, String> {
    let mut occupied = vec![false; hosts.len()];
    for h in stays.iter().flatten() {
        occupied[*h] = true;
    }
    // Variables: one per (residual service, capable host), then one "used" per
    // unoccupied host that some residual service could take.
    let mut names = Vec::new();
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    for &s in residual {
        for (h, host) in hosts.iter().enumerate() {
            if capable(host, &services[s]) && fits(&used[h], &demand(&services[s]), &budget(host)) {
                pairs.push((s, h));
                names.push(format!("x:{}@{}", services[s].service_iri, host.host_iri));
            }
        }
    }
    let mut used_var: BTreeMap<usize, VarId> = BTreeMap::new();
    for &(_, h) in &pairs {
        if !occupied[h] && !used_var.contains_key(&h) {
            used_var.insert(h, VarId(names.len() as u32));
            names.push(format!("u:{}", hosts[h].host_iri));
        }
    }
    if names.len() > MAX_VARIABLES {
        return budget_exhausted(None);
    }
    let var = |i: usize| VarId(i as u32);

    let mut constraints = Vec::new();
    for &s in residual {
        let vars: Vec<VarId> = (0..pairs.len())
            .filter(|i| pairs[*i].0 == s)
            .map(var)
            .collect();
        if vars.is_empty() {
            return infeasible(vec![format!(
                "{}: no capable host has room",
                services[s].service_iri
            )]);
        }
        constraints.push(ConstraintSpec {
            label: format!("one-host:{}", services[s].service_iri),
            body: ConstraintBody::ExactlyOne { vars },
        });
    }
    for (h, host) in hosts.iter().enumerate() {
        let cap = budget(host);
        for (d, dimension) in DIMENSIONS.iter().enumerate() {
            let terms: Vec<Term> = (0..pairs.len())
                .filter(|i| pairs[*i].1 == h && demand(&services[pairs[*i].0])[d] > 0)
                .map(|i| Term {
                    var: var(i),
                    coefficient: i64::try_from(demand(&services[pairs[i].0])[d])
                        .unwrap_or(i64::MAX),
                })
                .collect();
            if terms.is_empty() {
                continue;
            }
            constraints.push(ConstraintSpec {
                label: format!("{dimension}:{}", host.host_iri),
                body: ConstraintBody::Linear {
                    terms,
                    relation: Relation::LessEqual,
                    rhs: i64::try_from(cap[d].saturating_sub(used[h][d])).unwrap_or(i64::MAX),
                },
            });
        }
        if let Some(&u) = used_var.get(&h) {
            for i in (0..pairs.len()).filter(|i| pairs[*i].1 == h) {
                constraints.push(ConstraintSpec {
                    label: format!("uses:{}", names[i]),
                    body: ConstraintBody::Implication {
                        antecedent: var(i),
                        consequent: u,
                    },
                });
            }
        }
    }

    let moves: Vec<ObjectiveTerm> = (0..pairs.len())
        .filter(|i| {
            let (s, h) = pairs[*i];
            services[s]
                .current_host
                .as_deref()
                .is_some_and(|current| current != hosts[h].host_iri)
        })
        .map(|i| ObjectiveTerm {
            var: var(i),
            coefficient: Coefficient::Known(1),
        })
        .collect();
    let host_terms: Vec<ObjectiveTerm> = used_var
        .values()
        .map(|&u| ObjectiveTerm {
            var: u,
            coefficient: Coefficient::Known(1),
        })
        .collect();
    let objective = [("moves", moves), ("hosts", host_terms)]
        .into_iter()
        .filter(|(_, terms)| !terms.is_empty())
        .map(|(label, terms)| ObjectiveLevelSpec {
            label: label.to_string(),
            terms,
        })
        .collect();

    let model = Model::try_from(ModelSpec {
        variables: names,
        constraints,
        objective,
    })
    .map_err(|error| format!("PARAMETER_INVALID: placement program: {error:?}"))?;
    let config = SolverConfig::try_from(SolverConfigSpec {
        node_budget: knobs.node_budget,
        max_certificate_leaves: DEFAULT_CERTIFICATE_LEAVES,
        bound_denominator: 1,
        accepted_gap: Scalar::new(0),
    })
    .map_err(|error| format!("PARAMETER_INVALID: placement.node_budget: {error:?}"))?;
    let certificate = super::solve(&model, &config);
    match &certificate.outcome {
        SolveStatus::Optimal | SolveStatus::OptimalByDeterministicSearch { .. } => {}
        SolveStatus::Infeasible { core } => {
            let labels = core
                .iter()
                .filter_map(|row| model.spec().constraints.get(row.index()))
                .map(|row| row.label.clone())
                .collect::<Vec<_>>();
            return infeasible(if labels.is_empty() {
                vec!["capacity".to_string()]
            } else {
                labels
            });
        }
        SolveStatus::InfeasibleByDeterministicSearch { .. } => {
            return infeasible(vec!["capacity".to_string()]);
        }
        SolveStatus::FeasibleWithGap { .. } | SolveStatus::BudgetExhausted => {
            return budget_exhausted(certificate.lower_bound);
        }
    }
    let selected = &certificate
        .incumbent
        .as_ref()
        .ok_or("an optimal certificate carries its incumbent")?
        .selected;
    for (i, &(s, h)) in pairs.iter().enumerate() {
        if selected[i] {
            stays[s] = Some(h);
        }
    }
    let placements = assemble(hosts, services, stays);
    let moves = services
        .iter()
        .zip(stays.iter())
        .filter_map(|(service, stay)| {
            let to = &hosts[(*stay)?].host_iri;
            let from = service.current_host.as_ref()?;
            (from != to).then(|| ServiceMove {
                service_iri: service.service_iri.clone(),
                from_host: from.clone(),
                to_host: to.clone(),
            })
        })
        .collect();
    Ok(PlanOutcome::Planned(PlacementPlan {
        placements,
        moves,
        certificate_digest: String::from(certificate.digest()),
    }))
}

fn assemble(
    hosts: &[HostBudget],
    services: &[ServiceSpec],
    stays: &[Option<usize>],
) -> Vec<ServicePlacement> {
    services
        .iter()
        .zip(stays)
        .filter_map(|(service, stay)| {
            stay.map(|h| ServicePlacement {
                service_iri: service.service_iri.clone(),
                host_iri: hosts[h].host_iri.clone(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_decision_types::decision::statistical::placement::HeavyCapability;

    const GPU_HOSTS: usize = 4;

    fn knobs() -> PlanKnobs {
        PlanKnobs {
            node_budget: 200_000,
        }
    }

    /// `hosts` hosts (the first [`GPU_HOSTS`] declare `Gpu`), `n` services
    /// spread over them as stayers, plus three services that cannot stay: two
    /// new `Gpu` services and one whose current host lacks `Gpu`.
    fn fixture(n: usize, hosts: usize) -> (Vec<HostBudget>, Vec<ServiceSpec>) {
        let per_host = n.div_ceil(hosts) as u64 + 4;
        let hosts_v: Vec<HostBudget> = (0..hosts)
            .map(|h| HostBudget {
                host_iri: format!("urn:host:{h}"),
                cpu_millis: 100 * per_host,
                memory_bytes: (1 << 20) * per_host,
                storage_bytes: (1 << 20) * per_host,
                capabilities: if h < GPU_HOSTS {
                    vec![HeavyCapability::Gpu]
                } else {
                    vec![]
                },
            })
            .collect();
        let mut services: Vec<ServiceSpec> = (0..n)
            .map(|s| ServiceSpec {
                service_iri: format!("urn:svc:{s}"),
                cpu_millis: 100,
                memory_bytes: 1 << 20,
                storage_bytes: 0,
                required_capability: None,
                current_host: Some(format!("urn:host:{}", s % hosts)),
            })
            .collect();
        for k in 0..2 {
            services.push(ServiceSpec {
                service_iri: format!("urn:svc:new-gpu-{k}"),
                cpu_millis: 100,
                memory_bytes: 1 << 20,
                storage_bytes: 1,
                required_capability: Some(HeavyCapability::Gpu),
                current_host: None,
            });
        }
        services.push(ServiceSpec {
            service_iri: "urn:svc:needs-gpu".into(),
            cpu_millis: 100,
            memory_bytes: 1 << 20,
            storage_bytes: 0,
            required_capability: Some(HeavyCapability::Gpu),
            current_host: Some(format!("urn:host:{}", hosts - 1)),
        });
        (hosts_v, services)
    }

    fn check(hosts: &[HostBudget], services: &[ServiceSpec], plan: &PlacementPlan) {
        assert_eq!(
            plan.placements.len(),
            services.len(),
            "every service is placed"
        );
        plan.check().unwrap();
        let mut load: BTreeMap<&str, [u64; 3]> = BTreeMap::new();
        for (service, placement) in services.iter().zip(&plan.placements) {
            assert_eq!(service.service_iri, placement.service_iri);
            let host = hosts
                .iter()
                .find(|h| h.host_iri == placement.host_iri)
                .unwrap();
            assert!(
                capable(host, service),
                "{} lacks the capability",
                host.host_iri
            );
            let row = load.entry(host.host_iri.as_str()).or_default();
            for (slot, add) in row.iter_mut().zip(demand(service)) {
                *slot += add;
            }
        }
        for host in hosts {
            let row = load
                .get(host.host_iri.as_str())
                .copied()
                .unwrap_or_default();
            assert!(
                fits(&[0; 3], &row, &budget(host)),
                "{} is over budget",
                host.host_iri
            );
        }
    }

    // spec: MEDULLA-PLACEMENT-R700.2
    #[test]
    fn plan_scales_on_fixtures() {
        // 100,000 is the nightly-size fixture; the decomposition keeps its
        // program to the three services that cannot stay.
        for (n, host_count) in [(10, 5), (1_000, 40), (100_000, 500)] {
            let (hosts, services) = fixture(n, host_count);
            let PlanOutcome::Planned(plan) = plan_services(&hosts, &services, &knobs()).unwrap()
            else {
                panic!("fixture {n} did not plan")
            };
            check(&hosts, &services, &plan);
            // Minimum moves: only the service whose host lacks `Gpu` moves.
            assert_eq!(plan.moves.len(), 1, "fixture {n}");
            assert_eq!(plan.moves[0].service_iri, "urn:svc:needs-gpu");
            // Minimum hosts: the new services join already-used GPU hosts.
            let used: std::collections::BTreeSet<&str> = plan
                .placements
                .iter()
                .map(|p| p.host_iri.as_str())
                .collect();
            assert_eq!(used.len(), host_count.min(n), "fixture {n}");
            // Deterministic: the same inputs give the same plan and digest.
            assert_eq!(
                plan_services(&hosts, &services, &knobs()).unwrap(),
                PlanOutcome::Planned(plan)
            );
        }

        // A capability no host declares abstains naming it.
        let (hosts, mut services) = fixture(10, 5);
        services[0].required_capability = Some(HeavyCapability::Backtest);
        let PlanOutcome::Abstained(reasons) = plan_services(&hosts, &services, &knobs()).unwrap()
        else {
            panic!("a capability gap must abstain")
        };
        let [AbstainReason::Infeasible { constraints }] = reasons.as_slice() else {
            panic!("{reasons:?}")
        };
        assert!(constraints.as_slice()[0].ends_with("requires capability Backtest"));

        // A program above the solver's variable limit abstains BudgetExhausted.
        let wide: Vec<HostBudget> = (0..MAX_VARIABLES + 1)
            .map(|h| HostBudget {
                host_iri: format!("urn:wide:{h}"),
                cpu_millis: 1_000,
                memory_bytes: 1 << 30,
                storage_bytes: 1 << 30,
                capabilities: vec![],
            })
            .collect();
        let one = vec![ServiceSpec {
            service_iri: "urn:svc:x".into(),
            cpu_millis: 1,
            memory_bytes: 1,
            storage_bytes: 0,
            required_capability: None,
            current_host: None,
        }];
        let PlanOutcome::Abstained(reasons) = plan_services(&wide, &one, &knobs()).unwrap() else {
            panic!("an oversized program must abstain")
        };
        assert!(matches!(
            reasons.as_slice(),
            [AbstainReason::BudgetExhausted { .. }]
        ));
    }
}
