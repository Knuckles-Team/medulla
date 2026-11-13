//! MEDULLA-FEDERATED-QUERY-R053.100: the one quoting rule for every generated
//! `Cypher` property key. The Cypher source itself is bound for the AGE
//! backend only (MEDULLA-DURABLE-KERNEL-R024.3, `federation_age`).

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
