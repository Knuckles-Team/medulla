//! One-shot data migration of every retired ontology namespace (medulla rename,
//! 2026-10-30; single organization ontology base, 2026-11-12): every stored IRI
//! under a retired namespace in [`NAMESPACE_REWRITES`] is rewritten to its
//! replacement under the one ontology base, and every retired class IRI in
//! [`RETIRED_TERMS`] (a renamed or merged class under that base) is rewritten to
//! the class that replaces it.
//!
//! Stored node ids, edge endpoints and every MessagePack string inside node and
//! edge properties are rewritten. The pass runs after the listeners bind, as
//! part of post-bind maintenance, and writes through the ordinary mutation
//! gateway as the fixed service principal [`MIGRATION_ACTOR`]: every rewrite is
//! durable, ledgered and replicated like any caller's write, with no storage
//! bypass. It is idempotent: a graph holding no retired IRI plans no writes, so
//! after the first successful pass every later start is a read-only scan. The
//! pass report names every retired namespace it found and rewrote.
//!
//! medulla-rename-codemod: keep-old-names (this module must name the retired
//! namespaces; the rename codemod skips files carrying this marker).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use tokio::sync::RwLock;

use medulla_protocol_types::protocol::Method;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;

/// The fixed service principal the migration writes as.
#[cfg(feature = "security")]
pub const MIGRATION_ACTOR: &str = "service:namespace-migration";
const MIGRATION_SERVICE: &str = "namespace-migration";

/// Every retired namespace and the IRI that replaces its prefix, most specific
/// first; the first matching entry wins. An old IRI ending in `#` or `/` is a
/// prefix (the rest of the IRI is kept); any other old IRI is a whole IRI and
/// matches only when no name character follows it. The replacements derive from
/// the one ontology base. The 14 governed-concept paths that existed under both
/// legacy product tokens merge into one concept each, so both token paths map to
/// the same prefix.
pub const NAMESPACE_REWRITES: &[(&str, &str)] = &[
    // The legacy `au:` alias: two names live in core, everything else in infrastructure.
    (
        "http://agent-utilities.dev/ontology#HardwareNode",
        medulla_types::ontology_iri!("core#HardwareNode"),
    ),
    (
        "http://agent-utilities.dev/ontology#Agent",
        medulla_types::ontology_iri!("core#Agent"),
    ),
    (
        "http://agent-utilities.dev/ontology#WorldModel",
        medulla_types::ontology_iri!("core#WorldModel"),
    ),
    (
        "https://agent-utilities.dev/ontology#HardwareNode",
        medulla_types::ontology_iri!("core#HardwareNode"),
    ),
    (
        "https://agent-utilities.dev/ontology#Agent",
        medulla_types::ontology_iri!("core#Agent"),
    ),
    (
        "https://agent-utilities.dev/ontology#WorldModel",
        medulla_types::ontology_iri!("core#WorldModel"),
    ),
    (
        "http://agent-utilities.dev/ontology#",
        medulla_types::ontology_iri!("infrastructure#"),
    ),
    (
        "https://agent-utilities.dev/ontology#",
        medulla_types::ontology_iri!("infrastructure#"),
    ),
    (
        "http://agent-utilities.dev/ontology",
        medulla_types::ontology_iri!("infrastructure"),
    ),
    (
        "https://agent-utilities.dev/ontology",
        medulla_types::ontology_iri!("infrastructure"),
    ),
    // The engine's runtime assertion vocabulary (both retired spellings).
    (
        "http://medulla/owl#",
        medulla_types::ontology_iri!("reasoning#"),
    ),
    (
        "http://epistemic-graph/owl#",
        medulla_types::ontology_iri!("reasoning#"),
    ),
    // Governed concepts, schemes and pillars.
    (
        "http://knuckles.team/kg/concept/AU/",
        medulla_types::ontology_iri!("concept#"),
    ),
    (
        "http://knuckles.team/kg/concept/EG/",
        medulla_types::ontology_iri!("concept#"),
    ),
    (
        "http://knuckles.team/kg/scheme/AU",
        medulla_types::ontology_iri!("concept-scheme#governed-concepts"),
    ),
    (
        "http://knuckles.team/kg/scheme/EG",
        medulla_types::ontology_iri!("concept-scheme#governed-concepts"),
    ),
    (
        "http://knuckles.team/kg/pillar/",
        medulla_types::ontology_iri!("pillar#"),
    ),
    // Modules renamed for their domain or moved under the connector prefix.
    (
        "http://knuckles.team/kg/arbora#",
        medulla_types::ontology_iri!("admission#"),
    ),
    (
        "http://knuckles.team/kg/bellwether-mcp/shape#",
        medulla_types::ontology_iri!("connector/bellwether-shapes#"),
    ),
    (
        "http://knuckles.team/kg/artcraft/workflows#",
        medulla_types::ontology_iri!("connector/artcraft-workflows#"),
    ),
    (
        "http://knuckles.team/kg/artcraft/",
        medulla_types::ontology_iri!("connector/"),
    ),
    (
        "http://knuckles.team/kg/arr",
        medulla_types::ontology_iri!("connector/arr"),
    ),
    (
        "http://knuckles.team/kg/arr#",
        medulla_types::ontology_iri!("connector/arr#"),
    ),
    (
        "http://knuckles.team/kg/qbittorrent",
        medulla_types::ontology_iri!("connector/qbittorrent"),
    ),
    (
        "http://knuckles.team/kg/qbittorrent#",
        medulla_types::ontology_iri!("connector/qbittorrent#"),
    ),
    (
        "http://knuckles.team/kg/erpnext",
        medulla_types::ontology_iri!("connector/erpnext"),
    ),
    (
        "http://knuckles.team/kg/erpnext#",
        medulla_types::ontology_iri!("connector/erpnext#"),
    ),
    (
        "http://knuckles.team/kg/emerald-finance",
        medulla_types::ontology_iri!("connector/emerald-finance"),
    ),
    (
        "http://knuckles.team/kg/emerald-finance#",
        medulla_types::ontology_iri!("connector/emerald-finance#"),
    ),
    (
        "https://knuckles-team.github.io/agent-connector-sdk/access#",
        medulla_types::ontology_iri!("access#"),
    ),
    (
        "https://knuckles-team.github.io/scion-sdk/access#",
        medulla_types::ontology_iri!("access#"),
    ),
    (
        "https://knuckles-team.github.io/scion-sdk/lifecycle",
        medulla_types::ontology_iri!("connector-lifecycle"),
    ),
    (
        "https://knuckles-team.github.io/scion-sdk/lifecycle#",
        medulla_types::ontology_iri!("connector-lifecycle#"),
    ),
    (
        "https://knuckles-team.github.io/scion-sdk/breaker#",
        medulla_types::ontology_iri!("circuit-breaker#"),
    ),
    (
        "https://knuckles-team.github.io/scion-sdk/stream",
        medulla_types::ontology_iri!("stream-transport"),
    ),
    (
        "https://knuckles-team.github.io/scion-sdk/stream#",
        medulla_types::ontology_iri!("stream-transport#"),
    ),
    (
        "https://knuckles-team.github.io/scion-sdk/contract",
        medulla_types::ontology_iri!("surface-contract"),
    ),
    (
        "https://knuckles-team.github.io/scion-sdk/contract#",
        medulla_types::ontology_iri!("surface-contract#"),
    ),
    (
        "http://knuckles.team/xsrc#",
        medulla_types::ontology_iri!("source-record#"),
    ),
    (
        "http://knuckles.team/xsrc/",
        medulla_types::ontology_iri!("source-record/"),
    ),
    (
        "http://knuckles.team/test/",
        medulla_types::ontology_iri!("test/"),
    ),
    (
        "https://example.invalid/ontology/",
        medulla_types::ontology_iri!("connector/"),
    ),
    // The shared core vocabulary, its document, and every other `kg/<module>`.
    (
        "http://knuckles.team/kg#",
        medulla_types::ontology_iri!("core#"),
    ),
    (
        "http://knuckles.team/kg",
        medulla_types::ontology_iri!("core"),
    ),
    ("http://knuckles.team/kg/", medulla_types::ontology_base!()),
];

