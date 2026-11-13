//! Placement-plan model: host budgets, service specs and the assembled plan
//! (MEDULLA-PLACEMENT-R700.1). `placement_plan_outcome` decodes the request
//! body into these types.

use serde::{Deserialize, Serialize};

use super::placement::HeavyCapability;

/// Most hosts one request may carry.
pub const MAX_PLAN_HOSTS: usize = 512;
/// Most services one request may carry.
pub const MAX_PLAN_SERVICES: usize = 4_096;

/// One host's resource budget. Named `HostBudget` so it does not clash with
/// `HostCapacity` in `resource_status.rs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct HostBudget {
    pub host_iri: String,
    pub cpu_millis: u64,
    pub memory_bytes: u64,
    pub storage_bytes: u64,
    pub capabilities: Vec<HeavyCapability>,
}

/// One service to place and its resource demand.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ServiceSpec {
    pub service_iri: String,
    pub cpu_millis: u64,
    pub memory_bytes: u64,
    pub storage_bytes: u64,
    pub required_capability: Option<HeavyCapability>,
    pub current_host: Option<String>,
}

/// One service assigned to one host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ServicePlacement {
    pub service_iri: String,
    pub host_iri: String,
}

/// A service whose host changes under the plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ServiceMove {
    pub service_iri: String,
    pub from_host: String,
    pub to_host: String,
}

/// The solved plan: every placement, the moves it implies and the digest of
/// the solver certificate (64 lowercase hex characters).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PlacementPlan {
    pub placements: Vec<ServicePlacement>,
    pub moves: Vec<ServiceMove>,
    pub certificate_digest: String,
}

/// Check the request bounds on the host and service lists.
pub fn check_plan_request(hosts: &[HostBudget], services: &[ServiceSpec]) -> Result<(), String> {
    if hosts.len() > MAX_PLAN_HOSTS {
        return Err(format!(
            "at most {MAX_PLAN_HOSTS} hosts per request, got {}",
            hosts.len()
        ));
    }
    if services.len() > MAX_PLAN_SERVICES {
        return Err(format!(
            "at most {MAX_PLAN_SERVICES} services per request, got {}",
            services.len()
        ));
    }
    Ok(())
}

impl PlacementPlan {
    /// The certificate digest must be 64 lowercase hex characters.
    pub fn check(&self) -> Result<(), String> {
        let d = &self.certificate_digest;
        if d.len() == 64 && d.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            Ok(())
        } else {
            Err("certificate_digest must be 64 lowercase hex characters".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(i: usize) -> HostBudget {
        HostBudget {
            host_iri: format!("urn:host:{i}"),
            cpu_millis: 4000,
            memory_bytes: 1 << 33,
            storage_bytes: 1 << 40,
            capabilities: vec![HeavyCapability::Gpu],
        }
    }

    fn service(i: usize) -> ServiceSpec {
        ServiceSpec {
            service_iri: format!("urn:svc:{i}"),
            cpu_millis: 250,
            memory_bytes: 1 << 29,
            storage_bytes: 0,
            required_capability: Some(HeavyCapability::Backtest),
            current_host: Some("urn:host:0".into()),
        }
    }

    // spec: MEDULLA-PLACEMENT-R700.1
    #[test]
    fn plan_types_round_trip() {
        let h = host(0);
        let s = service(0);
        let plan = PlacementPlan {
            placements: vec![ServicePlacement {
                service_iri: s.service_iri.clone(),
                host_iri: h.host_iri.clone(),
            }],
            moves: vec![ServiceMove {
                service_iri: s.service_iri.clone(),
                from_host: "urn:host:1".into(),
                to_host: h.host_iri.clone(),
            }],
            certificate_digest: "a".repeat(64),
        };
        let h2: HostBudget = serde_json::from_str(&serde_json::to_string(&h).unwrap()).unwrap();
        let s2: ServiceSpec = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        let p2: PlacementPlan =
            serde_json::from_str(&serde_json::to_string(&plan).unwrap()).unwrap();
        assert_eq!((h2, s2, p2.clone()), (h, s, plan));
        p2.check().unwrap();

        // Unknown fields are refused.
        let bad = r#"{"host_iri":"x","cpu_millis":1,"memory_bytes":1,"storage_bytes":1,"capabilities":[],"extra":1}"#;
        assert!(serde_json::from_str::<HostBudget>(bad).is_err());

        // A short or upper-case digest is refused.
        let mut bad_plan = p2;
        bad_plan.certificate_digest = "A".repeat(64);
        assert!(bad_plan.check().is_err());

        // 512 hosts pass, 513 are rejected; 4,096 services pass, 4,097 are rejected.
        let hosts: Vec<_> = (0..MAX_PLAN_HOSTS).map(host).collect();
        let services: Vec<_> = (0..MAX_PLAN_SERVICES).map(service).collect();
        check_plan_request(&hosts, &services).unwrap();
        let mut too_many_hosts = hosts.clone();
        too_many_hosts.push(host(MAX_PLAN_HOSTS));
        assert!(check_plan_request(&too_many_hosts, &services).is_err());
        let mut too_many_services = services;
        too_many_services.push(service(MAX_PLAN_SERVICES));
        assert!(check_plan_request(&hosts, &too_many_services).is_err());
    }
}
