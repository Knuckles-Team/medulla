//! Architecture view derivation (MEDULLA-CQ-ORCH-R010.2, R010.4, R010.5).
//!
//! One pure function over the graph read view: deployable packages become
//! containers, cross-package `calls` edges become view edges, and ArchiMate
//! `:ApplicationComponent` diagram elements merge in by case-folded name. The
//! output is deterministic and every code container cites its ingested files.
//! No store is added; nothing is written.

use std::collections::{BTreeMap, BTreeSet};

use medulla_catalog_types::architecture_view::{
    ArchitectureView, ViewAbstained, ViewContainer, ViewEdge, ViewScope,
};
use medulla_graph_core::graph::GraphCore;
use medulla_types::scheduler_config::DEFAULT_ARCHITECTURE_VIEW_MAX_CONTAINERS;

/// Rule (1)+(2): manifest file name -> technology of the package it roots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerRule {
    pub manifests: Vec<(String, String)>,
    pub symbol_label: String,
}

impl Default for ContainerRule {
    fn default() -> Self {
        Self {
            manifests: [
                ("Cargo.toml", "Rust"),
                ("pyproject.toml", "Python"),
                ("package.json", "Node"),
                ("go.mod", "Go"),
            ]
            .into_iter()
            .map(|(m, t)| (m.to_owned(), t.to_owned()))
            .collect(),
            symbol_label: "SYMBOL".to_owned(),
        }
    }
}

/// Rule (3): the graph relationship that joins two containers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeRule {
    pub relationship: String,
}

impl Default for EdgeRule {
    fn default() -> Self {
        Self {
            relationship: "calls".to_owned(),
        }
    }
}

/// Rule (4): the diagram class whose elements merge into containers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagramMerge {
    pub labels: Vec<String>,
}

impl Default for DiagramMerge {
    fn default() -> Self {
        Self {
            labels: vec![
                "ApplicationComponent".to_owned(),
                ":ApplicationComponent".to_owned(),
            ],
        }
    }
}

/// Every derivation rule, plus the container bound from `SchedulerConfig`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewRules {
    pub container: ContainerRule,
    pub edge: EdgeRule,
    pub diagram: DiagramMerge,
    pub max_containers: usize,
}

impl Default for ViewRules {
    fn default() -> Self {
        Self {
            container: ContainerRule::default(),
            edge: EdgeRule::default(),
            diagram: DiagramMerge::default(),
            max_containers: DEFAULT_ARCHITECTURE_VIEW_MAX_CONTAINERS as usize,
        }
    }
}

type Props = serde_json::Map<String, serde_json::Value>;

fn decode(properties: &[u8]) -> Props {
    match rmp_serde::from_slice::<serde_json::Value>(properties) {
        Ok(serde_json::Value::Object(map)) => map,
        _ => Props::new(),
    }
}

fn text<'a>(props: &'a Props, key: &str) -> Option<&'a str> {
    props.get(key).and_then(serde_json::Value::as_str)
}

fn label_of(props: &Props) -> Option<&str> {
    ["type", "node_type", "label"]
        .into_iter()
        .find_map(|key| text(props, key))
}

fn path_of(props: &Props) -> Option<&str> {
    text(props, "file").or_else(|| text(props, "path"))
}

fn split_path(path: &str) -> (&str, &str) {
    let path = path.trim_start_matches("./");
    match path.rsplit_once('/') {
        Some((dir, base)) => (dir, base),
        None => ("", path),
    }
}

/// One deployable package found in scope.
struct Package {
    name: String,
    technology: String,
    support: BTreeSet<String>,
}

/// Derive the view with the default rules.
pub fn derive_architecture_view(
    core: &GraphCore,
    scope: &ViewScope,
) -> Result<ArchitectureView, ViewAbstained> {
    derive_architecture_view_with(core, scope, &ViewRules::default())
}

