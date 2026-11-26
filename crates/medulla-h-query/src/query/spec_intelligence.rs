//! MEDULLA-SDLC-R014.13 -- the spec intelligence report.
//!
//! `Method::UnifiedQuery` with a `spec_report` field reaches [`serve`] from the same dispatch
//! arm as `code_map`. The report is a read: it takes no plan, so no write can ride on it, and
//! it writes nothing itself. Two things come out of one snapshot of the caller's graph:
//!
//! * overlap candidates: the requirement rows and the glossary terms their `usesTerm` edges
//!   reach are copied, with the `usesTerm` and `dependsOnRow` edges, into a throwaway graph that
//!   `cluster_hierarchy` clusters; a cluster whose rows carry two or more repositories is one
//!   candidate;
//! * shape violations: each requirement row and its row-level edges are rendered to Turtle and
//!   checked against the `sdd-row-shapes` source with the one SHACL validator.

use super::code_map::{out_edges, text};
use super::*;

use std::collections::{BTreeMap, BTreeSet};

use medulla_graph_core::graph::GraphView;
use medulla_types::spec_report::{
    OverlapCandidate, ShapeViolation, SpecIntelligenceReport, SpecReportQuery, REPORT_READ_ONLY,
};

const REQUIREMENT_TYPE: &str = "Requirement";
const TERM_TYPE: &str = "GlossaryTerm";
const SHAPES_SOURCE: &str = "core:sdd-row-shapes@1";
const SPEC_IRI_PREFIX: &str = "urn:medulla:spec:";
const CORE_PREFIX: &str = "https://knuckles-team.github.io/ontology/core#";

/// The relationships a row renders to Turtle, each the `:`-prefixed predicate of the same name.
const ROW_RELATIONSHIPS: [&str; 8] = [
    "dependsOnRow",
    "boundTest",
    "verifiedBy",
    "implementedByCommit",
    "seamProvider",
    "seamConsumer",
    "refusesWith",
    "statedBy",
];

/// One requirement row of the snapshot.
struct Row {
    id: String,
    row_id: String,
    repo: String,
    status: String,
    edges: Vec<(String, String)>,
    terms: BTreeSet<String>,
}

fn rows_of(view: &GraphView) -> BTreeMap<String, Row> {
    let mut rows = BTreeMap::new();
    for id in view.node_properties.keys() {
        let Some(props) = view.node_row_object(id) else {
            continue;
        };
        if text(&props, "type") != REQUIREMENT_TYPE {
            continue;
        }
        let edges = out_edges(view, id);
        let terms = edges
            .iter()
            .filter(|(_, relationship)| relationship == "usesTerm")
            .filter_map(|(target, _)| {
                let term = view.node_row_object(target)?;
                (text(&term, "type") == TERM_TYPE).then(|| text(&term, "name").to_string())
            })
            .collect();
        rows.insert(
            id.clone(),
            Row {
                id: id.clone(),
                row_id: text(&props, "rowId").to_string(),
                repo: text(&props, "repo").to_string(),
                status: text(&props, "rowStatus").to_string(),
                edges,
                terms,
            },
        );
    }
    rows
}

