//! Machine identity, lifecycle and hosts-file functions of `medulla.engine`
//! (MEDULLA-MACHINE-R007).
//!
//! One thin wrapper per pure function of `medulla_source_types::machine`. Every function takes
//! and returns text; a structured value is canonical JSON (sorted keys, no white space). A
//! refusal is the `Err` text, which the Python module raises as `ValueError`. The wrappers are
//! plain Rust so they are unit-tested without an interpreter; `lib.rs` registers them in the
//! `engine` module under the `python` feature.

use std::collections::BTreeMap;

use medulla_source_types::machine::hosts::{self, HostsFileRefused};
use medulla_source_types::machine::identity::{self, MachineKey, MachineKeyKind};
use medulla_source_types::machine::lifecycle::{
    derive_event, MachineLifecycleEvent, MachineLifecycleFacts, MachineLifecycleState,
};
use medulla_source_types::machine::records::{
    self, canonical_text, ComponentKind, InventoryRecordSet, PropertyValue,
};
use medulla_types::transition_refused::{
    MachineEvent, MachineState, TransitionRefusalReason, TransitionRefused,
};
use serde::Deserialize;
use serde_json::json;

/// The refusal of an unknown key kind. The reasons of a key itself are `malformed`,
/// `placeholder` and `empty`.
const UNKNOWN_KIND: &str = "unknown_kind";

fn kind_of(tag: &str) -> Result<MachineKeyKind, String> {
    MachineKeyKind::from_tag(tag).ok_or_else(|| format!("{tag}:{UNKNOWN_KIND}"))
}

fn normalized(kind: &str, raw: &str) -> Result<MachineKey, String> {
    identity::normalize_key(kind_of(kind)?, raw).map_err(|refused| refused.to_string())
}

fn key_text(key: &MachineKey) -> String {
    format!("{}:{}", key.kind.tag(), key.value)
}

/// `machine_key_ref(kind, raw)`: normalize one key and return `{iri, key}`, the anchor id of
/// the key and its `<kind>:<value>` text.
pub fn machine_key_ref(kind: &str, raw: &str) -> Result<String, String> {
    let key = normalized(kind, raw)?;
    Ok(canonical_text(&json!({
        "iri": identity::machine_key_iri(&key),
        "key": key_text(&key),
    })))
}

