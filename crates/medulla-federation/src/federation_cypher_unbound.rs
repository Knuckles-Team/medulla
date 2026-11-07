//! MEDULLA-DURABLE-KERNEL-R024.3: the Neo4j and FalkorDb halves of the Cypher
//! transpiler's `ForeignSourceSpec` source.
//!
//! [`crate::federation_age`] already binds `Cypher { backend: Age, .. }`
//! through the existing guarded PostgreSQL reader. Neo4j speaks Bolt and
//! FalkorDb speaks its own RESP-based protocol; neither pure-Rust client
//! exists in this workspace yet, so this slice is the typed validation half
//! only (CONTRACT: ship the typed model plus refusal test when the bound
//! driver is a later child). A spec that is well-formed is validated and
//! then explicitly REFUSED with a named, typed reason -- never silently
//! accepted as if a driver existed, and never a generic/opaque error.

use medulla_types::wire::{ForeignCypherBackend, ForeignSourceSpec};

use crate::federation::ForeignSourceRegistry;

const MAX_GRAPH_BYTES: usize = 128;
const MAX_QUERY_BYTES: usize = 1024 * 1024;
const MAX_FIELD_BYTES: usize = 1024;

/// Validate a Neo4j `Cypher` spec's shape, then refuse: no bound Bolt driver
/// exists yet. Never registers anything.
pub fn register_neo4j(
    _registry: &mut ForeignSourceRegistry,
    _name: impl Into<String>,
    spec: &ForeignSourceSpec,
) -> Result<(), String> {
    validate_cypher_shape(spec, ForeignCypherBackend::Neo4j, "Neo4j")?;
    Err(
        "federation: Neo4j driver is not yet bound (MEDULLA-DURABLE-KERNEL-R024.3); \
         AGE is the only bound Cypher backend"
            .into(),
    )
}

/// Validate a FalkorDb `Cypher` spec's shape, then refuse: no bound RESP
/// driver exists yet. Never registers anything.
pub fn register_falkordb(
    _registry: &mut ForeignSourceRegistry,
    _name: impl Into<String>,
    spec: &ForeignSourceSpec,
) -> Result<(), String> {
    validate_cypher_shape(spec, ForeignCypherBackend::FalkorDb, "FalkorDb")?;
    Err(
        "federation: FalkorDb driver is not yet bound (MEDULLA-DURABLE-KERNEL-R024.3); \
         AGE is the only bound Cypher backend"
            .into(),
    )
}

/// Shared shape validation for the not-yet-bound Cypher dialects. Mirrors
/// `federation_age::AgeSource::new`'s field checks without the PostgreSQL
/// endpoint/credential binding that dialect alone has today.
fn validate_cypher_shape(
    spec: &ForeignSourceSpec,
    expected: ForeignCypherBackend,
    label: &str,
) -> Result<(), String> {
    let ForeignSourceSpec::Cypher {
        backend,
        endpoint,
        graph,
        query,
        id_field,
        score_field,
    } = spec
    else {
        return Err(format!("federation: expected a {label} Cypher source"));
    };
    if *backend != expected {
        return Err(format!("federation: expected a {label} Cypher source"));
    }
    if endpoint.is_empty() || endpoint.len() > MAX_FIELD_BYTES {
        return Err(format!("federation: invalid {label} endpoint"));
    }
    identifier(graph, MAX_GRAPH_BYTES, label, "graph")?;
    identifier(id_field, MAX_FIELD_BYTES, label, "id field")?;
    if let Some(score) = score_field {
        identifier(score, MAX_FIELD_BYTES, label, "score field")?;
        if score == id_field {
            return Err(format!(
                "federation: {label} score and id fields must differ"
            ));
        }
    }
    if query.trim().is_empty() || query.len() > MAX_QUERY_BYTES || query.contains('\0') {
        return Err(format!("federation: invalid {label} query"));
    }
    Ok(())
}

fn identifier(value: &str, max_len: usize, label: &str, field: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > max_len
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err(format!("federation: invalid {label} {field}"));
    }
    Ok(())
}

/// MEDULLA-FEDERATED-QUERY-R053.100: the one quoting rule for every generated
/// `Cypher` property key. A key is always rendered as a backtick-quoted
/// (escaped symbolic name) identifier; an embedded backtick doubles. Values never
/// pass through here: they stay query parameters. Never reuses SQL quoting.
pub fn quote_property_key(key: &str) -> Result<String, CypherKeyInvalid> {
    if key.is_empty() || key.len() > MAX_FIELD_BYTES || key.contains('\0') {
        return Err(CypherKeyInvalid);
    }
    let mut quoted = String::with_capacity(key.len() + 2);
    quoted.push('`');
    for ch in key.chars() {
        if ch == '`' {
            quoted.push('`');
        }
        quoted.push(ch);
    }
    quoted.push('`');
    Ok(quoted)
}

/// Render an equality predicate over `variable`'s properties: each key quoted
/// by [`quote_property_key`], each value bound as the parameter `$p<index>`.
pub fn render_property_predicate(
    variable: &str,
    keys: &[&str],
) -> Result<String, CypherKeyInvalid> {
    let mut parts = Vec::with_capacity(keys.len());
    for (index, key) in keys.iter().enumerate() {
        parts.push(format!(
            "{variable}.{} = $p{index}",
            quote_property_key(key)?
        ));
    }
    Ok(parts.join(" AND "))
}