/// Derive the view with explicit rules.
pub fn derive_architecture_view_with(
    core: &GraphCore,
    scope: &ViewScope,
    rules: &ViewRules,
) -> Result<ArchitectureView, ViewAbstained> {
    let mut seen_repository = false;
    let mut in_scope: Vec<(String, Props)> = Vec::new();
    for (id, properties) in core.get_nodes() {
        let props = decode(&properties);
        match text(&props, "repository_id") {
            Some(repo) if repo == scope.repository_id => {
                seen_repository = true;
                in_scope.push((id, props));
            }
            Some(_) => {}
            None => in_scope.push((id, props)),
        }
    }
    if !seen_repository {
        return Err(ViewAbstained::UnknownRepository);
    }

    // (1)+(2): packages keyed by their directory.
    let mut packages: BTreeMap<String, Package> = BTreeMap::new();
    for (_, props) in &in_scope {
        let Some(path) = path_of(props) else { continue };
        let (dir, base) = split_path(path);
        let Some((_, tech)) = rules.container.manifests.iter().find(|(m, _)| m == base) else {
            continue;
        };
        let name = match dir.rsplit('/').next() {
            Some(last) if !last.is_empty() => last.to_owned(),
            _ => scope.repository_id.clone(),
        };
        packages
            .entry(dir.to_owned())
            .or_insert_with(|| Package {
                name,
                technology: tech.clone(),
                support: BTreeSet::new(),
            })
            .support
            .insert(path.trim_start_matches("./").to_owned());
    }

    // A symbol belongs to the nearest package directory above its file.
    let package_of = |file: &str| -> Option<&String> {
        let file = file.trim_start_matches("./");
        packages
            .keys()
            .filter(|dir| dir.is_empty() || file.starts_with(&format!("{dir}/")))
            .max_by_key(|dir| dir.len())
    };
    let mut symbol_owner: BTreeMap<String, String> = BTreeMap::new();
    let mut symbol_support: Vec<(String, String)> = Vec::new();
    for (id, props) in &in_scope {
        if label_of(props) != Some(rules.container.symbol_label.as_str()) {
            continue;
        }
        let Some(file) = path_of(props) else { continue };
        if let Some(dir) = package_of(file) {
            symbol_owner.insert(id.clone(), dir.clone());
            symbol_support.push((dir.clone(), file.trim_start_matches("./").to_owned()));
        }
    }
    for (dir, file) in symbol_support {
        if let Some(package) = packages.get_mut(&dir) {
            package.support.insert(file);
        }
    }

    // (3): cross-package call edges with counts.
    let mut counts: BTreeMap<(String, String), u64> = BTreeMap::new();
    for (source, target, properties) in core.get_edges() {
        let props = decode(&properties);
        if text(&props, "relationship") != Some(rules.edge.relationship.as_str()) {
            continue;
        }
        let (Some(from), Some(to)) = (symbol_owner.get(&source), symbol_owner.get(&target)) else {
            continue;
        };
        if from != to {
            *counts.entry((from.clone(), to.clone())).or_default() += 1;
        }
    }

    // Containers keyed by case-folded name; (4) diagram elements merge in.
    let mut containers: BTreeMap<String, ViewContainer> = BTreeMap::new();
    let mut name_of_dir: BTreeMap<String, String> = BTreeMap::new();
    for (dir, package) in &packages {
        let key = package.name.to_lowercase();
        name_of_dir.insert(dir.clone(), package.name.clone());
        let entry = containers.entry(key).or_insert_with(|| ViewContainer {
            name: package.name.clone(),
            technology: package.technology.clone(),
            support: Vec::new(),
        });
        entry.support.extend(package.support.iter().cloned());
    }
    for (_, props) in &in_scope {
        let Some(label) = label_of(props) else {
            continue;
        };
        if !rules.diagram.labels.iter().any(|l| l == label) {
            continue;
        }
        let Some(name) = text(props, "name").filter(|n| !n.trim().is_empty()) else {
            continue;
        };
        containers
            .entry(name.to_lowercase())
            .or_insert_with(|| ViewContainer {
                name: name.to_owned(),
                technology: String::new(),
                support: Vec::new(),
            });
    }

    if containers.is_empty() {
        return Err(ViewAbstained::NoSupportingSource);
    }
    if containers.len() > rules.max_containers {
        return Err(ViewAbstained::TooLarge {
            containers: containers.len(),
            limit: rules.max_containers,
        });
    }

    // (5): deterministic order.
    let mut containers: Vec<ViewContainer> = containers.into_values().collect();
    for container in &mut containers {
        container.support.sort();
        container.support.dedup();
    }
    containers.sort_by(|a, b| a.name.cmp(&b.name));
    let mut by_name: BTreeMap<(String, String), u64> = BTreeMap::new();
    for ((from, to), count) in counts {
        let (from, to) = (name_of_dir[&from].clone(), name_of_dir[&to].clone());
        if from != to {
            *by_name.entry((from, to)).or_default() += count;
        }
    }
    let mut edges: Vec<ViewEdge> = by_name
        .into_iter()
        .map(|((from, to), call_count)| ViewEdge {
            from,
            to,
            kind: rules.edge.relationship.clone(),
            call_count,
        })
        .collect();
    edges.sort_by(|a, b| (&a.from, &a.to, &a.kind).cmp(&(&b.from, &b.to, &b.kind)));
    Ok(ArchitectureView { containers, edges })
}
