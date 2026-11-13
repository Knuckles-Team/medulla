//! Capability descriptors and request-scoped designation
//! (MEDULLA-DECISION-ENGINE-R650.1).
//!
//! Medulla holds one versioned descriptor per capability. A descriptor
//! carries typed inputs and outputs, side effects, data types, scopes, cost,
//! latency, reliability, locality and approval class. For one request
//! Medulla projects the classified closure; the harness receives the
//! designation and holds no descriptor index. Persisting the descriptor,
//! generating the power descriptor and projecting the OWL closure are later
//! children (R650.2); the served `CapabilityDesignate` method is R650.3.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use medulla_types::{contract::ResourceId, contract::ScopeKind, contract::TenantId};

/// The wire version of the descriptor family.
pub const CAPABILITY_DESCRIPTOR_VERSION: u16 = 1;

/// The largest reliability, in basis points of 1.0 (1 = 0.01%).
pub const MAX_CAPABILITY_RELIABILITY: u32 = 10_000;

/// One in Q32 fixed point: the scale of a learned reward.
const REWARD_ONE_Q32: i128 = 1 << 32;

/// The reliability the descriptor projection carries for a capability whose
/// learned reward is `reward_q32` (Q32 in `[0, 1]`), in basis points:
/// `(reward_q32 * 10000) >> 32`, clamped to [`MAX_CAPABILITY_RELIABILITY`].
/// A capability with no reward (`None`) is neutral (reward 0.5), so its
/// reliability is 5000. Reliability is derived at projection time and never
/// stored on the capability node (R650.2.1). The reward record itself lives in
/// the decision plane, so the function takes its Q32 value.
pub fn derived_reliability(reward_q32: Option<i64>) -> u32 {
    let reward = reward_q32.map_or(REWARD_ONE_Q32 / 2, i128::from);
    let basis_points = (reward * i128::from(MAX_CAPABILITY_RELIABILITY)) >> 32;
    u32::try_from(basis_points.clamp(0, i128::from(MAX_CAPABILITY_RELIABILITY)))
        .unwrap_or(MAX_CAPABILITY_RELIABILITY)
}

/// The side-effect class a capability declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum SideEffectClass {
    ReadOnly,
    GraphWrite,
    External,
    Irreversible,
}

/// The locality a capability runs from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum Locality {
    SameHost,
    SameRegion,
    Network,
}

/// The approval class a capability requires before execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ApprovalClass {
    None,
    Low,
    Medium,
    High,
}

/// One typed input or output port of a capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DataPort {
    pub name: ResourceId,
    /// The data type the port carries.
    pub data_type: ResourceId,
}

/// One versioned capability descriptor, tenant-scoped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CapabilityDescriptor {
    /// The descriptor version; [`CAPABILITY_DESCRIPTOR_VERSION`] for the
    /// current wire shape.
    pub version: u16,
    pub tenant: TenantId,
    pub capability_id: ResourceId,
    /// The typed inputs.
    pub inputs: Vec<DataPort>,
    /// The typed outputs.
    pub outputs: Vec<DataPort>,
    pub side_effect_class: SideEffectClass,
    /// The scopes the capability acts under.
    pub scopes: Vec<ScopeKind>,
    /// The unit cost of one call.
    pub cost: u64,
    /// The expected latency in milliseconds.
    pub latency_ms: u64,
    /// Reliability in basis points of 1.0. The value a projection carries;
    /// it is derived from the learned reward, not stored by the capability
    /// node (R650.2).
    pub reliability: u32,
    pub locality: Locality,
    pub approval_class: ApprovalClass,
}

impl CapabilityDescriptor {
    /// Validate before the descriptor is projected or designated.
    pub fn validate(&self) -> Result<(), String> {
        if self.version == 0 {
            return Err("capability descriptor version must be above zero".to_owned());
        }
        if self.reliability > MAX_CAPABILITY_RELIABILITY {
            return Err(format!(
                "capability descriptor reliability exceeds {MAX_CAPABILITY_RELIABILITY} basis points"
            ));
        }
        for (label, ports) in [("inputs", &self.inputs), ("outputs", &self.outputs)] {
            let mut seen = std::collections::BTreeSet::new();
            for port in ports {
                if !seen.insert(&port.name) {
                    return Err(format!(
                        "capability descriptor {label} repeats port name '{}'",
                        port.name.as_str()
                    ));
                }
            }
        }
        Ok(())
    }
}