/// Every retired class IRI under the one ontology base and the IRI that replaces
/// it (MEDULLA-TYPED-PACKS-R105.10.3: one local name per class across the core
/// namespaces). A class whose name repeated a core class's name is renamed in its
/// own namespace, or merged into the core class when it is the same concept. Each
/// entry is a whole IRI: it matches only when no name character follows it.
pub const RETIRED_TERMS: &[(&str, &str)] = &[
    (
        medulla_types::ontology_iri!("catalog#GlossaryTerm"),
        medulla_types::ontology_iri!("catalog#CatalogGlossaryTerm"),
    ),
    (
        medulla_types::ontology_iri!("catalog#Person"),
        medulla_types::ontology_iri!("catalog#PersonAsset"),
    ),
    (
        medulla_types::ontology_iri!("catalog#Role"),
        medulla_types::ontology_iri!("catalog#RoleAsset"),
    ),
    (
        medulla_types::ontology_iri!("catalog#Service"),
        medulla_types::ontology_iri!("catalog#ServiceAsset"),
    ),
    (
        medulla_types::ontology_iri!("catalog#Team"),
        medulla_types::ontology_iri!("catalog#TeamAsset"),
    ),
    (
        medulla_types::ontology_iri!("catalog#WorkflowDefinition"),
        medulla_types::ontology_iri!("catalog#WorkflowDefinitionAsset"),
    ),
    (
        medulla_types::ontology_iri!("catalog#WorkflowStep"),
        medulla_types::ontology_iri!("catalog#WorkflowStepAsset"),
    ),
    (
        medulla_types::ontology_iri!("connector-pack#AccessPolicy"),
        medulla_types::ontology_iri!("connector-pack#IngestAccessPolicy"),
    ),
    (
        medulla_types::ontology_iri!("connector-pack#Feature"),
        medulla_types::ontology_iri!("connector-pack#SurfaceFeature"),
    ),
    (
        medulla_types::ontology_iri!("connector-pack#Prompt"),
        medulla_types::ontology_iri!("connector-pack#SurfacePrompt"),
    ),
    (
        medulla_types::ontology_iri!("connector-pack#Skill"),
        medulla_types::ontology_iri!("connector-pack#SurfaceSkill"),
    ),
    (
        medulla_types::ontology_iri!("connector-pack#SourceObject"),
        medulla_types::ontology_iri!("connector-pack#SourceObjectMapping"),
    ),
    (
        medulla_types::ontology_iri!("connector-pack#Tool"),
        medulla_types::ontology_iri!("connector-pack#SurfaceTool"),
    ),
    (
        medulla_types::ontology_iri!("data-platform#AccessPolicy"),
        medulla_types::ontology_iri!("data-platform#NamedAccessPolicy"),
    ),
    (
        medulla_types::ontology_iri!("data-platform#Model"),
        medulla_types::ontology_iri!("data-platform#TrainedModel"),
    ),
    (
        medulla_types::ontology_iri!("data-platform#Partition"),
        medulla_types::ontology_iri!("data-platform#TopicPartition"),
    ),
    (
        medulla_types::ontology_iri!("data-platform#Topic"),
        medulla_types::ontology_iri!("data-platform#StreamTopic"),
    ),
    (
        medulla_types::ontology_iri!("downtime-work#Budget"),
        medulla_types::ontology_iri!("downtime-work#ResourceBudget"),
    ),
    (
        medulla_types::ontology_iri!("identity-metadata#Preference"),
        medulla_types::ontology_iri!("identity-metadata#ScopedPreference"),
    ),
    (
        medulla_types::ontology_iri!("infrastructure#Deployment"),
        medulla_types::ontology_iri!("infrastructure#DeploymentWorkload"),
    ),
    (
        medulla_types::ontology_iri!("sdd-spec#Goal"),
        medulla_types::ontology_iri!("sdd-spec#DeliveryGoal"),
    ),
    (
        medulla_types::ontology_iri!("sdd-spec#Phase"),
        medulla_types::ontology_iri!("sdd-spec#DeliveryPhase"),
    ),
    (
        medulla_types::ontology_iri!("sdd-spec#Requirement"),
        medulla_types::ontology_iri!("sdd-spec#RequirementRow"),
    ),
    (
        medulla_types::ontology_iri!("sdd-spec#Spec"),
        medulla_types::ontology_iri!("sdd-spec#SpecFolder"),
    ),
    (
        medulla_types::ontology_iri!("sdd-spec#Task"),
        medulla_types::ontology_iri!("sdd-spec#TaskLine"),
    ),
    (
        medulla_types::ontology_iri!("data-platform#Collection"),
        medulla_types::ontology_iri!("core#DocumentCollection"),
    ),
    (
        medulla_types::ontology_iri!("data-platform#Column"),
        medulla_types::ontology_iri!("core#Column"),
    ),
    (
        medulla_types::ontology_iri!("data-platform#Database"),
        medulla_types::ontology_iri!("core#Database"),
    ),
    (
        medulla_types::ontology_iri!("data-platform#Dataset"),
        medulla_types::ontology_iri!("core#Dataset"),
    ),
    (
        medulla_types::ontology_iri!("data-platform#Schema"),
        medulla_types::ontology_iri!("core#Schema"),
    ),
    (
        medulla_types::ontology_iri!("data-platform#Table"),
        medulla_types::ontology_iri!("core#Table"),
    ),
    (
        medulla_types::ontology_iri!("infrastructure#Incident"),
        medulla_types::ontology_iri!("core#Incident"),
    ),
];

