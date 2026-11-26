//! The tenant's Agent Library listing and its opaque page cursor
//! (MEDULLA-TYPED-PACKS-R011.1).
//!
//! A read contract of its own: a bounded request, the page it returns and the tenant-bound
//! cursor that resumes it. Nothing here participates in the definition digest.

use serde::{Deserialize, Serialize};

use super::{validate_key, validate_text, AgentLibraryEntry, AgentLibraryLifecycle};
use medulla_types::tenant_cursor::CursorFamily;

/// Most entries one listing page may carry.
pub const MAX_AGENT_LIBRARY_LIST_LIMIT: u32 = 256;
/// Longest opaque listing cursor a caller may hand back.
pub const MAX_AGENT_LIBRARY_LIST_CURSOR_BYTES: usize = 16 * 1024;
/// Format-identity constant (RF-ADR-006) for the tenant binding inside a listing cursor.
pub const AGENT_LIBRARY_LIST_CURSOR_DOMAIN: &[u8] = b"au-eg/agent-library-list-cursor/v1";

/// Which library an entry belongs to (MEDULLA-TYPED-PACKS-R011.2). The engine holds agents and,
/// in the component library beside them, tools. It holds no suggestions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum AgentLibraryKind {
    /// An agent definition.
    #[default]
    Agent,
    /// A tool component.
    Tool,
    /// A suggestion. The engine holds none: a request for this kind refuses `PARAMETER_INVALID`.
    Suggestion,
}

/// One entry of a listing: its identity, the digest of its current definition, the revision
/// that is its head and that head's lifecycle. `agent_id` is the id of the entry in its own
/// library: the agent id, or the component id of a tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct AgentLibraryListEntry {
    /// The library the entry belongs to; `agent` when absent.
    #[serde(default)]
    pub kind: AgentLibraryKind,
    pub agent_id: String,
    pub definition_digest: String,
    pub entry_revision: u64,
    pub lifecycle: AgentLibraryLifecycle,
}

impl From<&AgentLibraryEntry> for AgentLibraryListEntry {
    fn from(entry: &AgentLibraryEntry) -> Self {
        Self {
            kind: AgentLibraryKind::Agent,
            agent_id: entry.agent_id.clone(),
            definition_digest: entry.definition_digest.clone(),
            entry_revision: entry.entry_revision,
            lifecycle: entry.lifecycle,
        }
    }
}

impl AgentLibraryListEntry {
    /// The tool entry a component's head revision is.
    pub fn of_tool(entry: &crate::agent_component::AgentComponentEntry) -> Self {
        Self {
            kind: AgentLibraryKind::Tool,
            agent_id: entry.component_id.clone(),
            definition_digest: entry.definition_digest.clone(),
            entry_revision: entry.entry_revision,
            lifecycle: entry.lifecycle,
        }
    }
}

/// The refusal of a request for a library the engine does not hold.
pub fn unsupported_kind_refusal(kind: AgentLibraryKind) -> String {
    let name = match kind {
        AgentLibraryKind::Agent => "agent",
        AgentLibraryKind::Tool => "tool",
        AgentLibraryKind::Suggestion => "suggestion",
    };
    format!("PARAMETER_INVALID: the engine holds no {name} library entries")
}

/// One page of a listing, in agent id order. An object, never a bare array, so the cursor has
/// somewhere to live.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct AgentLibraryPage {
    pub entries: Vec<AgentLibraryListEntry>,
    /// `Some` when more of the tenant remains. Hand it back unmodified to continue; `None`
    /// means the library is exhausted. A page may be empty and still carry a cursor, because
    /// the engine bounds how many heads one page examines, so loop until `None`.
    #[serde(default)]
    pub next_cursor: Option<String>,
}