/// The power descriptor generated from the registry and the capability
/// ledger (R650.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CapabilityPowerDescriptor {
    /// The descriptor version the power view was generated from.
    pub version: u16,
    pub tenant: TenantId,
    pub capability_id: ResourceId,
    /// The registry digest the power view derives from.
    pub registry_digest: String,
    /// The capability-ledger digest the power view derives from.
    pub ledger_digest: String,
}

impl CapabilityPowerDescriptor {
    /// Validate before the power descriptor is served.
    pub fn validate(&self) -> Result<(), String> {
        if self.version == 0 {
            return Err("capability power descriptor version must be above zero".to_owned());
        }
        super::validate_digest("registry_digest", &self.registry_digest)?;
        super::validate_digest("ledger_digest", &self.ledger_digest)
    }
}

/// A request whose classified closure Medulla projects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DesignationRequest {
    pub tenant: TenantId,
    pub request_id: ResourceId,
    /// The capabilities the request requires.
    pub required_capabilities: Vec<ResourceId>,
    /// Milliseconds since the Unix epoch: the instant the projection reads at.
    pub as_of_ms: i64,
}

impl DesignationRequest {
    /// Validate before the closure projection starts.
    pub fn validate(&self) -> Result<(), String> {
        if self.required_capabilities.is_empty() {
            return Err("a designation request needs at least one required capability".to_owned());
        }
        if self.as_of_ms <= 0 {
            return Err(format!(
                "designation request as-of instant {} must be a positive unix-millisecond instant",
                self.as_of_ms
            ));
        }
        Ok(())
    }
}

/// One capability designated for a request, bound to its descriptor version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DesignationEntry {
    pub capability_id: ResourceId,
    pub descriptor_version: u16,
}

/// The designation the harness receives; it holds no descriptor index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct Designation {
    pub tenant: TenantId,
    pub request_id: ResourceId,
    /// The designated capabilities, in closure order.
    pub entries: Vec<DesignationEntry>,
    /// The digest of the projected closure the designation covers.
    pub closure_digest: String,
}

impl Designation {
    /// Validate before the designation leaves Medulla.
    pub fn validate(&self) -> Result<(), String> {
        super::validate_digest("closure_digest", &self.closure_digest)
    }
}

/// The request-scoped classified closure Medulla projects; R650.2 computes
/// it from OWL classification in O(closure size).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ClosureProjection {
    pub tenant: TenantId,
    pub request_id: ResourceId,
    /// The capability whose closure this is.
    pub capability_id: ResourceId,
    /// The closure members, in deterministic order.
    pub members: Vec<ResourceId>,
    /// The ontology digest the classification ran under.
    pub ontology_digest: String,
}

impl ClosureProjection {
    /// Validate before the projection is bound into a designation.
    pub fn validate(&self) -> Result<(), String> {
        super::validate_digest("ontology_digest", &self.ontology_digest)
    }
}

/// One Agent Library component head, as the registry digest sees it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RegistryHead<'a> {
    pub component_id: &'a str,
    pub version: &'a str,
    pub content_digest: &'a str,
}

/// One `capability_reward` row, as the ledger digest sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedgerRow<'a> {
    pub entity_id: &'a str,
    pub reward_q32: i64,
    pub outcome_count: u64,
    pub generation: u32,
    pub updated_revision: u64,
}