/// Host fragments every entry of [`NAMESPACE_REWRITES`] contains; a quick filter.
const RETIRED_MARKERS: [&str; 7] = [
    "knuckles.team/",
    "medulla/owl#",
    "epistemic-graph/owl#",
    "agent-utilities.dev/ontology",
    "example.invalid/ontology/",
    "knuckles-team.github.io/agent-connector-sdk/",
    "knuckles-team.github.io/scion-sdk/",
];

fn name_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'#' | b'/')
}

/// The first rewrite entry matching the start of `tail`, with the byte length of
/// the old text it covers.
fn matching_rewrite(tail: &str) -> Option<(usize, &'static str, &'static str)> {
    RETIRED_TERMS
        .iter()
        .chain(NAMESPACE_REWRITES)
        .find_map(|(old, new)| {
            let rest = tail.strip_prefix(old)?;
            let prefix = old.ends_with(['#', '/']);
            let whole = prefix || !rest.as_bytes().first().copied().is_some_and(name_char);
            whole.then_some((old.len(), *old, *new))
        })
}

/// Whether `text` may hold a retired IRI: a retired-namespace host fragment or a
/// retired class IRI (a quick filter; matching decides).
fn may_hold_retired(text: &[u8]) -> bool {
    let holds = |needle: &str| {
        text.windows(needle.len())
            .any(|window| window == needle.as_bytes())
    };
    RETIRED_MARKERS.iter().any(|marker| holds(marker))
        || RETIRED_TERMS.iter().any(|(old, _)| holds(old))
}

/// Rewrite `text` until it holds no retired IRI: a retired-namespace IRI whose
/// replacement is itself a retired class IRI is rewritten twice. Replacements
/// never hold a retired IRI, so the second pass is the last.
fn rewrite_traced(text: &str, mut seen: Option<&mut BTreeSet<&'static str>>) -> Option<String> {
    let mut current = rewrite_pass(text, seen.as_deref_mut())?;
    while let Some(next) = rewrite_pass(&current, seen.as_deref_mut()) {
        current = next;
    }
    Some(current)
}

fn rewrite_pass(text: &str, mut seen: Option<&mut BTreeSet<&'static str>>) -> Option<String> {
    if !may_hold_retired(text.as_bytes()) {
        return None;
    }
    let mut out = String::with_capacity(text.len() + 32);
    let mut rest = text;
    let mut changed = false;
    while let Some(at) = rest.find("http") {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        match matching_rewrite(tail) {
            Some((len, old, new)) => {
                out.push_str(new);
                rest = &tail[len..];
                changed = true;
                if let Some(seen) = seen.as_deref_mut() {
                    seen.insert(old);
                }
            }
            None => {
                out.push_str("http");
                rest = &tail[4..];
            }
        }
    }
    out.push_str(rest);
    changed.then_some(out)
}