/// Refuse a listing request outside its bounds: a tenant that is not a valid key, a limit of
/// zero or above [`MAX_AGENT_LIBRARY_LIST_LIMIT`], an empty or oversized cursor.
pub fn validate_list_request(
    tenant_id: &str,
    limit: Option<u32>,
    cursor: Option<&str>,
) -> Result<(), String> {
    validate_key(tenant_id, "agent-library")?;
    if let Some(limit) = limit {
        if limit == 0 || limit > MAX_AGENT_LIBRARY_LIST_LIMIT {
            return Err(format!(
                "agent library list limit must be 1..={MAX_AGENT_LIBRARY_LIST_LIMIT}"
            ));
        }
    }
    if let Some(cursor) = cursor {
        if cursor.is_empty() || cursor.len() > MAX_AGENT_LIBRARY_LIST_CURSOR_BYTES {
            return Err("agent library list cursor is outside its bound".to_string());
        }
    }
    Ok(())
}

/// The page size a request asks for, defaulted and already bounded.
pub fn list_page_limit(limit: Option<u32>) -> usize {
    limit
        .unwrap_or(MAX_AGENT_LIBRARY_LIST_LIMIT)
        .min(MAX_AGENT_LIBRARY_LIST_LIMIT) as usize
}

/// The listing's cursor family. The framing and the tenant-bound tag are owned by
/// [`medulla_types::tenant_cursor`]; this row only names the listing.
const LIST_CURSOR: CursorFamily = CursorFamily {
    domain: AGENT_LIBRARY_LIST_CURSOR_DOMAIN,
    max_bytes: MAX_AGENT_LIBRARY_LIST_CURSOR_BYTES,
    noun: "agent library list",
};

/// Mint the opaque cursor that resumes a listing after `agent_id`.
pub fn encode_list_cursor(tenant_id: &str, agent_id: &str) -> String {
    LIST_CURSOR.encode(tenant_id, agent_id)
}

/// Recover the resume agent id from an opaque cursor, or refuse it by name.
pub fn decode_list_cursor(tenant_id: &str, cursor: &str) -> Result<String, String> {
    LIST_CURSOR.decode(tenant_id, cursor, |agent_id| {
        validate_text("cursor agent_id", agent_id).is_ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-TYPED-PACKS-R011.1
    #[test]
    fn a_list_cursor_resumes_only_for_the_tenant_that_minted_it() {
        let cursor = encode_list_cursor("tenant-a", "agent-7");
        assert_eq!(decode_list_cursor("tenant-a", &cursor).unwrap(), "agent-7");
        assert!(decode_list_cursor("tenant-b", &cursor)
            .unwrap_err()
            .contains("was not minted for this tenant"));
        assert!(decode_list_cursor("tenant-a", "zz")
            .unwrap_err()
            .contains("malformed"));
    }

    // spec: MEDULLA-TYPED-PACKS-R011.2
    #[test]
    fn a_kind_defaults_to_agent_and_a_suggestion_is_refused_by_name() {
        assert_eq!(AgentLibraryKind::default(), AgentLibraryKind::Agent);
        assert_eq!(
            serde_json::to_value(AgentLibraryKind::Tool).expect("json"),
            serde_json::json!("tool")
        );
        let entry: AgentLibraryListEntry = serde_json::from_value(serde_json::json!({
            "agent_id": "a", "definition_digest": "d", "entry_revision": 1, "lifecycle": "published"
        }))
        .expect("an entry with no kind is an agent");
        assert_eq!(entry.kind, AgentLibraryKind::Agent);
        let refusal = unsupported_kind_refusal(AgentLibraryKind::Suggestion);
        assert!(refusal.starts_with("PARAMETER_INVALID:"), "{refusal}");
    }

    // spec: MEDULLA-TYPED-PACKS-R011.1
    #[test]
    fn a_list_request_outside_its_bounds_is_refused() {
        assert!(validate_list_request("tenant-a", None, None).is_ok());
        assert!(validate_list_request("tenant-a", Some(256), Some("abc")).is_ok());
        assert!(validate_list_request("tenant-a", Some(0), None).is_err());
        assert!(validate_list_request("tenant-a", Some(257), None).is_err());
        assert!(validate_list_request("tenant-a", None, Some("")).is_err());
        assert!(validate_list_request("", None, None).is_err());
        assert_eq!(list_page_limit(None), 256);
        assert_eq!(list_page_limit(Some(10)), 10);
    }
}
