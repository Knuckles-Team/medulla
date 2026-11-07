//! How a pack entry's name becomes a component id.
//!
//! `mcp:<connector>/<kind_token>/<escaped_name>`, where the escaped name
//! percent-encodes every UTF-8 byte outside `[A-Za-z0-9._-]` with uppercase
//! hex. That set is chosen so `/`, `~`, `%`, spaces and every non-ASCII byte
//! are encoded: the id is a path-shaped key in a durable store, and a name
//! that could inject a separator could address another entry's row.
//!
//! The mapping is total and injective for a given kind, so two entries collide
//! only when they really are the same `(kind, name)` -- which the importer
//! refuses as `DUPLICATE_COMPONENT_ID` rather than silently revising one.

use super::digest::entry_kind_token;
use super::index::PackEntryKind;

/// The prefix every pack-owned component id carries.
pub const PACK_COMPONENT_ID_PREFIX: &str = "mcp:";

const PACK_PROJECTION_GRAPH_DOMAIN: &[u8] = b"eg/connector-pack-projection-graph/v1";

/// Tenant-safe, non-reversible physical graph identity for one connector pack.
///
/// Both inputs are canonical identifiers from verified/typed authority.  The
/// physical name exposes neither tenant nor connector and the framed digest
/// preserves their boundary, while the logical GraphSchema key inside that
/// graph remains the human-readable `pack:<connector>`.
pub fn pack_projection_graph_name(
    tenant_id: &str,
    connector: &medulla_types::contract::ResourceId,
) -> Result<String, String> {
    medulla_types::contract::TenantId::new(tenant_id).map_err(|error| {
        format!("invalid verified tenant for connector pack projection: {error}")
    })?;
    let digest = medulla_types::contract::Digest256::framed(
        PACK_PROJECTION_GRAPH_DOMAIN,
        &[tenant_id.as_bytes(), connector.as_str().as_bytes()],
    )?;
    Ok(format!("pack__{}", digest.to_hex()))
}

const HEX: &[u8; 16] = b"0123456789ABCDEF";

/// Percent-encode `name` under the pack's unreserved set.
pub fn escape_pack_name(name: &str) -> String {
    let mut escaped = String::with_capacity(name.len());
    for byte in name.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-') {
            escaped.push(*byte as char);
        } else {
            escaped.push('%');
            escaped.push(HEX[usize::from(byte >> 4)] as char);
            escaped.push(HEX[usize::from(byte & 0x0f)] as char);
        }
    }
    escaped
}

/// A connector id is a [`crate::contract::ResourceId`] that also contains no
/// `/`: `mcp:<connector>/<kind>/<name>` is an unambiguous prefix only if the
/// connector cannot itself contain the separator. A grant-scoped probe that
/// sees a different tool set is its own connector
/// (`<server>@grant-<16 hex of the grant digest>`).
pub fn validate_connector(connector: &medulla_types::contract::ResourceId) -> Result<(), String> {
    if connector.as_str().contains('/') {
        return Err("MALFORMED_INDEX: a connector id must not contain '/'".to_string());
    }
    Ok(())
}

/// The component id one pack entry publishes under.
pub fn pack_component_id(connector: &str, kind: PackEntryKind, name: &str) -> String {
    format!(
        "{PACK_COMPONENT_ID_PREFIX}{connector}/{}/{}",
        entry_kind_token(kind),
        escape_pack_name(name)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::contract::ResourceId;

    #[test]
    fn a_connector_id_never_contains_the_component_separator() {
        assert!(validate_connector(&ResourceId::new("freshrss-mcp").unwrap()).is_ok());
        assert!(validate_connector(
            &ResourceId::new("freshrss-mcp@grant-0123456789abcdef").unwrap()
        )
        .is_ok());
        let error = validate_connector(&ResourceId::new("fresh/rss").unwrap()).unwrap_err();
        assert!(error.starts_with("MALFORMED_INDEX:"), "{error}");
    }
}
