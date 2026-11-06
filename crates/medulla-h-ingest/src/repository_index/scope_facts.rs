//! MEDULLA-REPO-INGEST-R601.3 / R602.3 — commit history, HTTP routes, IaC
//! resources and service path prefixes as typed nodes of the scoped index.
//!
//! The scope's optional `commits` lower to one `RepositoryCommit` node each; a
//! scope without commits carries no history. Routes and IaC resources are read
//! from the batch's files by line, and every package of the batch (R801) owns
//! the directory of its manifest as a service path prefix. All of them lower
//! beside the entity nodes, so they commit with the index envelope and the
//! enrichment work item reads them back from the committed graph.

use std::collections::BTreeSet;

use medulla_enrich::history::CommitRecord;
use medulla_enrich::Id;
use medulla_types::contract::ServerErrorCode;
use medulla_types::ingestion_wire::{IndexCommit, IndexRepositoryScope, IndexResult};

use super::ecosystem::{node, package_node_id, packages};

/// Node type of one commit of the scope.
pub const COMMIT_TYPE: &str = "RepositoryCommit";
/// Node type of one HTTP route found in the batch.
pub const ROUTE_TYPE: &str = "HttpRoute";
/// Node type of one infrastructure resource found in the batch.
pub const IAC_TYPE: &str = "IacResource";
/// Node type of one service path prefix.
pub const SERVICE_PREFIX_TYPE: &str = "ServicePrefix";

const VERBS: [&str; 5] = ["get", "post", "put", "delete", "patch"];
const OPENERS: [&str; 3] = [".", "#[", "@"];
const ROUTE_EXTENSIONS: [&str; 7] = ["py", "rs", "js", "ts", "go", "java", "kt"];

fn refuse(code: ServerErrorCode, detail: impl std::fmt::Display) -> String {
    format!("{}: {detail}", code.as_str())
}

fn extension(path: &str) -> &str {
    path.rsplit_once('.').map_or("", |(_, ext)| ext)
}

/// The first quoted literal of `rest` that is a URL path.
fn path_literal(rest: &str) -> Option<String> {
    let start = rest.find(['"', '\''])?;
    let quote = rest[start..].chars().next()?;
    let body = &rest[start + 1..];
    let end = body.find(quote)?;
    body[..end]
        .starts_with('/')
        .then(|| body[..end].to_string())
}

fn route_in_line(line: &str) -> Option<(String, String)> {
    for verb in VERBS {
        for opener in OPENERS {
            let marker = format!("{opener}{verb}(");
            let found = line
                .find(&marker)
                .and_then(|at| path_literal(&line[at + marker.len()..]));
            if let Some(path) = found {
                return Some((verb.to_uppercase(), path));
            }
        }
    }
    let at = line.find(".route(")?;
    Some((
        "ANY".to_string(),
        path_literal(&line[at + ".route(".len()..])?,
    ))
}

fn terraform_resource(line: &str) -> Option<(String, String)> {
    let rest = line.trim().strip_prefix("resource ")?;
    let mut quoted = rest.split('"').skip(1).step_by(2);
    Some((quoted.next()?.to_string(), quoted.next()?.to_string()))
}

/// Kubernetes-style manifests: a top-level `kind:` and the `name:` under the
/// next `metadata:`.
fn manifest_resources(text: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let (mut kind, mut in_metadata) = (None::<String>, false);
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("kind:") {
            kind = Some(value.trim().to_string()).filter(|k| k.starts_with(char::is_uppercase));
            in_metadata = false;
        } else if line.starts_with("metadata:") {
            in_metadata = true;
        } else if in_metadata {
            if let (Some(name), Some(kind)) = (line.trim().strip_prefix("name:"), kind.as_ref()) {
                found.push((
                    kind.clone(),
                    name.trim().trim_matches(['"', '\'']).to_string(),
                ));
                in_metadata = false;
            }
        }
    }
    found
}

fn resources_in(path: &str, text: &str) -> Vec<(String, String)> {
    match extension(path) {
        "tf" => text.lines().filter_map(terraform_resource).collect(),
        "yaml" | "yml" => manifest_resources(text),
        _ => Vec::new(),
    }
}

