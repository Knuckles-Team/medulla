//! The nested step-input shield declaration (MEDULLA-ORCH-VOCAB-R005.100).
//!
//! A step contract carries the JSON Schema of the step's input and one source slot per
//! declared field. Medulla declares the schema, the declared field paths and one projected
//! digest over them; it applies no projection and sends nothing. The harness applies the
//! projection before dispatch and checks it against `projected_digest`. A schema whose digest no
//! longer matches the source revision it was read at refuses `SHIELD_SCHEMA_STALE`.

use std::collections::{BTreeMap, BTreeSet};

use medulla_decision_types::decision::{StepContract, StepShield};
use medulla_types::contract::{BoundedVec, Digest256};

/// The refusal code of a step schema that no longer matches its source revision.
pub(crate) const SHIELD_SCHEMA_STALE: &str = "SHIELD_SCHEMA_STALE";

/// The digest domain of one step's projected shield.
const PROJECTED_DOMAIN: &[u8] = b"medulla/step-shield-projected/v1";

fn invalid(what: impl std::fmt::Display) -> String {
    format!("ASSEMBLY_INPUTS_INVALID: step_contracts: {what}")
}

/// The canonical bytes of a JSON value: object keys in sorted order, no whitespace.
fn canonical(value: &serde_json::Value, out: &mut String) {
    match value {
        serde_json::Value::Object(map) => {
            let sorted: BTreeMap<&String, &serde_json::Value> = map.iter().collect();
            out.push('{');
            for (index, (key, item)) in sorted.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::Value::String(key.clone()).to_string());
                out.push(':');
                canonical(item, out);
            }
            out.push('}');
        }
        serde_json::Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                canonical(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// `sha256:<hex>` of the canonical bytes of `schema`.
pub(crate) fn schema_digest(schema: &serde_json::Value) -> String {
    let mut text = String::new();
    canonical(schema, &mut text);
    format!("sha256:{}", Digest256::sha256(text.as_bytes()).to_hex())
}

/// The dotted paths of the leaf fields a schema declares, in sorted order. A schema with no
/// `properties` is itself the one leaf (the empty path is refused by the caller).
fn declared_paths(schema: &serde_json::Value, prefix: &str, out: &mut BTreeSet<String>) {
    match schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
    {
        Some(properties) if !properties.is_empty() => {
            for (name, child) in properties {
                let path = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}.{name}")
                };
                declared_paths(child, &path, out);
            }
        }
        _ => {
            if !prefix.is_empty() {
                out.insert(prefix.to_string());
            }
        }
    }
}

fn slot_digest(slot: &str) -> Vec<u8> {
    Digest256::sha256(slot.as_bytes()).as_bytes().to_vec()
}

/// Every contract must name a step of the request's tasks (the steps read from the graph).
///
/// An unknown step id refuses `INVALID_PARAMS`.
pub(crate) fn check_steps_exist(
    contracts: &[StepContract],
    steps: &[medulla_decision_types::decision::AssemblyStep],
) -> Result<(), String> {
    let known: BTreeSet<&str> = steps.iter().map(|step| step.step.as_str()).collect();
    match contracts
        .iter()
        .find(|contract| !known.contains(contract.step_id.as_str()))
    {
        Some(contract) => Err(format!(
            "INVALID_PARAMS: step_contracts: step {} is not a step of the request's tasks",
            contract.step_id
        )),
        None => Ok(()),
    }
}

/// Declare the shield of every step contract. Pure: reads nothing, writes nothing.
///
/// Refuses `SHIELD_SCHEMA_STALE` when a contract's `source_schema_digest` is not the digest of
/// its `input_schema`; `ASSEMBLY_INPUTS_INVALID` when a step id repeats, a schema declares no
/// field, a source slot names an undeclared field, or a declared field has no source slot.
pub(crate) fn shield_step_schema(contracts: &[StepContract]) -> Result<Vec<StepShield>, String> {
    let mut seen = BTreeSet::new();
    let mut shields = Vec::with_capacity(contracts.len());
    for contract in contracts {
        if !seen.insert(contract.step_id.as_str()) {
            return Err(invalid(format!(
                "step {} is declared twice",
                contract.step_id
            )));
        }
        let digest = schema_digest(&contract.input_schema);
        if digest != contract.source_schema_digest {
            return Err(format!(
                "{SHIELD_SCHEMA_STALE}: step {} schema {} no longer matches the source \
                 revision schema {}",
                contract.step_id, digest, contract.source_schema_digest
            ));
        }
        let mut declared = BTreeSet::new();
        declared_paths(&contract.input_schema, "", &mut declared);
        if declared.is_empty() {
            return Err(invalid(format!(
                "step {} declares no input field",
                contract.step_id
            )));
        }
        let mut slots: BTreeMap<&str, &str> = BTreeMap::new();
        for map in contract.source_slots.as_slice() {
            if !declared.contains(&map.field_path) {
                return Err(invalid(format!(
                    "step {} maps undeclared field {}",
                    contract.step_id, map.field_path
                )));
            }
            if slots
                .insert(map.field_path.as_str(), map.source_slot.as_str())
                .is_some()
            {
                return Err(invalid(format!(
                    "step {} maps field {} twice",
                    contract.step_id, map.field_path
                )));
            }
        }
        if let Some(missing) = declared
            .iter()
            .find(|path| !slots.contains_key(path.as_str()))
        {
            return Err(invalid(format!(
                "step {} field {missing} has no source slot",
                contract.step_id
            )));
        }
        let mut fields: Vec<Vec<u8>> = vec![
            contract.step_id.as_bytes().to_vec(),
            digest.as_bytes().to_vec(),
        ];
        fields.extend(slots.values().map(|slot| slot_digest(slot)));
        let refs: Vec<&[u8]> = fields.iter().map(Vec::as_slice).collect();
        let projected = Digest256::framed(PROJECTED_DOMAIN, &refs)?;
        shields.push(StepShield {
            step_id: contract.step_id.clone(),
            schema_digest: digest,
            declared_paths: BoundedVec::new(declared.into_iter().collect())
                .map_err(|error| invalid(error))?,
            projected_digest: format!("sha256:{}", projected.to_hex()),
        });
    }
    Ok(shields)
}