/// `text` with every retired-namespace occurrence rewritten, or `None` when it
/// holds none.
pub fn rewrite_text(text: &str) -> Option<String> {
    rewrite_traced(text, None)
}

fn holds_retired(bytes: &[u8]) -> bool {
    may_hold_retired(bytes)
}

/// Every retired namespace or term (as spelled in [`NAMESPACE_REWRITES`] and
/// [`RETIRED_TERMS`]) that `bytes` still holds.
fn namespaces_in(bytes: &[u8], seen: &mut BTreeSet<&'static str>) {
    if !holds_retired(bytes) {
        return;
    }
    let text = String::from_utf8_lossy(bytes);
    let _ = rewrite_traced(&text, Some(seen));
}

/// A MessagePack document with every string value (map keys included)
/// rewritten, or `None` when it holds no retired IRI. Every other value is
/// copied byte for byte; a malformed document is an error, never truncated.
pub fn rewrite_msgpack(bytes: &[u8]) -> Result<Option<Vec<u8>>, String> {
    if !holds_retired(bytes) {
        return Ok(None);
    }
    let mut out = Vec::with_capacity(bytes.len() + 16);
    let mut cursor = 0usize;
    let mut changed = false;
    walk(bytes, &mut cursor, &mut out, &mut changed, 0)?;
    if cursor != bytes.len() {
        return Err(format!(
            "trailing bytes after MessagePack value at {cursor}"
        ));
    }
    Ok(changed.then_some(out))
}

const MAX_DEPTH: usize = 512;

fn take<'a>(bytes: &'a [u8], cursor: &mut usize, n: usize) -> Result<&'a [u8], String> {
    let end = cursor
        .checked_add(n)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| format!("truncated MessagePack value at {cursor}"))?;
    let slice = &bytes[*cursor..end];
    *cursor = end;
    Ok(slice)
}

fn be_len(raw: &[u8]) -> usize {
    raw.iter()
        .fold(0usize, |acc, byte| (acc << 8) | usize::from(*byte))
}

fn write_str(out: &mut Vec<u8>, text: &str) {
    let len = text.len();
    if len < 32 {
        out.push(0xa0 | len as u8);
    } else if len <= u8::MAX as usize {
        out.extend_from_slice(&[0xd9, len as u8]);
    } else if len <= u16::MAX as usize {
        out.push(0xda);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(0xdb);
        out.extend_from_slice(&(len as u32).to_be_bytes());
    }
    out.extend_from_slice(text.as_bytes());
}

fn walk(
    bytes: &[u8],
    cursor: &mut usize,
    out: &mut Vec<u8>,
    changed: &mut bool,
    depth: usize,
) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err("MessagePack nesting too deep".into());
    }
    let start = *cursor;
    let marker = take(bytes, cursor, 1)?[0];
    // (header bytes after the marker, payload bytes, child values)
    let (header, payload, children): (usize, usize, usize) = match marker {
        0x00..=0x7f | 0xe0..=0xff | 0xc0 | 0xc2 | 0xc3 => (0, 0, 0),
        0x80..=0x8f => (0, 0, 2 * usize::from(marker & 0x0f)),
        0x90..=0x9f => (0, 0, usize::from(marker & 0x0f)),
        0xa0..=0xbf | 0xd9 | 0xda | 0xdb => {
            let len = match marker {
                0xd9 => be_len(take(bytes, cursor, 1)?),
                0xda => be_len(take(bytes, cursor, 2)?),
                0xdb => be_len(take(bytes, cursor, 4)?),
                _ => usize::from(marker & 0x1f),
            };
            let raw = take(bytes, cursor, len)?;
            match std::str::from_utf8(raw).ok().and_then(rewrite_text) {
                Some(text) => {
                    *changed = true;
                    write_str(out, &text);
                }
                None => out.extend_from_slice(&bytes[start..*cursor]),
            }
            return Ok(());
        }
        0xc4 => (1, be_len(bytes.get(*cursor..*cursor + 1).unwrap_or(&[])), 0),
        0xc5 => (2, be_len(bytes.get(*cursor..*cursor + 2).unwrap_or(&[])), 0),
        0xc6 => (4, be_len(bytes.get(*cursor..*cursor + 4).unwrap_or(&[])), 0),
        0xc7 => (
            1,
            be_len(bytes.get(*cursor..*cursor + 1).unwrap_or(&[])) + 1,
            0,
        ),
        0xc8 => (
            2,
            be_len(bytes.get(*cursor..*cursor + 2).unwrap_or(&[])) + 1,
            0,
        ),
        0xc9 => (
            4,
            be_len(bytes.get(*cursor..*cursor + 4).unwrap_or(&[])) + 1,
            0,
        ),
        0xca => (0, 4, 0),
        0xcb => (0, 8, 0),
        0xcc | 0xd0 => (0, 1, 0),
        0xcd | 0xd1 => (0, 2, 0),
        0xce | 0xd2 => (0, 4, 0),
        0xcf | 0xd3 => (0, 8, 0),
        0xd4 => (0, 2, 0),
        0xd5 => (0, 3, 0),
        0xd6 => (0, 5, 0),
        0xd7 => (0, 9, 0),
        0xd8 => (0, 17, 0),
        0xdc => (2, 0, be_len(bytes.get(*cursor..*cursor + 2).unwrap_or(&[]))),
        0xdd => (4, 0, be_len(bytes.get(*cursor..*cursor + 4).unwrap_or(&[]))),
        0xde => (
            2,
            0,
            2 * be_len(bytes.get(*cursor..*cursor + 2).unwrap_or(&[])),
        ),
        0xdf => (
            4,
            0,
            2 * be_len(bytes.get(*cursor..*cursor + 4).unwrap_or(&[])),
        ),
        0xc1 => return Err(format!("invalid MessagePack marker 0xc1 at {start}")),
    };
    take(bytes, cursor, header + payload)?;
    out.extend_from_slice(&bytes[start..*cursor]);
    for _ in 0..children {
        walk(bytes, cursor, out, changed, depth + 1)?;
    }
    Ok(())
}