/// The overlap candidates: clusters of the row and term graph whose rows span repositories.
fn overlaps(view: &GraphView, rows: &BTreeMap<String, Row>, query: &SpecReportQuery) -> Vec<OverlapCandidate> {
    let core = medulla_graph_core::graph::GraphCore::new();
    let put = |id: &str, properties: serde_json::Value| {
        if let Ok(blob) = rmp_serde::to_vec_named(&properties) {
            core.put_node(id.to_string(), blob);
        }
    };
    let mut terms: BTreeSet<&str> = BTreeSet::new();
    for row in rows.values() {
        put(&row.id, serde_json::json!({"type": REQUIREMENT_TYPE, "rowId": row.row_id, "repo": row.repo}));
        for (target, relationship) in &row.edges {
            if relationship == "usesTerm" {
                terms.insert(target.as_str());
            }
        }
    }
    for term in &terms {
        if let Some(props) = view.node_row_object(term) {
            put(term, serde_json::json!({"type": TERM_TYPE, "name": text(&props, "name")}));
        }
    }
    for row in rows.values() {
        for (target, relationship) in &row.edges {
            let wanted = relationship == "usesTerm" || relationship == "dependsOnRow";
            let known = rows.contains_key(target) || terms.contains(target.as_str());
            if wanted && known {
                if let Ok(blob) = rmp_serde::to_vec_named(&serde_json::json!({"relationship": relationship})) {
                    let _ = core.add_edge(row.id.clone(), target.clone(), blob);
                }
            }
        }
    }
    let resolution = f64::from(query.resolution_micros) / 1_000_000.0;
    let clustered = medulla_graph_algos::algorithms::cluster_hierarchy(
        &core.analysis_snapshot(),
        None,
        resolution,
        query.seed,
    );
    let mut clusters: BTreeMap<u32, Vec<&Row>> = BTreeMap::new();
    for (node, cluster) in &clustered.leaf_membership {
        if let Some(row) = rows.get(node) {
            clusters.entry(*cluster).or_default().push(row);
        }
    }
    let mut found = Vec::new();
    for (index, members) in clusters {
        let repositories: BTreeSet<&str> = members.iter().map(|row| row.repo.as_str()).collect();
        if repositories.len() < 2 {
            continue;
        }
        let mut use_count: BTreeMap<&str, usize> = BTreeMap::new();
        for row in &members {
            for term in &row.terms {
                *use_count.entry(term.as_str()).or_default() += 1;
            }
        }
        let row_ids: BTreeSet<&str> = members.iter().map(|row| row.row_id.as_str()).collect();
        found.push(OverlapCandidate {
            cluster_id: format!("l1:{index}"),
            row_ids: row_ids.into_iter().map(str::to_owned).collect(),
            shared_terms: use_count
                .into_iter()
                .filter(|(_, count)| *count >= 2)
                .map(|(term, _)| term.to_owned())
                .collect(),
            repositories: repositories.into_iter().map(str::to_owned).collect(),
        });
    }
    found.sort_by(|a, b| a.cluster_id.cmp(&b.cluster_id));
    found
}

/// The IRI of a node id: characters an IRI refuses are percent-encoded.
fn iri(node_id: &str) -> String {
    let mut out = String::from(SPEC_IRI_PREFIX);
    for byte in node_id.bytes() {
        match byte {
            b' ' | b'<' | b'>' | b'"' | b'{' | b'}' | b'|' | b'\\' | b'^' | b'`' | 0..=31 | 127.. => {
                out.push_str(&format!("%{byte:02X}"))
            }
            other => out.push(other as char),
        }
    }
    out
}