fn validate_commit(commit: &IndexCommit) -> Result<(), String> {
    let record = CommitRecord::new(
        &commit.digest,
        Id::new("author").map_err(|e| e.detail)?,
        commit.timestamp_ms,
        commit.parents.clone(),
        Vec::new(),
    );
    record
        .map(|_| ())
        .map_err(|error| refuse(ServerErrorCode::InvalidCommitDigest, error.detail))
}

fn commit_node(
    commit: &IndexCommit,
) -> Result<medulla_types::ingestion_wire::ExtractedNode, String> {
    let changes = serde_json::to_string(commit.changes.as_slice())
        .map_err(|error| refuse(ServerErrorCode::InvalidFacts, error))?;
    let parents = commit.parents.join(",");
    let timestamp = commit.timestamp_ms.to_string();
    Ok(node(
        format!("commit:{}", commit.digest),
        COMMIT_TYPE,
        &[
            ("author", &commit.author),
            ("timestamp_ms", &timestamp),
            ("parents", &parents),
            ("changes", &changes),
        ],
    ))
}

fn add_commits(scope: &IndexRepositoryScope, result: &mut IndexResult) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for commit in scope.commits.iter() {
        validate_commit(commit)?;
        if seen.insert(commit.digest.as_str()) {
            result.nodes.push(commit_node(commit)?);
        }
    }
    Ok(())
}

fn add_routes_and_resources(files: &[(String, Vec<u8>)], result: &mut IndexResult) {
    let mut seen = BTreeSet::new();
    for (path, bytes) in files {
        let Ok(text) = std::str::from_utf8(bytes) else {
            continue;
        };
        if ROUTE_EXTENSIONS.contains(&extension(path)) {
            for (method, route) in text.lines().filter_map(route_in_line) {
                let id = format!("http-route:{method}:{route}@{path}");
                if seen.insert(id.clone()) {
                    let props = [
                        ("method", &*method),
                        ("path", &*route),
                        ("file", path.as_str()),
                    ];
                    result.nodes.push(node(id, ROUTE_TYPE, &props));
                }
            }
        }
        for (kind, name) in resources_in(path, text) {
            let id = format!("iac-resource:{path}:{kind}.{name}");
            if seen.insert(id.clone()) {
                let props = [("kind", &*kind), ("name", &*name), ("file", path.as_str())];
                result.nodes.push(node(id, IAC_TYPE, &props));
            }
        }
    }
}

fn add_service_prefixes(files: &[(String, Vec<u8>)], result: &mut IndexResult) {
    for package in packages(files).values() {
        let Some((directory, _)) = package.manifest_path.rsplit_once('/') else {
            continue;
        };
        let service = package_node_id(&package.name);
        let prefix = format!("{directory}/");
        result.nodes.push(node(
            format!("service-prefix:{}", package.name),
            SERVICE_PREFIX_TYPE,
            &[("service", &service), ("path_prefix", &prefix)],
        ));
    }
}

/// Add the commit, route, resource and service-prefix nodes to `result`. A
/// commit with a malformed digest refuses the batch with
/// `INVALID_COMMIT_DIGEST`, before anything commits.
pub fn project(
    files: &[(String, Vec<u8>)],
    scope: &IndexRepositoryScope,
    result: &mut IndexResult,
) -> Result<(), String> {
    add_commits(scope, result)?;
    add_routes_and_resources(files, result);
    add_service_prefixes(files, result);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_resources_and_terraform_are_read_line_by_line() {
        assert_eq!(
            route_in_line("@app.get(\"/invoices\")"),
            Some(("GET".into(), "/invoices".into()))
        );
        assert_eq!(
            route_in_line("#[post(\"/pay\")]"),
            Some(("POST".into(), "/pay".into()))
        );
        assert_eq!(
            route_in_line("router.route('/x', handler)"),
            Some(("ANY".into(), "/x".into()))
        );
        assert_eq!(route_in_line("let x = cache.get(\"key\");"), None);
        assert_eq!(
            terraform_resource("resource \"aws_s3_bucket\" \"invoices\" {"),
            Some(("aws_s3_bucket".into(), "invoices".into()))
        );
        let manifest = "apiVersion: v1\nkind: Deployment\nmetadata:\n  name: billing\n";
        assert_eq!(
            manifest_resources(manifest),
            vec![("Deployment".to_string(), "billing".to_string())]
        );
    }
}