/// The typed refusal of a property key no `Cypher` identifier can carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CypherKeyInvalid;

impl CypherKeyInvalid {
    /// The registered `ServerErrorCode` wire token.
    pub const fn code(self) -> medulla_types::contract::ServerErrorCode {
        medulla_types::contract::ServerErrorCode::CypherKeyInvalid
    }
}

/// The openCypher `EscapedSymbolicName` production: a backtick, then any run of
/// characters where a backtick appears only doubled, then a closing backtick.
/// The build gate (the `key_quoting_fixtures` test in CI) applies it to every
/// rendered fixture; any unquoted or malformed key fails.
pub fn is_escaped_symbolic_name(text: &str) -> bool {
    let Some(inner) = text
        .strip_prefix('`')
        .and_then(|rest| rest.strip_suffix('`'))
    else {
        return false;
    };
    if inner.is_empty() || inner.contains('\0') {
        return false;
    }
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch == '`' && chars.next() != Some('`') {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_spec(backend: ForeignCypherBackend) -> ForeignSourceSpec {
        ForeignSourceSpec::Cypher {
            backend,
            endpoint: "bolt://db.example.invalid:7687".into(),
            graph: "kg".into(),
            query: "MATCH (n) RETURN n.id AS id".into(),
            id_field: "id".into(),
            score_field: None,
        }
    }

    // Partial refusal coverage for MEDULLA-DURABLE-KERNEL-R024.3; successful-operation acceptance remains open.
    #[test]
    fn neo4j_well_formed_spec_is_refused_not_silently_accepted() {
        let mut registry = ForeignSourceRegistry::default();
        let err = register_neo4j(
            &mut registry,
            "n4j",
            &valid_spec(ForeignCypherBackend::Neo4j),
        )
        .unwrap_err();
        assert!(err.contains("not yet bound"), "unexpected error: {err}");
    }

    // Partial refusal coverage for MEDULLA-DURABLE-KERNEL-R024.3; successful-operation acceptance remains open.
    #[test]
    fn falkordb_well_formed_spec_is_refused_not_silently_accepted() {
        let mut registry = ForeignSourceRegistry::default();
        let err = register_falkordb(
            &mut registry,
            "falkor",
            &valid_spec(ForeignCypherBackend::FalkorDb),
        )
        .unwrap_err();
        assert!(err.contains("not yet bound"), "unexpected error: {err}");
    }

    // Partial refusal coverage for MEDULLA-DURABLE-KERNEL-R024.3; successful-operation acceptance remains open.
    #[test]
    fn neo4j_rejects_an_empty_graph_before_reaching_the_refusal() {
        let mut registry = ForeignSourceRegistry::default();
        let mut spec = valid_spec(ForeignCypherBackend::Neo4j);
        if let ForeignSourceSpec::Cypher { graph, .. } = &mut spec {
            graph.clear();
        }
        let err = register_neo4j(&mut registry, "n4j", &spec).unwrap_err();
        assert!(
            err.contains("invalid Neo4j graph"),
            "unexpected error: {err}"
        );
    }

    // Partial refusal coverage for MEDULLA-DURABLE-KERNEL-R024.3; successful-operation acceptance remains open.
    #[test]
    fn falkordb_rejects_the_wrong_backend_variant() {
        let mut registry = ForeignSourceRegistry::default();
        let err = register_falkordb(
            &mut registry,
            "falkor",
            &valid_spec(ForeignCypherBackend::Age),
        )
        .unwrap_err();
        assert!(
            err.contains("expected a FalkorDb Cypher source"),
            "unexpected error: {err}"
        );
    }

    /// Verifies MEDULLA-FEDERATED-QUERY-R053.100: Generated `Cypher` property keys use backtick quoting with a build gate
    // spec: MEDULLA-FEDERATED-QUERY-R053.100
    #[test]
    fn key_quoting_fixtures() {
        let fixtures = [
            ("name", "`name`"),
            ("MATCH", "`MATCH`"),
            ("first name", "`first name`"),
            ("prénom_名前", "`prénom_名前`"),
            ("a`b", "`a``b`"),
        ];
        for (key, expected) in fixtures {
            let quoted = quote_property_key(key).expect("fixture key quotes");
            assert_eq!(quoted, expected);
            assert!(is_escaped_symbolic_name(&quoted), "{quoted}");
        }
        // The one unescaped key the gate rejects.
        assert!(!is_escaped_symbolic_name("`a`b`"));
        assert!(!is_escaped_symbolic_name("name"));
        assert_eq!(quote_property_key(""), Err(CypherKeyInvalid));
        assert_eq!(
            quote_property_key("a\0b").unwrap_err().code().as_str(),
            "CYPHER_KEY_INVALID"
        );
        // Values stay parameters: the rendered predicate carries no literal.
        let predicate = render_property_predicate("n", &["first name", "a`b"]).unwrap();
        assert_eq!(predicate, "n.`first name` = $p0 AND n.`a``b` = $p1");
    }
}