/// `sha256:` and 64 lowercase hex characters over length-prefixed fields, so
/// no two field lists share a byte stream.
fn digest_of(domain: &str, rows: &[Vec<String>]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update([0u8]);
    for row in rows {
        for field in row {
            hasher.update((field.len() as u64).to_be_bytes());
            hasher.update(field.as_bytes());
        }
        hasher.update([0xffu8]);
    }
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

/// The registry digest: SHA-256 of the tenant's Agent Library component
/// heads, sorted by component id (R650.2.2).
pub fn registry_digest(heads: &[RegistryHead<'_>]) -> String {
    let mut sorted: Vec<&RegistryHead<'_>> = heads.iter().collect();
    sorted.sort();
    let rows: Vec<Vec<String>> = sorted
        .iter()
        .map(|head| {
            vec![
                head.component_id.to_owned(),
                head.version.to_owned(),
                head.content_digest.to_owned(),
            ]
        })
        .collect();
    digest_of("eg/capability-registry/v1", &rows)
}

/// The ledger digest: SHA-256 of the tenant's `capability_reward` rows in
/// entity (key) order (R650.2.2).
pub fn ledger_digest(rows: &[LedgerRow<'_>]) -> String {
    let mut sorted: Vec<&LedgerRow<'_>> = rows.iter().collect();
    sorted.sort_by(|a, b| a.entity_id.cmp(b.entity_id));
    let rows: Vec<Vec<String>> = sorted
        .iter()
        .map(|row| {
            vec![
                row.entity_id.to_owned(),
                row.reward_q32.to_string(),
                row.outcome_count.to_string(),
                row.generation.to_string(),
                row.updated_revision.to_string(),
            ]
        })
        .collect();
    digest_of("eg/capability-ledger/v1", &rows)
}

/// The power descriptor of `descriptor`, generated from the registry and the
/// capability ledger (R650.2.2). Pure and byte-stable for equal inputs.
pub fn power_descriptor(
    descriptor: &CapabilityDescriptor,
    registry_digest: &str,
    ledger_digest: &str,
) -> CapabilityPowerDescriptor {
    CapabilityPowerDescriptor {
        version: descriptor.version,
        tenant: descriptor.tenant.clone(),
        capability_id: descriptor.capability_id.clone(),
        registry_digest: registry_digest.to_owned(),
        ledger_digest: ledger_digest.to_owned(),
    }
}

/// What one request-scoped closure projection reads (R650.2.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosureRequest {
    pub tenant: TenantId,
    pub request_id: ResourceId,
    /// The capability whose closure is projected.
    pub capability_id: ResourceId,
    /// The largest closure served (`designation.closure_max_members`).
    pub max_members: usize,
}

/// The `rdfs:subClassOf` edges of the question's schema, indexed downward.
/// The engine's `OwlReason` derives them at the schema digest; nothing here
/// reads a schema.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SchemaClosure {
    /// The ontology digest the classification ran under.
    pub ontology_digest: String,
    children: BTreeMap<ResourceId, BTreeSet<ResourceId>>,
}

impl SchemaClosure {
    /// A closure from `(subclass, superclass)` edges.
    pub fn new<'a>(
        ontology_digest: &str,
        edges: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Result<Self, String> {
        let mut children: BTreeMap<ResourceId, BTreeSet<ResourceId>> = BTreeMap::new();
        for (child, parent) in edges {
            children
                .entry(ResourceId::new(parent).map_err(|e| format!("{parent}: {e}"))?)
                .or_default()
                .insert(ResourceId::new(child).map_err(|e| format!("{child}: {e}"))?);
        }
        Ok(Self {
            ontology_digest: ontology_digest.to_owned(),
            children,
        })
    }
}

/// A closure larger than the request allows (`DESIGNATION_CLOSURE_TOO_LARGE`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosureTooLarge {
    pub capability_id: ResourceId,
    pub limit: usize,
}