/// The writes that move one graph's nodes and edges off the retired namespace,
/// in apply order: affected edges are removed first, then nodes are rewritten
/// (a renamed node is removed and re-added under its new id), then the edges
/// are re-added between the rewritten endpoints. Empty when nothing is stored
/// under the retired namespace.
pub fn plan_graph(
    nodes: &[(String, Vec<u8>)],
    edges: &[(String, String, Vec<u8>)],
) -> Result<Vec<Method>, String> {
    let mut renamed: BTreeMap<&str, String> = BTreeMap::new();
    let mut node_writes = Vec::new();
    for (node_id, properties) in nodes {
        let new_id = rewrite_text(node_id);
        let new_props =
            rewrite_msgpack(properties).map_err(|error| format!("node {node_id}: {error}"))?;
        if new_id.is_none() && new_props.is_none() {
            continue;
        }
        let properties_msgpack = new_props.unwrap_or_else(|| properties.clone());
        if let Some(new_id) = new_id {
            node_writes.push(Method::RemoveNode {
                node_id: node_id.clone(),
            });
            renamed.insert(node_id.as_str(), new_id.clone());
            node_writes.push(Method::AddNode {
                node_id: new_id,
                properties_msgpack,
            });
        } else {
            node_writes.push(Method::AddNode {
                node_id: node_id.clone(),
                properties_msgpack,
            });
        }
    }
    let endpoint = |id: &String| -> String {
        renamed
            .get(id.as_str())
            .cloned()
            .or_else(|| rewrite_text(id))
            .unwrap_or_else(|| id.clone())
    };
    // An edge pair is rewritten when an endpoint is renamed or any of its
    // (possibly parallel) edges holds a retired IRI; RemoveEdge drops every
    // edge of the pair, so all of them are re-added.
    let mut affected: BTreeSet<(&str, &str)> = BTreeSet::new();
    for (source, target, properties) in edges {
        if endpoint(source) != *source || endpoint(target) != *target || holds_retired(properties) {
            affected.insert((source.as_str(), target.as_str()));
        }
    }
    let mut writes: Vec<Method> = affected
        .iter()
        .map(|(source, target)| Method::RemoveEdge {
            source_id: (*source).to_string(),
            target_id: (*target).to_string(),
            relationship: None,
        })
        .collect();
    writes.extend(node_writes);
    for (source, target, properties) in edges {
        if !affected.contains(&(source.as_str(), target.as_str())) {
            continue;
        }
        let properties_msgpack = rewrite_msgpack(properties)
            .map_err(|error| format!("edge {source}->{target}: {error}"))?
            .unwrap_or_else(|| properties.clone());
        writes.push(Method::AddEdge {
            source_id: endpoint(source),
            target_id: endpoint(target),
            properties_msgpack,
        });
    }
    Ok(writes)
}

/// The name the migration registers under in the task registry (MEDULLA-OBSERVE-R003.5).
pub const TASK_NAME: &str = "namespace-migration";

/// The code a refusal carries when its text names no declared code.
pub const MIGRATION_REFUSED: &str = "MIGRATION_REFUSED";

/// Run the migration over every registered graph as a background task registered as
/// `namespace-migration`; its cursor is the graph being visited (MEDULLA-OBSERVE-R003.5). The
/// task ends `Failed` with the code `MIGRATION_REFUSED` when any graph was refused.
pub fn spawn(state: Arc<RwLock<ServerState>>) {
    medulla_observe::spawn::spawn(
        medulla_observe::registry::tasks::TaskSpec {
            name: TASK_NAME.to_string(),
            owner: "h-admin".to_string(),
            kind: medulla_observe::registry::tasks::TaskKind::Migration,
        },
        async move {
            let started = std::time::Instant::now();
            let report = run(&state).await;
            // The pass always logs one summary, also when it wrote nothing.
            tracing::info!(
                graphs = report.graphs,
                writes = report.writes,
                refusals = report.refusals.len(),
                namespaces = ?report.namespaces,
                "post-bind maintenance: ontology namespace migration finished in {:.3}s \
                 (retired namespaces rewritten onto the one ontology base)",
                started.elapsed().as_secs_f64()
            );
            for refusal in &report.refusals {
                tracing::warn!(
                    graph = %refusal.graph,
                    code = %refusal.code,
                    detail = %refusal.detail,
                    "ontology namespace migration refused"
                );
            }
            if report.refusals.is_empty() {
                Ok(())
            } else {
                Err(format!(
                    "{MIGRATION_REFUSED}: {} graph(s) refused",
                    report.refusals.len()
                ))
            }
        },
    );
}