fn literal(value: &str) -> String {
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// The row-level facts of the snapshot as Turtle: one subject per row with a triple per
/// property or edge R014.10 names, the predicate being the property's `:`-prefixed name.
fn turtle(rows: &BTreeMap<String, Row>) -> String {
    let mut out = format!("@prefix : <{CORE_PREFIX}> .\n");
    let status_class = |status: &str| match status {
        "LANDED" => Some(("LandedStatus", "LANDED")),
        "VERIFIED" => Some(("VerifiedStatus", "VERIFIED")),
        "SPECIFIED" => Some(("SpecifiedStatus", "SPECIFIED")),
        _ => None,
    };
    let mut statuses: BTreeSet<(&str, &str)> = BTreeSet::new();
    for row in rows.values() {
        out.push_str(&format!("<{}> a :Requirement ; :rowId {}", iri(&row.id), literal(&row.row_id)));
        if let Some((class, name)) = status_class(&row.status) {
            out.push_str(&format!(" ; :rowStatus <urn:medulla:status:{name}>"));
            statuses.insert((class, name));
        }
        for (target, relationship) in &row.edges {
            if ROW_RELATIONSHIPS.contains(&relationship.as_str()) {
                out.push_str(&format!(" ; :{relationship} <{}>", iri(target)));
            }
        }
        out.push_str(" .\n");
    }
    for (class, name) in statuses {
        out.push_str(&format!("<urn:medulla:status:{name}> a :{class} .\n"));
    }
    out
}

/// The refusal code a shape's message starts with: the text before the first `:` when it is
/// upper-case words joined by underscores.
fn code_of(message: &str) -> Option<&str> {
    let code = message.split(':').next()?.trim();
    (!code.is_empty() && code.bytes().all(|b| b.is_ascii_uppercase() || b == b'_')).then_some(code)
}

#[cfg(feature = "shacl")]
fn violations(view: &GraphView, rows: &BTreeMap<String, Row>) -> Result<Vec<ShapeViolation>, String> {
    let shapes = view
        .schema_sources
        .shapes()
        .find(|(id, _)| *id == SHAPES_SOURCE)
        .map(|(_, document)| document.to_string())
        .ok_or_else(|| format!("INTERNAL: the schema sources hold no {SHAPES_SOURCE}"))?;
    let by_iri: BTreeMap<String, &str> = rows
        .values()
        .map(|row| (format!("<{}>", iri(&row.id)), row.row_id.as_str()))
        .collect();
    let report = medulla_shacl::validate_turtle(&shapes, &turtle(rows))?;
    let mut found: BTreeSet<(String, String)> = BTreeSet::new();
    for result in &report.results {
        let (Some(row_id), Some(code)) = (
            by_iri.get(&result.focus_node),
            result.message.as_deref().and_then(code_of),
        ) else {
            continue;
        };
        found.insert(((*row_id).to_string(), code.to_string()));
    }
    Ok(found
        .into_iter()
        .map(|(row_id, code)| ShapeViolation { row_id, code })
        .collect())
}

#[cfg(not(feature = "shacl"))]
fn violations(_view: &GraphView, _rows: &BTreeMap<String, Row>) -> Result<Vec<ShapeViolation>, String> {
    Err("INTERNAL: the spec report needs the `shacl` feature to check the row shapes".to_string())
}

fn build(view: &GraphView, query: &SpecReportQuery) -> Result<SpecIntelligenceReport, String> {
    query.validate()?;
    let rows = rows_of(view);
    Ok(SpecIntelligenceReport {
        overlaps: overlaps(view, &rows, query),
        violations: violations(view, &rows)?,
        rows: u32::try_from(rows.len()).unwrap_or(u32::MAX),
    })
}

/// Serve one spec report. Authorization is the first step. `alongside` says the request also
/// carried `code_map`, `retrieval` or a foreign execution: with any of those, or with a plan,
/// the report refuses `REPORT_READ_ONLY` before anything is read.
pub(super) async fn serve(
    ctx: &QueryHandlerCtx<'_>,
    plan: &medulla_exec::algebra::Plan,
    query: &SpecReportQuery,
    alongside: bool,
) -> Response {
    let req_id = ctx.req_id;
    if let Err(denied) = required_sql_authority(req_id, ctx.read_authority) {
        return denied;
    }
    if alongside || !plan.ops.is_empty() {
        return Response::err(
            req_id,
            format!("{REPORT_READ_ONLY}: a spec_report request carries an empty plan and no other body"),
        );
    }
    if let Err(refusal) = query.validate() {
        return Response::err(req_id, refusal);
    }
    let (snap, version) = sql_read_snapshot(
        ctx.core,
        #[cfg(feature = "security")]
        ctx.caller,
        #[cfg(feature = "security")]
        ctx.rls,
    );
    let scope = ReadScope::resolve(ctx.state, ctx.read_authority, ctx.graph_name).await;
    let snap = match scope.narrow(req_id, version, snap, &["spec_report".to_string()]) {
        Ok(snap) => snap,
        Err(refusal) => return refusal,
    };
    let query = query.clone();
    let built = compute_off_lock(req_id, move || build(&snap, &query)).await;
    match built {
        Ok(Ok(report)) => {
            match ResultPayload::of_ref::<query_results::UnifiedQuerySpecReport>(&report) {
                Ok(payload) => Response::ok(req_id, payload),
                Err(error) => Response::err(req_id, error),
            }
        }
        Ok(Err(refusal)) => Response::err(req_id, refusal),
        Err(response) => response,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, repo: &str, status: &str, edges: &[(&str, &str)], terms: &[&str]) -> Row {
        Row {
            id: id.to_string(),
            row_id: id.rsplit(':').next().unwrap_or(id).to_string(),
            repo: repo.to_string(),
            status: status.to_string(),
            edges: edges
                .iter()
                .map(|(target, relationship)| (target.to_string(), relationship.to_string()))
                .collect(),
            terms: terms.iter().map(|term| term.to_string()).collect(),
        }
    }

    // spec: MEDULLA-SDLC-R014.13
    #[test]
    fn a_shape_message_names_its_code_and_a_prose_message_names_none() {
        assert_eq!(code_of("ROW_TEST_UNBOUND: the row has no bound test."), Some("ROW_TEST_UNBOUND"));
        assert_eq!(code_of("LANDED_WITHOUT_COMMIT"), Some("LANDED_WITHOUT_COMMIT"));
        assert_eq!(code_of("commitHash is one 40-character hexadecimal hash."), None);
        assert_eq!(code_of(""), None);
    }

    // spec: MEDULLA-SDLC-R014.13
    #[test]
    fn rows_render_one_subject_each_with_their_edges_and_status_class() {
        let mut rows = BTreeMap::new();
        for row in [
            row(
                "requirement:a:A-R001",
                "a",
                "LANDED",
                &[("testfile:a:t.py", "boundTest"), ("requirement:a:A-R002", "dependsOnRow")],
                &[],
            ),
            row("requirement:a:A-R002", "a", "SPECIFIED", &[], &[]),
        ] {
            rows.insert(row.id.clone(), row);
        }
        let ttl = turtle(&rows);
        assert!(ttl.contains("<urn:medulla:spec:requirement:a:A-R001> a :Requirement ; :rowId \"A-R001\""));
        assert!(ttl.contains(":rowStatus <urn:medulla:status:LANDED>"));
        assert!(ttl.contains(":boundTest <urn:medulla:spec:testfile:a:t.py>"));
        assert!(ttl.contains(":dependsOnRow <urn:medulla:spec:requirement:a:A-R002>"));
        assert!(ttl.contains("<urn:medulla:status:LANDED> a :LandedStatus ."));
        assert!(ttl.contains("<urn:medulla:status:SPECIFIED> a :SpecifiedStatus ."));
        assert_eq!(iri("a b"), "urn:medulla:spec:a%20b");
        assert_eq!(literal("a\"b"), "\"a\\\"b\"");
    }

    #[cfg(feature = "shacl")]
    // spec: MEDULLA-SDLC-R014.13
    #[test]
    fn a_landed_row_with_no_commit_and_a_one_sided_seam_are_violations() {
        use medulla_graph_core::graph::GraphCore;
        let core = GraphCore::new();
        let put = |id: &str, value: serde_json::Value| {
            core.put_node(id.to_string(), rmp_serde::to_vec_named(&value).expect("encodes"));
        };
        put(
            "requirement:a:A-R001",
            serde_json::json!({"type": "Requirement", "rowId": "A-R001", "repo": "a", "rowStatus": "LANDED"}),
        );
        put("testfile:a:t.py", serde_json::json!({"type": "TestFile"}));
        put("seam:a:x.md", serde_json::json!({"type": "SeamSide"}));
        let edge = |source: &str, target: &str, relationship: &str| {
            core.add_edge(
                source.to_string(),
                target.to_string(),
                rmp_serde::to_vec_named(&serde_json::json!({"relationship": relationship})).expect("encodes"),
            )
            .expect("endpoints exist");
        };
        edge("requirement:a:A-R001", "testfile:a:t.py", "boundTest");
        edge("requirement:a:A-R001", "seam:a:x.md", "seamProvider");
        let report = build(&core.analysis_snapshot(), &SpecReportQuery::default()).expect("builds");
        assert_eq!(report.rows, 1);
        let codes: Vec<(&str, &str)> = report
            .violations
            .iter()
            .map(|v| (v.row_id.as_str(), v.code.as_str()))
            .collect();
        assert_eq!(
            codes,
            [("A-R001", "LANDED_WITHOUT_COMMIT"), ("A-R001", "SEAM_SIDE_MISSING")]
        );
    }
}
