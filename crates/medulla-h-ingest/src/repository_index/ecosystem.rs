//! MEDULLA-REPO-INGEST-R801 — ecosystem packages and architecture layers as
//! typed nodes of the scoped repository index.
//!
//! A package is one manifest in the batch (`Cargo.toml` `[package]`,
//! `pyproject.toml` `[project]`, `package.json`). Its architecture layer is
//! the first segment of the manifest's directory (`crates/a/Cargo.toml` is in
//! layer `crates`; a manifest at the repository root is in layer `root`).
//! A package depends on another package of the batch when its manifest names
//! it as a dependency; a layer depends on another layer when one of its
//! packages depends on a package of that layer. The nodes and edges lower
//! beside the entity nodes, so they commit with the index envelope and the
//! enrichment work item reads them from the committed graph. A cycle among
//! layers refuses the batch with `LAYER_CYCLE`, before anything commits.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use medulla_types::ingestion_wire::{ExtractedEdge, ExtractedNode, IndexResult};

/// The registered refusal code of a layer dependency cycle.
pub const LAYER_CYCLE: &str = "LAYER_CYCLE";
/// Node type of one ecosystem package.
pub const PACKAGE_TYPE: &str = "EcosystemPackage";
/// Node type of one architecture layer.
pub const LAYER_TYPE: &str = "ArchitectureLayer";
/// Package → package dependency edge.
pub const DEPENDS_ON_PACKAGE: &str = "dependsOnPackage";
/// Package → its layer.
pub const IN_LAYER: &str = "inLayer";
/// Layer → layer dependency edge.
pub const DEPENDS_ON_LAYER: &str = "dependsOnLayer";
const ROOT_LAYER: &str = "root";

/// One manifest's package facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageFacts {
    pub name: String,
    pub ecosystem: &'static str,
    pub manifest_path: String,
    pub layer: String,
    pub dependencies: BTreeSet<String>,
}

pub fn package_node_id(name: &str) -> String {
    format!("package:{name}")
}

pub fn layer_node_id(name: &str) -> String {
    format!("layer:{name}")
}

fn layer_of(path: &str) -> String {
    let mut segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    segments.pop();
    segments
        .first()
        .map_or_else(|| ROOT_LAYER.to_string(), |s| (*s).to_string())
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn unquote(value: &str) -> Option<String> {
    let value = value.trim();
    let inner = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))?;
    (!inner.is_empty()).then(|| inner.to_string())
}

/// The bare distribution name of a PEP 508 requirement (`a[x]>=1` → `a`).
fn requirement_name(requirement: &str) -> Option<String> {
    let name: String = requirement
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect();
    (!name.is_empty()).then_some(name)
}

/// Line-oriented reader of the TOML subset manifests use for names and
/// dependency tables: `[section]` headers, `key = "value"` pairs and one
/// `dependencies = [ ... ]` string array (possibly spanning lines).
fn toml_manifest(text: &str, name_section: &str) -> Option<(String, BTreeSet<String>)> {
    let mut section = String::new();
    let mut name = None;
    let mut dependencies = BTreeSet::new();
    let mut in_array = false;
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        if in_array {
            for item in line.trim_end_matches(']').split(',') {
                if let Some(dep) = unquote(item).as_deref().and_then(requirement_name) {
                    dependencies.insert(dep);
                }
            }
            in_array = !line.ends_with(']');
            continue;
        }
        if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = header.trim().to_string();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().trim_matches('"');
        if section == name_section && key == "name" {
            name = unquote(value);
        } else if section == name_section && key == "dependencies" {
            let value = value.trim();
            if let Some(body) = value.strip_prefix('[') {
                for item in body.trim_end_matches(']').split(',') {
                    if let Some(dep) = unquote(item).as_deref().and_then(requirement_name) {
                        dependencies.insert(dep);
                    }
                }
                in_array = !value.ends_with(']');
            }
        } else if matches!(
            section.as_str(),
            "dependencies" | "dev-dependencies" | "build-dependencies"
        ) {
            dependencies.insert(key.to_string());
        }
    }
    name.map(|name| (name, dependencies))
}