/// `machine_normalize_key(kind, raw)`: `{kind, value}` of the normalized key, or the refusal
/// text `<kind>:<reason>`.
pub fn machine_normalize_key(kind: &str, raw: &str) -> Result<String, String> {
    let key = normalized(kind, raw)?;
    Ok(canonical_text(
        &json!({"kind": key.kind.tag(), "value": key.value}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyInput {
    kind: String,
    value: String,
}

/// `machine_iri(tenant, keys_json)`: select the best valid key of `[{kind, value}]` and return
/// `{iri, tier, key}`. `no_valid_key` when none is valid.
pub fn machine_iri(tenant: &str, keys_json: &str) -> Result<String, String> {
    let inputs: Vec<KeyInput> =
        serde_json::from_str(keys_json).map_err(|_| "keys_json:malformed".to_string())?;
    let mut candidates = Vec::with_capacity(inputs.len());
    for input in inputs {
        candidates.push(MachineKey {
            kind: kind_of(&input.kind)?,
            value: input.value,
        });
    }
    let key = identity::select_key(&candidates).ok_or_else(|| "no_valid_key".to_string())?;
    Ok(canonical_text(&json!({
        "iri": identity::machine_iri(tenant, &key),
        "tier": key.kind.tag(),
        "key": key_text(&key),
    })))
}

fn state_of(state: &str) -> Result<MachineLifecycleState, String> {
    MachineLifecycleState::from_stored(state).ok_or_else(|| "unknown_state".to_string())
}

fn facts_of(facts_json: &str) -> Result<MachineLifecycleFacts, String> {
    serde_json::from_str(facts_json).map_err(|_| "facts_json:malformed".to_string())
}

/// `machine_lifecycle_derive(state, facts_json)`: `{event}` with the one event the facts call
/// for in the stored `state`, or `null`.
pub fn machine_lifecycle_derive(state: &str, facts_json: &str) -> Result<String, String> {
    let event = derive_event(state_of(state)?, &facts_of(facts_json)?);
    Ok(canonical_text(
        &json!({"event": event.map(MachineEvent::name)}),
    ))
}

/// `machine_lifecycle_apply(state, event, facts_json)`: `{state, edge_id}` after one
/// transition, or the `TRANSITION_REFUSED` detail text.
pub fn machine_lifecycle_apply(
    state: &str,
    event: &str,
    facts_json: &str,
) -> Result<String, String> {
    let from = state_of(state)?;
    let facts = facts_of(facts_json)?;
    let Some(event_value) = MachineLifecycleEvent::from_name(event) else {
        return Err(TransitionRefused {
            machine: "MachineLifecycleMachine".to_string(),
            from: from.stored().to_string(),
            event: event.to_string(),
            reason: TransitionRefusalReason::UnknownEvent,
        }
        .to_string());
    };
    let (next, edge_id) = from
        .apply_with_edge(event_value, &facts)
        .map_err(|refused| refused.to_string())?;
    Ok(canonical_text(
        &json!({"state": next.stored(), "edge_id": edge_id}),
    ))
}

fn refusal_text(refused: &HostsFileRefused) -> String {
    canonical_text(refused)
}

/// `normalize_hosts_file(yaml)`: the canonical JSON of the normalized hosts file, or the
/// `HOSTS_FILE_INVALID` record as JSON.
pub fn normalize_hosts_file(yaml: &str) -> Result<String, String> {
    hosts::normalize_hosts_file(yaml)
        .map(|normalized| hosts::normalized_hosts_json(&normalized))
        .map_err(|refused| refusal_text(&refused))
}

/// `machine_hosts_digest(yaml)`: the digest of the normalized hosts file.
pub fn machine_hosts_digest(yaml: &str) -> Result<String, String> {
    hosts::normalize_hosts_file(yaml)
        .map(|normalized| hosts::hosts_digest(&normalized))
        .map_err(|refused| refusal_text(&refused))
}

/// `canonical_digest(text)`: `sha256:` and the lower-case hex digest of the text.
pub fn canonical_digest(text: &str) -> String {
    records::canonical_digest(text.as_bytes())
}

/// `machine_canonical_json(records_json)`: the canonical JSON of an `inventory-records/v1`
/// record set.
pub fn machine_canonical_json(records_json: &str) -> Result<String, String> {
    let set: InventoryRecordSet =
        serde_json::from_str(records_json).map_err(|_| "records_json:malformed".to_string())?;
    if set.schema != records::INVENTORY_RECORDS_SCHEMA {
        return Err("records_json:schema".to_string());
    }
    Ok(records::canonical_json(&set))
}

/// `machine_check_shapes(records_json)`: the canonical JSON of the record set when every machine
/// and part passes the machine shapes, or `<entity>: <constraint>` for the first one that does
/// not.
pub fn machine_check_shapes(records_json: &str) -> Result<String, String> {
    let set: InventoryRecordSet =
        serde_json::from_str(records_json).map_err(|_| "records_json:malformed".to_string())?;
    if set.schema != records::INVENTORY_RECORDS_SCHEMA {
        return Err("records_json:schema".to_string());
    }
    set.check_shapes().map_err(|refused| refused.to_string())?;
    Ok(records::canonical_json(&set))
}

/// `machine_host_entity(tenant, host_json, state)`: the `Machine` entity of one normalized host as
/// canonical JSON, or the shape constraint the host would break.
pub fn machine_host_entity(tenant: &str, host_json: &str, state: &str) -> Result<String, String> {
    let host: hosts::NormalizedHost =
        serde_json::from_str(host_json).map_err(|_| "host_json:malformed".to_string())?;
    let state = state_of(state)?;
    hosts::host_machine_entity(tenant, &host, state)
        .map(|entity| canonical_text(&entity))
        .map_err(|refused| refused.to_string())
}

/// `machine_component_ids(machine_iri, kind, parts_json)`: the ids of the parts of one kind, as
/// a JSON list in input order; `null` for a part that has no natural key.
pub fn machine_component_ids(
    machine_iri: &str,
    kind: &str,
    parts_json: &str,
) -> Result<String, String> {
    let kind = ComponentKind::from_tag(kind).ok_or_else(|| format!("{kind}:{UNKNOWN_KIND}"))?;
    let parts: Vec<BTreeMap<String, PropertyValue>> =
        serde_json::from_str(parts_json).map_err(|_| "parts_json:malformed".to_string())?;
    Ok(canonical_text(&records::component_ids(
        machine_iri,
        kind,
        &parts,
    )))
}

/// `machine_probe_id(machine_iri, probe_kind, probe_digest)`: the id of one probe snapshot.
pub fn machine_probe_id(machine_iri: &str, probe_kind: &str, probe_digest: &str) -> String {
    records::probe_id(machine_iri, probe_kind, probe_digest)
}

/// `machine_transition_id(machine_iri, edge_id, evidence_digest)`: the id of one transition
/// record.
pub fn machine_transition_id(machine_iri: &str, edge_id: &str, evidence_digest: &str) -> String {
    records::transition_id(machine_iri, edge_id, evidence_digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VECTORS: &str = include_str!("../../medulla-source-types/tests/vectors/machine_iri.json");

    fn vectors() -> serde_json::Value {
        serde_json::from_str(VECTORS).unwrap()
    }

    #[test]
    fn the_wrappers_return_the_recorded_vectors() {
        for row in vectors()["iri_vectors"].as_array().unwrap() {
            let kind = row["kind"].as_str().unwrap();
            let raw = row["raw"].as_str().unwrap();
            let keys = canonical_text(&json!([{"kind": kind, "value": raw}]));
            let out: serde_json::Value =
                serde_json::from_str(&machine_iri(row["tenant"].as_str().unwrap(), &keys).unwrap())
                    .unwrap();
            assert_eq!(out["iri"], row["iri"]);
            assert_eq!(out["tier"], row["kind"]);
        }
        for row in vectors()["anchor_vectors"].as_array().unwrap() {
            let out: serde_json::Value = serde_json::from_str(
                &machine_key_ref(
                    row["kind"].as_str().unwrap(),
                    row["value"].as_str().unwrap(),
                )
                .unwrap(),
            )
            .unwrap();
            assert_eq!(out["iri"], row["iri"]);
        }
    }

    #[test]
    fn the_wrappers_refuse_with_the_documented_texts() {
        assert_eq!(machine_normalize_key("uuid", "").unwrap_err(), "uuid:empty");
        assert_eq!(
            machine_normalize_key("serial", "none").unwrap_err(),
            "serial:placeholder"
        );
        assert_eq!(
            machine_normalize_key("bmc", "aa:bb").unwrap_err(),
            "bmc:malformed"
        );
        assert_eq!(
            machine_normalize_key("rack", "x").unwrap_err(),
            "rack:unknown_kind"
        );
        let nil = r#"[{"kind":"uuid","value":"00000000-0000-0000-0000-000000000000"}]"#;
        assert_eq!(machine_iri("t", nil).unwrap_err(), "no_valid_key");
        assert_eq!(machine_iri("t", "[]").unwrap_err(), "no_valid_key");
        let facts = r#"{"retire_requested":true}"#;
        assert_eq!(
            machine_lifecycle_apply("active", "Retire", facts).unwrap_err(),
            "TRANSITION_REFUSED: MachineLifecycleMachine refused 'Retire' from 'active' (illegal_transition)"
        );
        assert_eq!(
            machine_lifecycle_apply("retired", "Ready", facts).unwrap_err(),
            "TRANSITION_REFUSED: MachineLifecycleMachine refused 'Ready' from 'retired' (terminal_state)"
        );
        assert_eq!(
            machine_lifecycle_apply("commissioned", "Ready", "{}").unwrap_err(),
            "TRANSITION_REFUSED: MachineLifecycleMachine refused 'Ready' from 'commissioned' (guard_failed)"
        );
        assert_eq!(
            machine_lifecycle_apply("commissioned", "Wake", "{}").unwrap_err(),
            "TRANSITION_REFUSED: MachineLifecycleMachine refused 'Wake' from 'commissioned' (unknown_event)"
        );
        assert_eq!(
            machine_lifecycle_apply("asleep", "Ready", "{}").unwrap_err(),
            "unknown_state"
        );
    }

    #[test]
    fn lifecycle_functions_follow_the_edge_table() {
        let up = r#"{"reachable":true,"ready":true}"#;
        assert_eq!(
            machine_lifecycle_derive("commissioned", up).unwrap(),
            r#"{"event":"Ready"}"#
        );
        assert_eq!(
            machine_lifecycle_derive("active", up).unwrap(),
            r#"{"event":null}"#
        );
        assert_eq!(
            machine_lifecycle_apply("commissioned", "Ready", up).unwrap(),
            r#"{"edge_id":"H01","state":"active"}"#
        );
        assert_eq!(
            machine_lifecycle_derive("active", r#"{"x":1}"#).unwrap_err(),
            "facts_json:malformed"
        );
    }

    #[test]
    fn hosts_and_record_wrappers_are_canonical() {
        let yaml = "schema: hosts/v1\nhosts:\n  - name: alpha\n    addresses: [Alpha.Example]\n";
        let text = normalize_hosts_file(yaml).unwrap();
        assert!(text.starts_with(r#"{"hosts":[{"addresses":["alpha.example"]"#));
        assert_eq!(canonical_digest(&text), machine_hosts_digest(yaml).unwrap());
        let refusal = normalize_hosts_file("schema: hosts/v2\nhosts: []\n").unwrap_err();
        assert_eq!(
            refusal,
            r#"{"code":"HOSTS_FILE_INVALID","pointer":"/schema","reason":"schema_version"}"#
        );
        let records = r#"{"schema":"inventory-records/v1","relationships":[],"entities":[]}"#;
        assert_eq!(
            machine_canonical_json(records).unwrap(),
            r#"{"entities":[],"relationships":[],"schema":"inventory-records/v1"}"#
        );
        assert!(
            machine_canonical_json(r#"{"schema":"x","entities":[],"relationships":[]}"#).is_err()
        );
        let ids =
            machine_component_ids("urn:machine:host:1", "bond", r#"[{"bondName":"bond0"},{}]"#);
        let ids: serde_json::Value = serde_json::from_str(&ids.unwrap()).unwrap();
        assert!(ids[0].is_null() && ids[1].is_null());
        assert!(machine_component_ids("m", "gpu", "[]").is_err());
        let host = r#"{"name":"alpha","addresses":["a.example"],"management_address":null,"roles":[],"groups":[],"credentials":{"ssh":null,"management":null},"identity":{"system_uuid":null,"system_serial":"SN-0001","management_mac":null},"retired":false}"#;
        let entity: serde_json::Value =
            serde_json::from_str(&machine_host_entity("t", host, "commissioned").unwrap()).unwrap();
        assert_eq!(entity["node_type"], "Machine");
        assert_eq!(entity["properties"]["identityTier"], "serial");
        assert_eq!(
            machine_host_entity("t", host, "asleep").unwrap_err(),
            "unknown_state"
        );
        let set = format!(
            r#"{{"schema":"inventory-records/v1","entities":[{entity}],"relationships":[]}}"#
        );
        assert!(machine_check_shapes(&set).is_ok());
        let broken = set.replace("commissioned", "asleep");
        assert!(machine_check_shapes(&broken)
            .unwrap_err()
            .contains(":machineLifecycleState"));
        assert!(machine_probe_id("m", "system", "sha256:00").starts_with("urn:machine-probe:"));
        assert!(
            machine_transition_id("m", "H01", "sha256:00").starts_with("urn:machine-transition:")
        );
    }
}