/// The classified closure of one capability: the capability and every class
/// below it, found by following `rdfs:subClassOf` edges downward, in sorted
/// order. The cost is O(closure size); a closure past `max_members` refuses
/// before it is complete (R650.2.3).
pub fn project_closure(
    request: &ClosureRequest,
    schema: &SchemaClosure,
) -> Result<ClosureProjection, ClosureTooLarge> {
    let mut found: BTreeSet<&ResourceId> = BTreeSet::from([&request.capability_id]);
    let mut queue: VecDeque<&ResourceId> = VecDeque::from([&request.capability_id]);
    while let Some(at) = queue.pop_front() {
        for child in schema.children.get(at).into_iter().flatten() {
            if found.insert(child) {
                if found.len() > request.max_members {
                    return Err(ClosureTooLarge {
                        capability_id: request.capability_id.clone(),
                        limit: request.max_members,
                    });
                }
                queue.push_back(child);
            }
        }
    }
    Ok(ClosureProjection {
        tenant: request.tenant.clone(),
        request_id: request.request_id.clone(),
        capability_id: request.capability_id.clone(),
        members: found.into_iter().cloned().collect(),
        ontology_digest: schema.ontology_digest.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A valid `sha256:<64 lowercase hex>` digest.
    const DIGEST: &str = "sha256:abababababababababababababababababababababababababababababababab";

    fn descriptor() -> CapabilityDescriptor {
        CapabilityDescriptor {
            version: CAPABILITY_DESCRIPTOR_VERSION,
            tenant: TenantId::new("tenant-a").unwrap(),
            capability_id: ResourceId::new("eg:capability/translate").unwrap(),
            inputs: vec![DataPort {
                name: ResourceId::new("text").unwrap(),
                data_type: ResourceId::new("eg:type/text").unwrap(),
            }],
            outputs: vec![DataPort {
                name: ResourceId::new("translation").unwrap(),
                data_type: ResourceId::new("eg:type/text").unwrap(),
            }],
            side_effect_class: SideEffectClass::External,
            scopes: vec![ScopeKind::new("tenant").unwrap()],
            cost: 12,
            latency_ms: 300,
            reliability: 9_800,
            locality: Locality::Network,
            approval_class: ApprovalClass::Low,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R650.1
    #[test]
    fn capability_descriptor_types_round_trip() {
        let descriptor = descriptor();
        let power = CapabilityPowerDescriptor {
            version: 1,
            tenant: TenantId::new("tenant-a").unwrap(),
            capability_id: ResourceId::new("eg:capability/translate").unwrap(),
            registry_digest: DIGEST.into(),
            ledger_digest: DIGEST.into(),
        };
        let request = DesignationRequest {
            tenant: TenantId::new("tenant-a").unwrap(),
            request_id: ResourceId::new("designate:1").unwrap(),
            required_capabilities: vec![ResourceId::new("eg:capability/translate").unwrap()],
            as_of_ms: 1_700_000_000_000,
        };
        let designation = Designation {
            tenant: TenantId::new("tenant-a").unwrap(),
            request_id: ResourceId::new("designate:1").unwrap(),
            entries: vec![DesignationEntry {
                capability_id: ResourceId::new("eg:capability/translate").unwrap(),
                descriptor_version: 1,
            }],
            closure_digest: DIGEST.into(),
        };
        let projection = ClosureProjection {
            tenant: TenantId::new("tenant-a").unwrap(),
            request_id: ResourceId::new("designate:1").unwrap(),
            capability_id: ResourceId::new("eg:capability/translate").unwrap(),
            members: vec![
                ResourceId::new("eg:capability/translate").unwrap(),
                ResourceId::new("eg:capability/gloss").unwrap(),
            ],
            ontology_digest: DIGEST.into(),
        };
        macro_rules! round_trip {
            ($value:expr, $ty:ty) => {{
                let json = serde_json::to_string(&$value).unwrap();
                assert_eq!(serde_json::from_str::<$ty>(&json).unwrap(), $value);
            }};
        }
        round_trip!(descriptor, CapabilityDescriptor);
        round_trip!(power, CapabilityPowerDescriptor);
        round_trip!(request, DesignationRequest);
        round_trip!(designation, Designation);
        round_trip!(projection, ClosureProjection);
        assert_eq!(
            serde_json::to_value(descriptor.side_effect_class).unwrap(),
            "external"
        );
        assert_eq!(
            serde_json::to_value(descriptor.approval_class).unwrap(),
            "low"
        );
        assert!(descriptor.validate().is_ok());
        assert!(power.validate().is_ok());
        assert!(request.validate().is_ok());
        assert!(designation.validate().is_ok());
        assert!(projection.validate().is_ok());
    }

    // spec: MEDULLA-DECISION-ENGINE-R650.2.1
    #[test]
    fn reliability_is_derived_from_reward_and_never_stored() {
        // Neutral (no reward), zero and full reward.
        assert_eq!(derived_reliability(None), 5_000);
        assert_eq!(derived_reliability(Some(1 << 31)), 5_000);
        assert_eq!(derived_reliability(Some(0)), 0);
        assert_eq!(
            derived_reliability(Some(1 << 32)),
            MAX_CAPABILITY_RELIABILITY
        );
        // Out-of-range rewards clamp rather than wrap.
        assert_eq!(derived_reliability(Some(-5)), 0);
        assert_eq!(
            derived_reliability(Some(i64::MAX)),
            MAX_CAPABILITY_RELIABILITY
        );
        // A quarter reward is 2500 basis points.
        assert_eq!(derived_reliability(Some(1 << 30)), 2_500);
        // The derived value is always a valid descriptor reliability.
        let mut descriptor = descriptor();
        descriptor.reliability = derived_reliability(Some(1 << 32));
        assert!(descriptor.validate().is_ok());
    }

    // spec: MEDULLA-DECISION-ENGINE-R650.1
    #[test]
    fn capability_descriptor_types_reject_invalid_inputs() {
        assert!(serde_json::from_str::<CapabilityDescriptor>(
            r#"{"version":1,"tenant":"tenant a","capability_id":"eg:capability/x","inputs":[],"outputs":[],"side_effect_class":"read_only","scopes":[],"cost":0,"latency_ms":1,"reliability":0,"locality":"same_host","approval_class":"none"}"#
        )
        .is_err());
        let mut descriptor = descriptor();
        descriptor.reliability = MAX_CAPABILITY_RELIABILITY + 1;
        assert!(descriptor.validate().is_err());
        descriptor.reliability = 100;
        descriptor.version = 0;
        assert!(descriptor.validate().is_err());
        descriptor.version = 1;
        descriptor.inputs.push(descriptor.inputs[0].clone());
        assert!(descriptor.validate().is_err());

        let power = CapabilityPowerDescriptor {
            registry_digest: "not-a-digest".into(),
            ..descriptor_power()
        };
        assert!(power.validate().is_err());

        let mut request = DesignationRequest {
            tenant: TenantId::new("tenant-a").unwrap(),
            request_id: ResourceId::new("designate:1").unwrap(),
            required_capabilities: vec![],
            as_of_ms: 1,
        };
        assert!(request.validate().is_err());
        request.required_capabilities = vec![ResourceId::new("eg:capability/translate").unwrap()];
        request.as_of_ms = 0;
        assert!(request.validate().is_err());

        let mut designation = Designation {
            tenant: TenantId::new("tenant-a").unwrap(),
            request_id: ResourceId::new("designate:1").unwrap(),
            entries: vec![],
            closure_digest: "ab".repeat(31),
        };
        assert!(designation.validate().is_err());
        designation.closure_digest = DIGEST.into();
        assert!(designation.validate().is_ok());

        assert!(serde_json::from_str::<DesignationRequest>(
            r#"{"tenant":"tenant-a","request_id":"designate:1","required_capabilities":[],"as_of_ms":1,"extra":2}"#
        )
        .is_err());
    }

    fn lines(range: std::ops::Range<usize>) -> Vec<(String, String)> {
        // A diamond-rich tree: class n has parents n/2 and n/3 (when below n).
        range
            .flat_map(|n| {
                [n / 2, n / 3]
                    .into_iter()
                    .filter(move |p| *p >= 1 && *p < n)
                    .map(move |p| (format!("eg:c/{n}"), format!("eg:c/{p}")))
            })
            .collect()
    }

    fn closure_of(edges: &[(String, String)]) -> SchemaClosure {
        SchemaClosure::new(DIGEST, edges.iter().map(|(c, p)| (c.as_str(), p.as_str()))).unwrap()
    }

    fn closure_request(capability: &str, max: usize) -> ClosureRequest {
        ClosureRequest {
            tenant: TenantId::new("tenant-a").unwrap(),
            request_id: ResourceId::new("designate:1").unwrap(),
            capability_id: ResourceId::new(capability).unwrap(),
            max_members: max,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R650.2.2
    #[test]
    fn power_descriptor_is_stable_for_equal_inputs() {
        let descriptor = descriptor();
        let heads = [
            RegistryHead {
                component_id: "tool:b",
                version: "1",
                content_digest: DIGEST,
            },
            RegistryHead {
                component_id: "tool:a",
                version: "2",
                content_digest: DIGEST,
            },
        ];
        let reversed = [heads[1].clone(), heads[0].clone()];
        let row = |reward_q32| LedgerRow {
            entity_id: "tool:a",
            reward_q32,
            outcome_count: 3,
            generation: 0,
            updated_revision: 7,
        };
        let other = LedgerRow {
            entity_id: "tool:b",
            ..row(1 << 31)
        };
        let ledger = [row(1 << 30), other];
        let registry = registry_digest(&heads);
        let ledger_one = ledger_digest(&ledger);
        // Equal inputs give byte-equal output, whatever order they arrive in.
        assert_eq!(registry, registry_digest(&reversed));
        assert_eq!(ledger_one, ledger_digest(&[ledger[1], ledger[0]]));
        let power = power_descriptor(&descriptor, &registry, &ledger_one);
        assert_eq!(power, power_descriptor(&descriptor, &registry, &ledger_one));
        assert_eq!(
            serde_json::to_string(&power).unwrap(),
            serde_json::to_string(&power_descriptor(&descriptor, &registry, &ledger_one)).unwrap()
        );
        assert!(power.validate().is_ok());
        for digest in [&power.registry_digest, &power.ledger_digest] {
            let hex = digest.strip_prefix("sha256:").unwrap();
            assert_eq!(hex.len(), 64);
            assert!(hex
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)));
        }
        // One changed reward row changes the ledger digest and only it.
        let changed = ledger_digest(&[row((1 << 30) + 1), other]);
        assert_ne!(changed, ledger_one);
        let moved = power_descriptor(&descriptor, &registry, &changed);
        assert_ne!(moved.ledger_digest, power.ledger_digest);
        assert_eq!(moved.registry_digest, power.registry_digest);
        // One changed head changes the registry digest.
        let mut newer = heads.clone();
        newer[0].version = "9";
        assert_ne!(registry_digest(&newer), registry);
    }

    // spec: MEDULLA-DECISION-ENGINE-R650.2.3
    #[test]
    fn closure_is_deterministic_across_runs_and_threads() {
        let edges = lines(2..400);
        let schema = closure_of(&edges);
        let request = closure_request("eg:c/1", 4096);
        let reference = project_closure(&request, &schema).unwrap();
        assert!(reference.members.len() > 100);
        assert!(reference.members.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(reference.ontology_digest, DIGEST);
        assert!(reference.validate().is_ok());
        // A shuffled edge order builds the same closure.
        let mut shuffled = edges.clone();
        shuffled.reverse();
        assert_eq!(
            project_closure(&request, &closure_of(&shuffled)).unwrap(),
            reference
        );
        // Repeated runs on one thread, then eight threads at once.
        for _ in 0..8 {
            assert_eq!(project_closure(&request, &schema).unwrap(), reference);
        }
        let results: Vec<ClosureProjection> = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..8)
                .map(|_| scope.spawn(|| project_closure(&request, &schema).unwrap()))
                .collect();
            workers.into_iter().map(|w| w.join().unwrap()).collect()
        });
        assert!(results.iter().all(|r| *r == reference));
        // The closure of a leaf is the leaf; the closure of a middle class
        // holds only classes below it.
        let leaf = project_closure(&closure_request("eg:c/399", 64), &schema).unwrap();
        assert_eq!(leaf.members, vec![ResourceId::new("eg:c/399").unwrap()]);
        let middle = project_closure(&closure_request("eg:c/100", 4096), &schema).unwrap();
        assert!(middle.members.len() < reference.members.len());
        assert!(middle.members.iter().all(|m| reference.members.contains(m)));
    }

    // spec: MEDULLA-DECISION-ENGINE-R650.2.3
    #[test]
    fn a_closure_past_the_limit_refuses_without_a_partial_result() {
        let schema = closure_of(&lines(2..400));
        let whole = project_closure(&closure_request("eg:c/1", 4096), &schema).unwrap();
        let exact = project_closure(&closure_request("eg:c/1", whole.members.len()), &schema);
        assert_eq!(exact.unwrap(), whole);
        let refused = project_closure(&closure_request("eg:c/1", whole.members.len() - 1), &schema)
            .unwrap_err();
        assert_eq!(refused.limit, whole.members.len() - 1);
        assert_eq!(refused.capability_id.as_str(), "eg:c/1");
    }

    fn descriptor_power() -> CapabilityPowerDescriptor {
        CapabilityPowerDescriptor {
            version: 1,
            tenant: TenantId::new("tenant-a").unwrap(),
            capability_id: ResourceId::new("eg:capability/translate").unwrap(),
            registry_digest: DIGEST.into(),
            ledger_digest: DIGEST.into(),
        }
    }
}