fn json_manifest(text: &str) -> Option<(String, BTreeSet<String>)> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let name = value.get("name")?.as_str()?.to_string();
    let dependencies = ["dependencies", "devDependencies", "peerDependencies"]
        .iter()
        .filter_map(|key| value.get(*key).and_then(serde_json::Value::as_object))
        .flat_map(|deps| deps.keys().cloned())
        .collect();
    Some((name, dependencies))
}

/// The package facts of every manifest in the batch, keyed by package name.
/// A later manifest with an already-seen name is ignored (first path wins in
/// sorted order, so the choice is deterministic).
pub fn packages(files: &[(String, Vec<u8>)]) -> BTreeMap<String, PackageFacts> {
    let mut sorted: Vec<&(String, Vec<u8>)> = files.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut packages = BTreeMap::new();
    for (path, bytes) in sorted {
        let Ok(text) = std::str::from_utf8(bytes) else {
            continue;
        };
        let parsed = match file_name(path) {
            "Cargo.toml" => toml_manifest(text, "package").map(|p| (p, "cargo")),
            "pyproject.toml" => toml_manifest(text, "project").map(|p| (p, "python")),
            "package.json" => json_manifest(text).map(|p| (p, "npm")),
            _ => None,
        };
        let Some(((name, dependencies), ecosystem)) = parsed else {
            continue;
        };
        packages.entry(name.clone()).or_insert(PackageFacts {
            name,
            ecosystem,
            manifest_path: path.clone(),
            layer: layer_of(path),
            dependencies,
        });
    }
    packages
}

/// The layer dependency edges implied by in-batch package dependencies.
fn layer_edges(packages: &BTreeMap<String, PackageFacts>) -> BTreeSet<(String, String)> {
    let mut edges = BTreeSet::new();
    for package in packages.values() {
        for dependency in &package.dependencies {
            if let Some(target) = packages.get(dependency) {
                if target.layer != package.layer {
                    edges.insert((package.layer.clone(), target.layer.clone()));
                }
            }
        }
    }
    edges
}

/// A layer cycle, as the sorted layer names on it, or `None`.
pub fn find_layer_cycle(edges: &BTreeSet<(String, String)>) -> Option<Vec<String>> {
    let mut adjacency: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (from, to) in edges {
        adjacency.entry(from).or_default().push(to);
    }
    // 0 = unvisited, 1 = on the stack, 2 = done.
    let mut color: HashMap<&str, u8> = HashMap::new();
    let mut path: Vec<&str> = Vec::new();
    fn visit<'a>(
        node: &'a str,
        adjacency: &BTreeMap<&'a str, Vec<&'a str>>,
        color: &mut HashMap<&'a str, u8>,
        path: &mut Vec<&'a str>,
    ) -> Option<Vec<String>> {
        color.insert(node, 1);
        path.push(node);
        for next in adjacency.get(node).into_iter().flatten() {
            match color.get(next).copied().unwrap_or(0) {
                1 => {
                    let start = path.iter().position(|n| n == next).unwrap_or(0);
                    let mut cycle: Vec<String> =
                        path[start..].iter().map(|n| (*n).to_string()).collect();
                    cycle.sort();
                    return Some(cycle);
                }
                0 => {
                    if let Some(cycle) = visit(next, adjacency, color, path) {
                        return Some(cycle);
                    }
                }
                _ => {}
            }
        }
        path.pop();
        color.insert(node, 2);
        None
    }
    let roots: Vec<&str> = adjacency.keys().copied().collect();
    for root in roots {
        if color.get(root).copied().unwrap_or(0) == 0 {
            if let Some(cycle) = visit(root, &adjacency, &mut color, &mut path) {
                return Some(cycle);
            }
        }
    }
    None
}

fn node(id: String, node_type: &str, properties: &[(&str, &str)]) -> ExtractedNode {
    ExtractedNode {
        node_id: id,
        node_type: node_type.to_string(),
        properties: properties
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect(),
    }
}

fn edge(source: String, target: String, edge_type: &str) -> ExtractedEdge {
    ExtractedEdge {
        source,
        target,
        edge_type: edge_type.to_string(),
        properties: HashMap::from([("evidence_rung".to_string(), "EXTRACTED".to_string())]),
    }
}