/// One graph the pass refused, with the typed code of its refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationRefusal {
    /// The graph that was refused.
    pub graph: String,
    /// The declared code of the refusal, or [`MIGRATION_REFUSED`] when the text names none.
    pub code: String,
    /// The refusal's text; it names the cause and never holds graph content.
    pub detail: String,
}

/// What one migration pass did.
#[derive(Debug, Default)]
pub struct MigrationReport {
    /// Graphs the pass visited.
    pub graphs: usize,
    /// Writes the pass submitted.
    pub writes: usize,
    /// The graphs the pass refused, in the order it met them.
    pub refusals: Vec<MigrationRefusal>,
    /// Every retired namespace found stored and rewritten, in the spelling of
    /// [`NAMESPACE_REWRITES`].
    pub namespaces: BTreeSet<&'static str>,
}

/// The typed code a refusal text starts with, or [`MIGRATION_REFUSED`].
fn refusal_code(text: &str) -> String {
    use medulla_observe::spawn::FailureCode;
    match text.to_string().failure_code().as_str() {
        "INTERNAL" => MIGRATION_REFUSED.to_string(),
        code => code.to_string(),
    }
}

/// Run one migration pass over every registered graph. Each graph records one startup step
/// (`namespace_migration`, with the graph) whose outcome is `Ok`, `Skipped` when it held no
/// retired namespace, or `Refused(code)`; each graph is `Migrating` while it is visited, and a
/// refused graph is `Degraded` with the reason `MIGRATION_REFUSED` (MEDULLA-OBSERVE-R008.3).
pub async fn run(state: &Arc<RwLock<ServerState>>) -> MigrationReport {
    use medulla_observe::machine::readiness::{DegradeReason, ReadinessEvent};
    let mut report = MigrationReport::default();
    let graphs = state.read().await.registry.list();
    for (graph, _) in graphs {
        report.graphs += 1;
        medulla_observe::spawn::progress(
            medulla_observe::registry::tasks::ProgressCursor::opaque(graph.clone()),
            report.graphs as u64,
        );
        let step = medulla_observe::startup().step("namespace_migration", &graph);
        // A graph that is still opening is migrating while the pass visits it; a graph in any
        // other state has no such edge and the refusal is the machine's answer.
        let _ = medulla_observe::readiness().apply(&graph, ReadinessEvent::BeginMigrate, None);
        let writes_before = report.writes;
        match migrate_graph(state, &graph, &mut report).await {
            Ok(()) if report.writes == writes_before => step.skipped("no retired namespace stored"),
            Ok(()) => step.ok(),
            Err(error) => {
                let code = refusal_code(&error);
                step.refused(&code);
                let _ = medulla_observe::readiness().degrade(
                    &graph,
                    DegradeReason {
                        code: MIGRATION_REFUSED.to_string(),
                        task: Some(TASK_NAME.to_string()),
                        detail: format!("namespace migration refused ({code})"),
                    },
                );
                report.refusals.push(MigrationRefusal {
                    graph: graph.clone(),
                    code,
                    detail: error,
                });
            }
        }
    }
    report
}

async fn migrate_graph(
    state: &Arc<RwLock<ServerState>>,
    graph: &str,
    report: &mut MigrationReport,
) -> Result<(), String> {
    // A lazily registered graph is opened by an ordinary read first.
    if !state.read().await.registry.is_resident(graph) {
        submit(state, graph, Method::NodeIds).await?;
    }
    let core = {
        let state = state.read().await;
        let Some(entry) = state.registry.get(graph) else {
            return Ok(());
        };
        entry.core.clone()
    };
    let (nodes, edges) = (core.get_nodes(), core.get_edges());
    let writes = plan_graph(&nodes, &edges)?;
    if !writes.is_empty() {
        for (id, properties) in &nodes {
            namespaces_in(id.as_bytes(), &mut report.namespaces);
            namespaces_in(properties, &mut report.namespaces);
        }
        for (source, target, properties) in &edges {
            namespaces_in(source.as_bytes(), &mut report.namespaces);
            namespaces_in(target.as_bytes(), &mut report.namespaces);
            namespaces_in(properties, &mut report.namespaces);
        }
    }
    for method in writes {
        submit(state, graph, method).await?;
        report.writes += 1;
    }
    Ok(())
}

async fn submit(
    state: &Arc<RwLock<ServerState>>,
    graph: &str,
    method: Method,
) -> Result<(), String> {
    let (anonymous, server) = {
        let state = state.read().await;
        (state.local_anonymous, state.context.clone())
    };
    #[cfg(feature = "security")]
    if !anonymous {
        grant::ensure(&mut state.write().await.isolation, graph)?;
    }
    let context = if anonymous {
        VerifiedRequestContext::anonymous_local(Some(format!("{MIGRATION_SERVICE}:{graph}")))
    } else {
        let service = VerifiedRequestContext::authenticated_fixed_service_actor(
            &server,
            MIGRATION_SERVICE,
            &["kg:read", "kg:write"],
        )?;
        VerifiedRequestContext::from_verified_claims_with_nonce(
            service.claims().clone(),
            format!("{MIGRATION_SERVICE}:{graph}"),
            Some(medulla_types::contract::Nonce::minted()),
        )
    };
    let request = medulla_protocol_types::protocol::Request {
        id: 0,
        graph: graph.to_string(),
        auth_token: String::new(),
        agent_id: Some(context.agent_id().to_string()),
        method,
        trace_id: None,
    };
    let response =
        medulla_server_api::api::reentry::dispatch_verified_request(state, request, context).await;
    match response.refusal_text() {
        None => Ok(()),
        Some(refusal) => Err(refusal.to_string()),
    }
}

