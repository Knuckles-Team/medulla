//! MEDULLA-CONTRACT-R011.1: validate the land gate's fan-out host-group manifest.
//!
//! `cargo xtask fanout-host-groups [manifest]` (default `.config/fanout-host-groups.yml`).
//! The land gate fans work out across independent build-host groups, each with one persistent
//! warm target and a headroom reservation strictly between 0 and 1; no host may sit in two
//! groups. The check takes no action on any host; it only validates the declared groups.

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

pub const NAME: &str = "fanout-host-groups";
pub const SUMMARY: &str = "the land gate's fan-out host-group manifest is structurally valid";

const DEFAULT_MANIFEST: &str = ".config/fanout-host-groups.yml";

pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let path = args
        .first()
        .map_or_else(|| root.join(DEFAULT_MANIFEST), Into::into);
    let groups = load_manifest(&path)?;
    let problems = validate_groups(&groups);
    if problems.is_empty() {
        println!(
            "fanout-host-groups: PASS: {} disjoint group(s), each with a warm target and headroom reservation",
            groups.len()
        );
        return Ok(());
    }
    Err(problems
        .iter()
        .map(|p| format!("  {p}"))
        .collect::<Vec<_>>()
        .join("\n"))
}

pub fn load_manifest(path: &Path) -> Result<Vec<Value>, String> {
    let raw = fs::read_to_string(path)
        .map_err(|e| format!("CANNOT RUN: could not read {}: {e}", path.display()))?;
    let document: Value = serde_yaml_ng::from_str(&raw)
        .map_err(|e| format!("CANNOT RUN: {} is not valid YAML: {e}", path.display()))?;
    document
        .get("groups")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| {
            format!(
                "CANNOT RUN: {} has no top-level 'groups' list",
                path.display()
            )
        })
}

fn non_empty_string(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
}

/// Every human-readable problem in order; empty means valid.
pub fn validate_groups(groups: &[Value]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut seen_hosts: BTreeMap<String, String> = BTreeMap::new();
    let mut seen_names: BTreeSet<String> = BTreeSet::new();
    for (index, group) in groups.iter().enumerate() {
        let label = format!("groups[{index}]");
        let Some(group) = group.as_object() else {
            problems.push(format!("{label} is not a mapping"));
            continue;
        };
        let name = match non_empty_string(group.get("name")) {
            Some(name) => {
                if seen_names.contains(name) {
                    problems.push(format!("group '{name}' is declared more than once"));
                }
                name.to_string()
            }
            None => {
                problems.push(format!("{label} has no non-empty 'name'"));
                label.clone()
            }
        };
        seen_names.insert(name.clone());
        if non_empty_string(group.get("warm_target")).is_none() {
            problems.push(format!("group '{name}' has no non-empty 'warm_target'"));
        }
        let headroom_ok = group
            .get("headroom_reservation")
            .and_then(Value::as_f64)
            .is_some_and(|h| h > 0.0 && h < 1.0);
        if !headroom_ok {
            problems.push(format!(
                "group '{name}' has an invalid 'headroom_reservation' (must be a number strictly between 0 and 1)"
            ));
        }
        match group.get("hosts").and_then(Value::as_array) {
            Some(hosts) if !hosts.is_empty() => {
                for host in hosts {
                    let Some(host) = host.as_str().filter(|h| !h.trim().is_empty()) else {
                        problems.push(format!("group '{name}' has a non-string host entry"));
                        continue;
                    };
                    if let Some(first) = seen_hosts.get(host) {
                        problems.push(format!(
                            "host '{host}' is claimed by both '{first}' and '{name}'"
                        ));
                    } else {
                        seen_hosts.insert(host.to_string(), name.clone());
                    }
                }
            }
            _ => problems.push(format!("group '{name}' has no non-empty 'hosts' list")),
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn group(name: &str, warm: &str, headroom: Value, hosts: &[&str]) -> Value {
        json!({"name": name, "warm_target": warm, "headroom_reservation": headroom, "hosts": hosts})
    }

    #[test]
    fn disjoint_fully_specified_manifest_is_valid() {
        let groups = [
            group("group-a", "warm-a", json!(0.1), &["host-a1", "host-a2"]),
            group("group-b", "warm-b", json!(0.2), &["host-b1"]),
        ];
        assert!(validate_groups(&groups).is_empty());
    }

    #[test]
    fn overlapping_host_assignment_is_rejected() {
        let groups = [
            group("group-a", "warm-a", json!(0.1), &["shared-host"]),
            group("group-b", "warm-b", json!(0.2), &["shared-host"]),
        ];
        let problems = validate_groups(&groups);
        assert!(problems
            .iter()
            .any(|p| p.contains("shared-host") && p.contains("claimed by both")));
    }

    #[test]
    fn missing_headroom_reservation_is_rejected() {
        let groups = [json!({"name": "group-a", "warm_target": "warm-a", "hosts": ["host-a1"]})];
        assert!(validate_groups(&groups)
            .iter()
            .any(|p| p.contains("headroom_reservation")));
    }

    #[test]
    fn repository_default_manifest_is_valid() {
        let path = crate::workspace_root().join(DEFAULT_MANIFEST);
        assert!(validate_groups(&load_manifest(&path).expect("manifest loads")).is_empty());
    }

    #[test]
    fn invalid_headroom_values_remain_rejected() {
        let bad = [
            Value::Null,
            json!(true),
            json!(false),
            json!("0.1"),
            json!(0),
            json!(1),
            json!(-0.1),
        ];
        for headroom in bad {
            let groups = [group("a", "warm", headroom.clone(), &["host"])];
            assert_eq!(
                validate_groups(&groups),
                vec![
                    "group 'a' has an invalid 'headroom_reservation' (must be a number strictly between 0 and 1)"
                        .to_string()
                ],
                "headroom {headroom}"
            );
        }
    }

    #[test]
    fn yaml_nan_and_infinity_headroom_are_rejected() {
        for text in [".nan", ".inf"] {
            let yaml = format!(
                "groups:\n  - name: a\n    warm_target: w\n    headroom_reservation: {text}\n    hosts: [h]\n"
            );
            let document: Value = serde_yaml_ng::from_str(&yaml).expect("yaml parses");
            let groups = document["groups"].as_array().expect("groups").clone();
            assert_eq!(validate_groups(&groups).len(), 1, "{text}");
        }
    }

    #[test]
    fn all_group_problems_remain_ordered_and_host_ownership_preserved() {
        let groups = [
            Value::Null,
            json!({"hosts": [null, "shared", "shared"]}),
            group("groups[1]", "warm", json!(0.2), &["shared"]),
            group("empty", " ", json!(0.1), &[]),
        ];
        assert_eq!(
            validate_groups(&groups),
            vec![
                "groups[0] is not a mapping",
                "groups[1] has no non-empty 'name'",
                "group 'groups[1]' has no non-empty 'warm_target'",
                "group 'groups[1]' has an invalid 'headroom_reservation' (must be a number strictly between 0 and 1)",
                "group 'groups[1]' has a non-string host entry",
                "host 'shared' is claimed by both 'groups[1]' and 'groups[1]'",
                "group 'groups[1]' is declared more than once",
                "host 'shared' is claimed by both 'groups[1]' and 'groups[1]'",
                "group 'empty' has no non-empty 'warm_target'",
                "group 'empty' has no non-empty 'hosts' list",
            ]
        );
    }
}