/// Add the package and layer nodes and their dependency edges to `result`.
/// A cycle among layers refuses the batch with [`LAYER_CYCLE`].
pub fn project(files: &[(String, Vec<u8>)], result: &mut IndexResult) -> Result<(), String> {
    let packages = packages(files);
    if packages.is_empty() {
        return Ok(());
    }
    let layer_deps = layer_edges(&packages);
    if let Some(cycle) = find_layer_cycle(&layer_deps) {
        return Err(format!(
            "{LAYER_CYCLE}: architecture layers depend on each other in a cycle ({}); \
             the layer dependency edge set is refused",
            cycle.join(", ")
        ));
    }
    let layers: BTreeSet<&str> = packages.values().map(|p| p.layer.as_str()).collect();
    for layer in &layers {
        result
            .nodes
            .push(node(layer_node_id(layer), LAYER_TYPE, &[("name", layer)]));
    }
    for package in packages.values() {
        result.nodes.push(node(
            package_node_id(&package.name),
            PACKAGE_TYPE,
            &[
                ("name", &package.name),
                ("ecosystem", package.ecosystem),
                ("manifest_path", &package.manifest_path),
                ("layer", &package.layer),
            ],
        ));
        result.edges.push(edge(
            package_node_id(&package.name),
            layer_node_id(&package.layer),
            IN_LAYER,
        ));
        for dependency in &package.dependencies {
            if packages.contains_key(dependency) && dependency != &package.name {
                result.edges.push(edge(
                    package_node_id(&package.name),
                    package_node_id(dependency),
                    DEPENDS_ON_PACKAGE,
                ));
            }
        }
    }
    for (from, to) in &layer_deps {
        result.edges.push(edge(
            layer_node_id(from),
            layer_node_id(to),
            DEPENDS_ON_LAYER,
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, text: &str) -> (String, Vec<u8>) {
        (path.to_string(), text.as_bytes().to_vec())
    }

    // spec: MEDULLA-REPO-INGEST-R801
    #[test]
    fn packages_and_layers_become_typed_nodes_and_a_layer_cycle_is_refused() {
        let files =
            vec![
            file(
                "crates/core/Cargo.toml",
                "[package]\nname = \"core\"\n\n[dependencies]\nserde = \"1\"\n",
            ),
            file(
                "services/api/pyproject.toml",
                "[project]\nname = \"api\"\ndependencies = [\n  \"core>=1\",\n  \"httpx\",\n]\n",
            ),
            file("web/package.json", r#"{"name": "web", "dependencies": {"api": "1"}}"#),
            file("README.md", "not a manifest"),
        ];
        let mut result = IndexResult::default();
        project(&files, &mut result).expect("an acyclic layer set projects");
        let ids: BTreeSet<&str> = result.nodes.iter().map(|n| n.node_id.as_str()).collect();
        for id in [
            "package:core",
            "package:api",
            "package:web",
            "layer:crates",
            "layer:services",
            "layer:web",
        ] {
            assert!(ids.contains(id), "{id} missing from {ids:?}");
        }
        let edges: BTreeSet<(&str, &str, &str)> = result
            .edges
            .iter()
            .map(|e| (e.source.as_str(), e.target.as_str(), e.edge_type.as_str()))
            .collect();
        assert!(edges.contains(&("package:api", "package:core", DEPENDS_ON_PACKAGE)));
        assert!(edges.contains(&("package:web", "package:api", DEPENDS_ON_PACKAGE)));
        assert!(edges.contains(&("layer:services", "layer:crates", DEPENDS_ON_LAYER)));
        assert!(edges.contains(&("package:core", "layer:crates", IN_LAYER)));
        // An out-of-batch dependency is not an edge.
        assert!(!edges.iter().any(|(_, t, _)| *t == "package:serde"));

        let cyclic = vec![
            file(
                "crates/core/Cargo.toml",
                "[package]\nname = \"core\"\n[dependencies]\napi = \"1\"\n",
            ),
            file(
                "services/api/pyproject.toml",
                "[project]\nname = \"api\"\ndependencies = [\"core\"]\n",
            ),
        ];
        let mut refused = IndexResult::default();
        let error = project(&cyclic, &mut refused).expect_err("a layer cycle refuses");
        assert!(error.starts_with(LAYER_CYCLE), "{error}");
        assert!(refused.nodes.is_empty() && refused.edges.is_empty());
        assert_eq!(
            medulla_types::contract::ServerErrorCode::LayerCycle.as_str(),
            LAYER_CYCLE
        );
    }
}