#[cfg(feature = "security")]
mod grant {
    use medulla_isolation::isolation::IsolationLayer;
    use medulla_server_api::service_grant::ServiceGraphGrant;

    const GRANT: ServiceGraphGrant<'static> = ServiceGraphGrant {
        actor: super::MIGRATION_ACTOR,
        role_prefix: "namespace-migration",
        unbootstrapped: "NAMESPACE_MIGRATION_POLICY_UNBOOTSTRAPPED",
    };

    pub fn ensure(isolation: &mut IsolationLayer, graph: &str) -> Result<(), String> {
        GRANT.ensure(isolation, graph)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(value: &serde_json::Value) -> Vec<u8> {
        rmp_serde::to_vec_named(value).unwrap()
    }

    fn decode(bytes: &[u8]) -> serde_json::Value {
        rmp_serde::from_slice(bytes).unwrap()
    }

    #[test]
    fn msgpack_strings_are_rewritten_and_everything_else_is_kept() {
        let long_tail = "x".repeat(300);
        let value = serde_json::json!({
            "type": "http://epistemic-graph/owl#Agent",
            "n": 7, "f": 1.5, "ok": true, "none": null,
            "list": ["http://epistemic-graph/owl#a", "keep", -3],
            "nested": {"http://epistemic-graph/owl#key": format!("http://epistemic-graph/owl#{long_tail}")},
        });
        let rewritten = rewrite_msgpack(&props(&value)).unwrap().expect("changed");
        let text = serde_json::to_string(&decode(&rewritten)).unwrap();
        assert!(!text.contains("epistemic-graph/owl#"));
        let expected = serde_json::from_str::<serde_json::Value>(
            &serde_json::to_string(&value).unwrap().replace(
                "http://epistemic-graph/owl#",
                medulla_types::ontology_iri!("reasoning#"),
            ),
        )
        .unwrap();
        assert_eq!(decode(&rewritten), expected);
        // Idempotent: the rewritten document holds nothing to migrate.
        assert_eq!(rewrite_msgpack(&rewritten).unwrap(), None);
    }

    #[test]
    fn malformed_msgpack_is_refused_not_truncated() {
        // The retired IRI stays whole; the cut removes the trailing value, so the
        // document still looks migratable but its last map entry is missing.
        let mut bytes = props(&serde_json::json!({"t": "http://epistemic-graph/owl#A", "n": 1}));
        bytes.truncate(bytes.len() - 1);
        assert!(rewrite_msgpack(&bytes).is_err());
    }

    #[test]
    fn plan_renames_nodes_and_rewires_their_edges() {
        let old = "http://epistemic-graph/owl#Agent".to_string();
        let new = medulla_types::ontology_iri!("reasoning#Agent").to_string();
        let nodes = vec![
            (old.clone(), props(&serde_json::json!({"label": "Agent"}))),
            (
                "plain".to_string(),
                props(&serde_json::json!({"type": old.clone()})),
            ),
            (
                "untouched".to_string(),
                props(&serde_json::json!({"type": "Thing"})),
            ),
        ];
        let edges = vec![
            (
                "plain".to_string(),
                old.clone(),
                props(&serde_json::json!({"rel": "is"})),
            ),
            (
                "untouched".to_string(),
                "plain".to_string(),
                props(&serde_json::json!({})),
            ),
        ];
        let writes = plan_graph(&nodes, &edges).unwrap();
        let names: Vec<&'static str> = writes.iter().map(|m| m.into()).collect();
        assert_eq!(
            names,
            ["RemoveEdge", "RemoveNode", "AddNode", "AddNode", "AddEdge"]
        );
        match &writes[2] {
            Method::AddNode { node_id, .. } => assert_eq!(node_id, &new),
            other => panic!("unexpected {other:?}"),
        }
        match &writes[3] {
            Method::AddNode {
                node_id,
                properties_msgpack,
            } => {
                assert_eq!(node_id, "plain");
                assert_eq!(decode(properties_msgpack)["type"], serde_json::json!(new));
            }
            other => panic!("unexpected {other:?}"),
        }
        match &writes[4] {
            Method::AddEdge {
                source_id,
                target_id,
                ..
            } => {
                assert_eq!(
                    (source_id.as_str(), target_id.as_str()),
                    ("plain", new.as_str())
                );
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn migrated_graph_plans_nothing() {
        let nodes = vec![(
            medulla_types::ontology_iri!("reasoning#Agent").to_string(),
            props(&serde_json::json!({"t": "x"})),
        )];
        assert!(plan_graph(&nodes, &[]).unwrap().is_empty());
    }

    #[test]
    fn every_retired_namespace_maps_under_the_one_base() {
        let base = medulla_types::ontology_base::ONTOLOGY_BASE;
        for (old, new) in NAMESPACE_REWRITES {
            assert!(new.starts_with(base), "{old} -> {new}");
            assert!(!old.starts_with(base), "{old} is already under the base");
            let sample = if old.ends_with(['#', '/']) {
                format!("{old}X")
            } else {
                (*old).to_string()
            };
            let rewritten = rewrite_text(&sample).unwrap_or_else(|| panic!("{old} not rewritten"));
            assert!(rewritten.starts_with(base), "{old} -> {rewritten}");
            assert_eq!(rewrite_text(&rewritten), None, "{old} is not idempotent");
        }
    }

    // spec: MEDULLA-TYPED-PACKS-R105.10.3
    #[test]
    fn every_retired_term_maps_to_a_live_term_under_the_one_base() {
        let base = medulla_types::ontology_base::ONTOLOGY_BASE;
        let olds: BTreeSet<&str> = RETIRED_TERMS.iter().map(|(old, _)| *old).collect();
        assert_eq!(
            olds.len(),
            RETIRED_TERMS.len(),
            "one entry per retired term"
        );
        for (old, new) in RETIRED_TERMS {
            assert!(
                old.starts_with(base) && new.starts_with(base),
                "{old} -> {new}"
            );
            assert!(!old.ends_with(['#', '/']), "{old} is a whole IRI");
            assert!(!olds.contains(new), "{new} is itself retired");
            assert_eq!(rewrite_text(old).as_deref(), Some(*new), "{old}");
            assert_eq!(rewrite_text(new), None, "{new} is not idempotent");
            // A longer live name that starts with the retired one is kept.
            assert_eq!(rewrite_text(&format!("{old}Shape")), None, "{old}Shape");
        }
        let tool = medulla_types::ontology_iri!("connector-pack#Tool");
        let surface = medulla_types::ontology_iri!("connector-pack#SurfaceTool");
        assert_eq!(
            rewrite_text(&format!("<{tool}> <{tool}>")).as_deref(),
            Some(format!("<{surface}> <{surface}>").as_str())
        );
        // A retired-namespace IRI whose rewrite is a retired term lands on the
        // live term in one call.
        assert_eq!(
            rewrite_text("http://knuckles.team/kg/connector-pack#Tool").as_deref(),
            Some(surface)
        );
        let mut seen = BTreeSet::new();
        namespaces_in(tool.as_bytes(), &mut seen);
        assert_eq!(seen.into_iter().collect::<Vec<_>>(), [tool]);
    }

    #[test]
    fn rewrites_follow_the_published_map() {
        let cases = [
            ("http://knuckles.team/kg#Person", "core#Person"),
            ("http://knuckles.team/kg", "core"),
            (
                "http://knuckles.team/kg/infrastructure#Host",
                "infrastructure#Host",
            ),
            ("http://knuckles.team/kg/commercial", "commercial"),
            (
                "http://knuckles.team/kg/arbora#CredentialLease",
                "admission#CredentialLease",
            ),
            (
                "http://knuckles.team/kg/artcraft/cadcraft#Part",
                "connector/cadcraft#Part",
            ),
            (
                "http://knuckles.team/kg/concept/AU/KG/memory",
                "concept#KG/memory",
            ),
            (
                "http://knuckles.team/kg/concept/EG/KG/memory",
                "concept#KG/memory",
            ),
            (
                "http://knuckles.team/kg/scheme/AU",
                "concept-scheme#governed-concepts",
            ),
            (
                "http://knuckles.team/kg/scheme/EG",
                "concept-scheme#governed-concepts",
            ),
            ("http://knuckles.team/kg/pillar/ORCH", "pillar#ORCH"),
            (
                "http://knuckles.team/xsrc#supplier-acme",
                "source-record#supplier-acme",
            ),
            ("http://knuckles.team/test/recorded#a", "test/recorded#a"),
            ("http://medulla/owl#confidence", "reasoning#confidence"),
            (
                "http://epistemic-graph/owl#confidence",
                "reasoning#confidence",
            ),
            (
                "http://agent-utilities.dev/ontology#HardwareNode",
                "core#HardwareNode",
            ),
            (
                "https://agent-utilities.dev/ontology#PlatformService",
                "infrastructure#PlatformService",
            ),
            (
                "https://knuckles-team.github.io/scion-sdk/breaker#Open",
                "circuit-breaker#Open",
            ),
            (
                "https://knuckles-team.github.io/agent-connector-sdk/access#AccessContract",
                "access#AccessContract",
            ),
        ];
        for (old, new) in cases {
            assert_eq!(
                rewrite_text(old).as_deref(),
                Some(format!("{}{new}", medulla_types::ontology_base::ONTOLOGY_BASE).as_str()),
                "{old}"
            );
        }
        // A look-alike host is not a retired namespace.
        assert_eq!(rewrite_text("http://knuckles.team/kgx"), None);
        // Text around a retired IRI is kept.
        assert_eq!(
            rewrite_text("a <http://knuckles.team/kg#A> b").as_deref(),
            Some(
                format!(
                    "a <{}core#A> b",
                    medulla_types::ontology_base::ONTOLOGY_BASE
                )
                .as_str()
            )
        );
    }

    #[test]
    fn the_report_names_each_retired_namespace_it_rewrote() {
        let mut seen = BTreeSet::new();
        namespaces_in(b"http://medulla/owl#a http://knuckles.team/kg#b", &mut seen);
        assert_eq!(
            seen.into_iter().collect::<Vec<_>>(),
            ["http://knuckles.team/kg#", "http://medulla/owl#"]
        );
    }
}
