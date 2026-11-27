//! Capability gap and desired enhancement nodes (MEDULLA-ORCH-VOCAB-R004.6.1).
//!
//! Both records name the capability, task class and domain they are about,
//! the digest of the evidence that raised them, the owning principal and a
//! closed disposition. The gap write of R004.6.2 decodes them before the
//! batch is built.

use serde::{Deserialize, Serialize};

/// The closed set of dispositions of a gap or enhancement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum GapDisposition {
    Open,
    Accepted,
    Resolved,
    Rejected,
}

macro_rules! gap_node {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        #[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
        pub struct $name {
            pub tenant_id: String,
            pub node_id: String,
            pub revision: u64,
            /// The capability the node is for (`:forCapability`).
            pub capability_ref: String,
            /// The task class the node is for (`:forTaskClass`).
            pub task_class_ref: String,
            /// The domain the node is in (`:inDomain`).
            pub domain_ref: String,
            pub source_evidence_digest: String,
            pub owner_principal: String,
            pub disposition: GapDisposition,
        }
    };
}

gap_node!(
    /// A capability the library lacks for a task class.
    CapabilityGap
);
gap_node!(
    /// An improvement wanted on a capability the library has.
    DesiredEnhancement
);

/// The refusal of a gap or enhancement write that does not decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityGapRefusal {
    /// Not the marked record, or a shape the record does not have.
    RecordMalformed,
}

/// Whether `props` is a capability-gap write (its engine-owned marker).
pub fn is_capability_gap_write(props: &serde_json::Map<String, serde_json::Value>) -> bool {
    props
        .get("_capability_gap")
        .and_then(|value| value.as_bool())
        == Some(true)
}

/// Whether `props` is a desired-enhancement write (its engine-owned marker).
pub fn is_desired_enhancement_write(props: &serde_json::Map<String, serde_json::Value>) -> bool {
    props
        .get("_desired_enhancement")
        .and_then(|value| value.as_bool())
        == Some(true)
}

/// Engine-owned keys stripped before the strict decode.
const ENGINE_OWNED_GAP_KEYS: &[&str] = &[
    "_capability_gap",
    "_desired_enhancement",
    "tx_from",
    "tx_to",
    "_owner_id",
];

fn decode_gap_record<T: for<'de> Deserialize<'de>>(
    props: &serde_json::Map<String, serde_json::Value>,
    marker: &str,
) -> Result<T, CapabilityGapRefusal> {
    if props.get(marker).and_then(|value| value.as_bool()) != Some(true) {
        return Err(CapabilityGapRefusal::RecordMalformed);
    }
    let mut record = props.clone();
    for key in ENGINE_OWNED_GAP_KEYS {
        record.remove(*key);
    }
    serde_json::from_value(serde_json::Value::Object(record))
        .map_err(|_| CapabilityGapRefusal::RecordMalformed)
}

/// Decode a capability-gap write from its property object.
pub fn decode_gap_write(
    props: &serde_json::Map<String, serde_json::Value>,
) -> Result<CapabilityGap, CapabilityGapRefusal> {
    decode_gap_record(props, "_capability_gap")
}

/// Decode a desired-enhancement write from its property object.
pub fn decode_enhancement_write(
    props: &serde_json::Map<String, serde_json::Value>,
) -> Result<DesiredEnhancement, CapabilityGapRefusal> {
    decode_gap_record(props, "_desired_enhancement")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gap() -> CapabilityGap {
        CapabilityGap {
            tenant_id: "tenant-a".into(),
            node_id: "gap:1".into(),
            revision: 1,
            capability_ref: "capability:translate".into(),
            task_class_ref: "task-class:docs".into(),
            domain_ref: "domain:legal".into(),
            source_evidence_digest: format!("sha256:{}", "ab".repeat(32)),
            owner_principal: "principal:sha256:x".into(),
            disposition: GapDisposition::Open,
        }
    }

    // spec: MEDULLA-ORCH-VOCAB-R004.6.1
    #[test]
    fn nodes_round_trip_and_disposition_is_closed() {
        let gap = gap();
        let json = serde_json::to_string(&gap).unwrap();
        assert_eq!(serde_json::from_str::<CapabilityGap>(&json).unwrap(), gap);

        let enhancement = DesiredEnhancement {
            tenant_id: gap.tenant_id.clone(),
            node_id: "enh:1".into(),
            revision: 2,
            capability_ref: gap.capability_ref.clone(),
            task_class_ref: gap.task_class_ref.clone(),
            domain_ref: gap.domain_ref.clone(),
            source_evidence_digest: gap.source_evidence_digest.clone(),
            owner_principal: gap.owner_principal.clone(),
            disposition: GapDisposition::Accepted,
        };
        let json = serde_json::to_string(&enhancement).unwrap();
        assert_eq!(
            serde_json::from_str::<DesiredEnhancement>(&json).unwrap(),
            enhancement
        );

        for (name, kind) in [
            ("open", GapDisposition::Open),
            ("accepted", GapDisposition::Accepted),
            ("resolved", GapDisposition::Resolved),
            ("rejected", GapDisposition::Rejected),
        ] {
            assert_eq!(
                serde_json::from_str::<GapDisposition>(&format!("\"{name}\"")).unwrap(),
                kind
            );
        }
        assert!(serde_json::from_str::<GapDisposition>("\"closed\"").is_err());

        let mut props = match serde_json::to_value(&gap).unwrap() {
            serde_json::Value::Object(map) => map,
            _ => unreachable!(),
        };
        assert_eq!(
            decode_gap_write(&props),
            Err(CapabilityGapRefusal::RecordMalformed)
        );
        props.insert("_capability_gap".into(), true.into());
        props.insert("tx_from".into(), 5u64.into());
        assert_eq!(decode_gap_write(&props).unwrap(), gap);
        props.insert("stray".into(), 1u64.into());
        assert_eq!(
            decode_gap_write(&props),
            Err(CapabilityGapRefusal::RecordMalformed)
        );
    }
}
